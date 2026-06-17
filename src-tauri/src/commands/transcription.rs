use crate::actions::process_transcription_output;
use crate::managers::history::{HistoryEntry, HistoryManager};
use crate::managers::transcription::{
    TranscriptionManager, TranscriptionProgress, TranscriptionProgressCallback,
};
use crate::settings::{get_settings, write_settings, ModelUnloadTimeout};
use serde::Serialize;
use specta::Type;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Serialize, Type)]
pub struct ModelLoadStatus {
    is_loaded: bool,
    current_model: Option<String>,
}

#[derive(Serialize, Type)]
pub struct FileTranscriptionResult {
    text: String,
    raw_text: String,
    post_processed_text: Option<String>,
    history_entry: Option<HistoryEntry>,
}

#[derive(Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FileTranscriptionProgressEvent {
    stage: String,
    text: Option<String>,
    progress: Option<i32>,
}

fn emit_file_transcription_progress(
    app: &AppHandle,
    stage: &str,
    text: Option<String>,
    progress: Option<i32>,
) {
    let _ = app.emit(
        "file-transcription-progress",
        FileTranscriptionProgressEvent {
            stage: stage.to_string(),
            text,
            progress,
        },
    );
}

#[tauri::command]
#[specta::specta]
pub fn set_model_unload_timeout(app: AppHandle, timeout: ModelUnloadTimeout) {
    let mut settings = get_settings(&app);
    settings.model_unload_timeout = timeout;
    write_settings(&app, settings);
}

#[tauri::command]
#[specta::specta]
pub fn get_model_load_status(
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
) -> Result<ModelLoadStatus, String> {
    Ok(ModelLoadStatus {
        is_loaded: transcription_manager.is_model_loaded(),
        current_model: transcription_manager.get_current_model(),
    })
}

#[tauri::command]
#[specta::specta]
pub fn unload_model_manually(
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
) -> Result<(), String> {
    transcription_manager
        .unload_model()
        .map_err(|e| format!("Failed to unload model: {}", e))
}

#[tauri::command]
#[specta::specta]
pub fn cancel_file_transcription(transcription_manager: State<'_, Arc<TranscriptionManager>>) {
    transcription_manager.cancel_file_transcription();
}

#[tauri::command]
#[specta::specta]
pub async fn transcribe_file(
    app: AppHandle,
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
    history_manager: State<'_, Arc<HistoryManager>>,
    path: String,
    post_process: bool,
) -> Result<FileTranscriptionResult, String> {
    let source_path = PathBuf::from(&path);
    if !source_path.is_file() {
        return Err("Selected path is not a file".to_string());
    }

    transcription_manager.reset_file_transcription_cancelled();

    emit_file_transcription_progress(&app, "loading_model", None, None);
    transcription_manager.initiate_model_load();

    if transcription_manager.is_file_transcription_cancelled() {
        return Err("Cancelled".to_string());
    }

    emit_file_transcription_progress(&app, "transcribing", None, Some(0));
    let decode_path = source_path.clone();

    let vad_path = app
        .path()
        .resolve(
            "resources/models/silero_vad_v4.onnx",
            tauri::path::BaseDirectory::Resource,
        )
        .ok()
        .filter(|p| p.exists());

    const DECODE_CHUNK_SIZE: usize = 16_000 * 10; // 10 seconds of 16 kHz audio

    let tm = Arc::clone(&transcription_manager);
    let progress_app = app.clone();
    let cancel_tm = Arc::clone(&transcription_manager);
    let max_progress = Arc::new(AtomicI32::new(0));

    let wav_file_name = format!(
        "hanhcute-file-{}.wav",
        chrono::Utc::now().timestamp_millis()
    );
    let wav_path = history_manager.recordings_dir().join(&wav_file_name);

    let emit_monotonic_progress =
        |app: &AppHandle, text: Option<String>, proposed: i32, max: &AtomicI32| {
            let actual = max.fetch_max(proposed, Ordering::SeqCst).max(proposed);
            emit_file_transcription_progress(app, "transcribing", text, Some(actual));
        };

    let streaming_result =
        tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
            let mut decoder = crate::audio_toolkit::MediaFileDecoder::open(&decode_path)
                .map_err(|e| format!("Failed to open media file: {}", e))?;
            let duration_secs = decoder.duration_seconds();
            let total_samples_estimate = duration_secs.map(|d| {
                (d * crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE as f64) as usize
            });

            let mut vad = match vad_path {
                Some(ref path) => Some(
                    crate::audio_toolkit::StreamingVad::new(path)
                        .map_err(|e| format!("Failed to initialise VAD: {}", e))?,
                ),
                None => None,
            };

            let mut wav_writer = crate::audio_toolkit::StreamingWavWriter::create(&wav_path)
                .map_err(|e| format!("Failed to create history WAV file: {}", e))?;
            let mut saved_sample_count: usize = 0;

            let mut current_chunk: Vec<f32> = Vec::new();
            let mut chunk_texts: Vec<String> = Vec::new();
            let accumulated_text = Arc::new(std::sync::Mutex::new(String::new()));
            let mut decoded_samples: usize = 0;
            let mut prev_in_speech = false;
            let mut current_chunk_start_decoded: usize = 0;

            let make_callback = |chunk_accumulated: Arc<std::sync::Mutex<String>>,
                                 chunk_start_decoded: usize,
                                 chunk_end_decoded: usize,
                                 chunk_max_progress: Arc<AtomicI32>|
             -> TranscriptionProgressCallback {
                let chunk_progress_app = progress_app.clone();
                let chunk_cancel_tm = Arc::clone(&cancel_tm);
                let chunk_total_samples = total_samples_estimate;
                Arc::new(move |progress: TranscriptionProgress| {
                    if chunk_cancel_tm.is_file_transcription_cancelled() {
                        return;
                    }

                    let overall_text = {
                        let previous = chunk_accumulated.lock().unwrap().clone();
                        match progress.text {
                            Some(ref current) if !current.is_empty() => {
                                if previous.is_empty() {
                                    current.clone()
                                } else {
                                    format!("{} {}", previous, current)
                                }
                            }
                            _ => previous,
                        }
                    };

                    let proposed_progress = chunk_total_samples.and_then(|total| {
                        if total == 0 {
                            return None;
                        }
                        let whisper_pct =
                            progress.progress.map(|p| p as f64 / 100.0).unwrap_or(0.0);
                        let current_pos = chunk_start_decoded as f64
                            + (chunk_end_decoded - chunk_start_decoded) as f64 * whisper_pct;
                        let pct = (current_pos / total as f64) * 100.0;
                        Some(pct.min(100.0) as i32)
                    });

                    if let Some(proposed) = proposed_progress {
                        let actual = chunk_max_progress
                            .fetch_max(proposed, Ordering::SeqCst)
                            .max(proposed);
                        emit_file_transcription_progress(
                            &chunk_progress_app,
                            "transcribing",
                            Some(overall_text),
                            Some(actual),
                        );
                    } else {
                        emit_file_transcription_progress(
                            &chunk_progress_app,
                            "transcribing",
                            Some(overall_text),
                            None,
                        );
                    }
                })
            };

            while let Some(audio_chunk) = decoder
                .decode_chunk(DECODE_CHUNK_SIZE)
                .map_err(|e| format!("Failed to decode media file: {}", e))?
            {
                if tm.is_file_transcription_cancelled() {
                    return Err("Cancelled".to_string());
                }

                let audio_chunk_len = audio_chunk.len();
                decoded_samples += audio_chunk_len;

                if let Some(total) = total_samples_estimate {
                    if total > 0 {
                        let progress =
                            ((decoded_samples as f64 / total as f64) * 100.0).min(100.0) as i32;
                        emit_monotonic_progress(&progress_app, None, progress, &max_progress);
                    }
                }

                let speech = match vad.as_mut() {
                    Some(v) => v
                        .push(&audio_chunk)
                        .map_err(|e| format!("VAD processing failed: {}", e))?,
                    None => audio_chunk,
                };

                wav_writer
                    .write_samples(&speech)
                    .map_err(|e| format!("Failed to write history WAV: {}", e))?;
                saved_sample_count += speech.len();

                if current_chunk.is_empty() {
                    current_chunk_start_decoded = decoded_samples.saturating_sub(audio_chunk_len);
                }
                current_chunk.extend_from_slice(&speech);

                let in_speech = vad.as_ref().map(|v| v.is_in_speech()).unwrap_or(true);
                if prev_in_speech && !in_speech && !current_chunk.is_empty() {
                    let callback = make_callback(
                        Arc::clone(&accumulated_text),
                        current_chunk_start_decoded,
                        decoded_samples,
                        Arc::clone(&max_progress),
                    );
                    let text = tm
                        .transcribe_chunk_with_progress(&current_chunk, Some(callback), true, true)
                        .map_err(|e| format!("Transcription failed: {}", e))?;
                    if !text.is_empty() {
                        {
                            let mut acc = accumulated_text.lock().unwrap();
                            if !acc.is_empty() {
                                acc.push('\n');
                            }
                            acc.push_str(&text);
                        }
                        chunk_texts.push(text);
                    }
                    current_chunk.clear();
                }
                prev_in_speech = in_speech;
            }

            if tm.is_file_transcription_cancelled() {
                return Err("Cancelled".to_string());
            }

            // Flush any trailing speech buffered inside the VAD.
            if let Some(v) = vad.as_mut() {
                let speech = v.flush().map_err(|e| format!("VAD flush failed: {}", e))?;
                wav_writer
                    .write_samples(&speech)
                    .map_err(|e| format!("Failed to write history WAV: {}", e))?;
                saved_sample_count += speech.len();
                current_chunk.extend_from_slice(&speech);
            }

            // Transcribe any remaining speech at the end of the file.
            if !current_chunk.is_empty() {
                let callback = make_callback(
                    Arc::clone(&accumulated_text),
                    current_chunk_start_decoded,
                    decoded_samples,
                    Arc::clone(&max_progress),
                );
                let text = tm
                    .transcribe_chunk_with_progress(&current_chunk, Some(callback), false, true)
                    .map_err(|e| format!("Transcription failed: {}", e))?;
                if !text.is_empty() {
                    {
                        let mut acc = accumulated_text.lock().unwrap();
                        if !acc.is_empty() {
                            acc.push('\n');
                        }
                        acc.push_str(&text);
                    }
                    chunk_texts.push(text);
                }
            }

            // Finalise the history WAV and verify the sample count.
            wav_writer
                .finalize()
                .map_err(|e| format!("Failed to finalise history WAV: {}", e))?;
            crate::audio_toolkit::verify_wav_file(&wav_path, saved_sample_count)
                .map_err(|e| format!("History WAV verification failed: {}", e))?;

            Ok(chunk_texts.join("\n").trim().to_string())
        })
        .await
        .map_err(|e| format!("Streaming transcription task panicked: {}", e))?
        .map_err(|e| e.to_string())?;

    if transcription_manager.is_file_transcription_cancelled() {
        return Err("Cancelled".to_string());
    }

    let raw_text = streaming_result;

    if raw_text.is_empty() {
        return Err("Selected file contains no transcribable speech".to_string());
    }

    emit_file_transcription_progress(&app, "transcribing", Some(raw_text.clone()), Some(100));
    if post_process {
        emit_file_transcription_progress(&app, "post_processing", Some(raw_text.clone()), None);
    }
    let processed = process_transcription_output(&app, &raw_text, post_process).await;
    let text = processed.final_text;

    if transcription_manager.is_file_transcription_cancelled() {
        return Err("Cancelled".to_string());
    }

    let history_entry = {
        match history_manager.save_entry(
            wav_file_name,
            raw_text.clone(),
            post_process,
            processed.post_processed_text.clone(),
            processed.post_process_prompt,
        ) {
            Ok(entry) => Some(entry),
            Err(error) => {
                log::error!("Failed to save file transcription history entry: {}", error);
                None
            }
        }
    };

    if transcription_manager.is_file_transcription_cancelled() {
        return Err("Cancelled".to_string());
    }

    emit_file_transcription_progress(&app, "complete", Some(text.clone()), Some(100));

    Ok(FileTranscriptionResult {
        text,
        raw_text,
        post_processed_text: processed.post_processed_text,
        history_entry,
    })
}

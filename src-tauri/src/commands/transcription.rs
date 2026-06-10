use crate::actions::process_transcription_output;
use crate::managers::history::{HistoryEntry, HistoryManager};
use crate::managers::transcription::{
    TranscriptionManager, TranscriptionProgress, TranscriptionProgressCallback,
};
use crate::settings::{get_settings, write_settings, ModelUnloadTimeout};
use serde::Serialize;
use specta::Type;
use std::path::PathBuf;
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

    emit_file_transcription_progress(&app, "decoding", None, None);
    let decode_path = source_path.clone();

    let vad_path = app
        .path()
        .resolve(
            "resources/models/silero_vad_v4.onnx",
            tauri::path::BaseDirectory::Resource,
        )
        .ok()
        .filter(|p| p.exists());

    let samples = tauri::async_runtime::spawn_blocking(move || {
        let raw = crate::audio_toolkit::read_media_file_samples(&decode_path)?;
        match vad_path {
            Some(ref path) => crate::audio_toolkit::vad_filter_samples(&raw, path),
            None => Ok(raw),
        }
    })
    .await
    .map_err(|e| format!("Audio decode task panicked: {}", e))?
    .map_err(|e| format!("Failed to process audio file: {}", e))?;
    if transcription_manager.is_file_transcription_cancelled() {
        return Err("Cancelled".to_string());
    }

    if samples.is_empty() {
        return Err("Selected file contains no audio samples".to_string());
    }

    let samples_for_history = samples.clone();
    let tm = Arc::clone(&transcription_manager);
    let progress_app = app.clone();
    let cancel_tm = Arc::clone(&transcription_manager);
    let progress_callback: TranscriptionProgressCallback =
        Arc::new(move |progress: TranscriptionProgress| {
            if cancel_tm.is_file_transcription_cancelled() {
                return;
            }
            emit_file_transcription_progress(
                &progress_app,
                "transcribing",
                progress.text,
                progress.progress,
            );
        });

    emit_file_transcription_progress(&app, "transcribing", None, Some(0));
    let raw_text = tauri::async_runtime::spawn_blocking(move || {
        tm.transcribe_with_progress(samples, Some(progress_callback))
    })
    .await
    .map_err(|e| format!("Transcription task panicked: {}", e))?
    .map_err(|e| e.to_string())?;

    if transcription_manager.is_file_transcription_cancelled() {
        return Err("Cancelled".to_string());
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
        let file_name = format!(
            "hanhcute-file-{}.wav",
            chrono::Utc::now().timestamp_millis()
        );
        let wav_path = history_manager.recordings_dir().join(&file_name);
        let sample_count = samples_for_history.len();
        let save_result = tauri::async_runtime::spawn_blocking(move || {
            crate::audio_toolkit::save_wav_file(&wav_path, &samples_for_history)?;
            crate::audio_toolkit::verify_wav_file(&wav_path, sample_count)
        })
        .await;

        match save_result {
            Ok(Ok(())) => match history_manager.save_entry(
                file_name,
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
            },
            Ok(Err(error)) => {
                log::error!("Failed to save decoded file audio: {}", error);
                None
            }
            Err(error) => {
                log::error!("File audio save task panicked: {}", error);
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

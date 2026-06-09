use crate::actions::process_transcription_output;
use crate::managers::history::{HistoryEntry, HistoryManager};
use crate::managers::transcription::TranscriptionManager;
use crate::settings::{get_settings, write_settings, ModelUnloadTimeout};
use serde::Serialize;
use specta::Type;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, State};

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

    transcription_manager.initiate_model_load();

    let decode_path = source_path.clone();
    let samples = tauri::async_runtime::spawn_blocking(move || {
        crate::audio_toolkit::read_media_file_samples(&decode_path)
    })
    .await
    .map_err(|e| format!("Audio decode task panicked: {}", e))?
    .map_err(|e| format!("Failed to decode media file: {}", e))?;

    if samples.is_empty() {
        return Err("Selected file contains no audio samples".to_string());
    }

    let samples_for_history = samples.clone();
    let tm = Arc::clone(&transcription_manager);
    let raw_text = tauri::async_runtime::spawn_blocking(move || tm.transcribe(samples))
        .await
        .map_err(|e| format!("Transcription task panicked: {}", e))?
        .map_err(|e| e.to_string())?;

    let processed = process_transcription_output(&app, &raw_text, post_process).await;
    let text = processed.final_text;

    let history_entry = {
        let file_name = format!("handy-file-{}.wav", chrono::Utc::now().timestamp_millis());
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

    Ok(FileTranscriptionResult {
        text,
        raw_text,
        post_processed_text: processed.post_processed_text,
        history_entry,
    })
}

// CI-only mock TranscriptionManager - avoids whisper/Vulkan dependencies.
// This file is copied over transcription.rs during CI tests.
// Existing tests don't exercise transcription, so this is safe.

use crate::managers::model::ModelManager;
use crate::settings::TimestampMode;
use anyhow::Result;
use serde::Serialize;
use specta::Type;
use std::sync::Arc;
use tauri::AppHandle;

#[derive(Clone, Debug, Serialize)]
pub struct ModelStateEvent {
    pub event_type: String,
    pub model_id: Option<String>,
    pub model_name: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TranscriptionProgress {
    pub text: Option<String>,
    pub progress: Option<i32>,
}

pub type TranscriptionProgressCallback = Arc<dyn Fn(TranscriptionProgress) + Send + Sync>;

/// RAII guard that is a no-op in the mock — mirrors the real `LoadingGuard`.
pub struct LoadingGuard;

#[derive(Clone)]
pub struct TranscriptionManager {
    #[allow(dead_code)]
    app_handle: AppHandle,
}

impl TranscriptionManager {
    pub fn new(app_handle: &AppHandle, _model_manager: Arc<ModelManager>) -> Result<Self> {
        Ok(Self {
            app_handle: app_handle.clone(),
        })
    }

    pub fn is_model_loaded(&self) -> bool {
        false
    }

    pub fn try_start_loading(&self) -> Option<LoadingGuard> {
        Some(LoadingGuard)
    }

    pub fn unload_model(&self) -> Result<()> {
        Ok(())
    }

    pub fn maybe_unload_immediately(&self, _context: &str) {}

    pub fn load_model(&self, _model_id: &str) -> Result<()> {
        Ok(())
    }

    pub fn initiate_model_load(&self) {}

    pub fn get_current_model(&self) -> Option<String> {
        None
    }

    pub fn transcribe(&self, _audio: Vec<f32>) -> Result<String> {
        Ok(String::new())
    }

    pub fn transcribe_segments(
        &self,
        _audio: Vec<f32>,
        _segments: Vec<(usize, usize)>,
    ) -> Result<String> {
        Ok(String::new())
    }

    pub fn transcribe_chunk_with_progress(
        &self,
        _audio: &[f32],
        _progress_callback: Option<TranscriptionProgressCallback>,
        _skip_immediate_unload: bool,
        _no_context: bool,
        _base_offset_seconds: f64,
        _timestamp_mode: TimestampMode,
    ) -> Result<String> {
        Ok(String::new())
    }

    pub fn cancel_file_transcription(&self) {}

    pub fn is_file_transcription_cancelled(&self) -> bool {
        false
    }

    pub fn reset_file_transcription_cancelled(&self) {}
}

/// No-op in CI mock.
pub fn apply_accelerator_settings(_app: &tauri::AppHandle) {}

#[derive(Serialize, Clone, Debug, Type)]
pub struct GpuDeviceOption {
    pub id: i32,
    pub name: String,
    pub total_vram_mb: usize,
}

#[derive(Serialize, Clone, Debug, Type)]
pub struct AvailableAccelerators {
    pub whisper: Vec<String>,
    pub ort: Vec<String>,
    pub gpu_devices: Vec<GpuDeviceOption>,
}

/// Returns empty lists in CI mock.
pub fn get_available_accelerators() -> AvailableAccelerators {
    AvailableAccelerators {
        whisper: vec![],
        ort: vec![],
        gpu_devices: vec![],
    }
}

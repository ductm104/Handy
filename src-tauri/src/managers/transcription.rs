use crate::audio_toolkit::{apply_custom_words, filter_transcription_output};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::model::{EngineType, ModelManager};
use crate::settings::{
    get_settings, ModelUnloadTimeout, OrtAcceleratorSetting, TimestampMode,
    WhisperAcceleratorSetting,
};
use anyhow::Result;
use log::{debug, error, info, warn};
use serde::Serialize;
use specta::Type;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter, Manager};
use transcribe_cpp::{
    Backend, CancelToken, Device, DeviceType, Model, ModelOptions, RunExtension, RunOptions,
    Session, Task, TimestampKind as CppTimestampKind, WhisperRunOptions,
};
use transcribe_rs::{
    onnx::{
        canary::CanaryModel,
        cohere::CohereModel,
        gigaam::GigaAMModel,
        moonshine::{MoonshineModel, MoonshineVariant, StreamingModel},
        parakeet::{ParakeetModel, ParakeetParams, TimestampGranularity},
        sense_voice::{SenseVoiceModel, SenseVoiceParams},
        Quantization,
    },
    SpeechModel, TranscribeOptions, TranscriptionResult, TranscriptionSegment,
};

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

/// Format a duration in seconds as `hh:mm:ss`.
pub fn format_seconds_hhmmss(seconds: f64) -> String {
    let total_seconds = seconds.max(0.0) as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}

/// Format a begin/end range as `[hh:mm:ss - hh:mm:ss]`.
pub fn format_timestamp_range(begin: f64, end: f64) -> String {
    format!(
        "[{} - {}]",
        format_seconds_hhmmss(begin),
        format_seconds_hhmmss(end)
    )
}

/// Maximum wall-clock duration of a single grouped-timestamp block.
///
/// Consecutive segments whose combined span stays under this threshold are
/// merged into one `[begin - end] text...` line. Once adding another segment
/// would push the group past this length, a new block is started.
const GROUP_TIMESTAMP_MAX_DURATION_SECS: f64 = 30.0;

/// Silence gap (in seconds) that is treated as a paragraph break.
///
/// When the gap between the end of the previous segment and the start of the
/// next one exceeds this value, a new grouped-timestamp block is started even
/// if the maximum duration has not been reached yet.
const GROUP_TIMESTAMP_PARAGRAPH_GAP_SECS: f64 = 1.5;

/// Merge consecutive transcription segments into paragraph-sized blocks and
/// prefix each block with a single continuous `[hh:mm:ss - hh:mm:ss]` range.
///
/// The range of every block is continuous: it starts at the first segment's
/// begin time and ends at the last segment's end time, so the timeline flows
/// naturally from one block to the next without overlaps or holes.
fn group_segments_with_timestamps(
    segs: &[TranscriptionSegment],
    base_offset_seconds: f64,
) -> String {
    // Each group: (begin, end, joined_texts).
    let mut groups: Vec<(f64, f64, Vec<String>)> = Vec::new();

    for s in segs {
        let text = s.text.trim();
        if text.is_empty() {
            continue;
        }

        let seg_begin = base_offset_seconds + s.start as f64;
        let seg_end = base_offset_seconds + s.end as f64;
        if seg_end <= seg_begin {
            continue;
        }

        // Decide whether to append to the current group or start a new one.
        let start_new = match groups.last_mut() {
            None => true,
            Some((g_begin, g_end, texts)) => {
                let gap = (seg_begin - *g_end).max(0.0);
                let new_duration = seg_end - *g_begin;
                if gap > GROUP_TIMESTAMP_PARAGRAPH_GAP_SECS
                    || new_duration > GROUP_TIMESTAMP_MAX_DURATION_SECS
                {
                    true
                } else {
                    // Extend the continuous range; guard against any overlap by
                    // taking the max of the current end and the segment end so
                    // the recorded range stays monotonic and gap-free.
                    *g_end = g_end.max(seg_end);
                    texts.push(text.to_string());
                    false
                }
            }
        };

        if start_new {
            groups.push((seg_begin, seg_end, vec![text.to_string()]));
        }
    }

    groups
        .into_iter()
        .map(|(begin, end, texts)| {
            format!("{} {}", format_timestamp_range(begin, end), texts.join(" "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whisper-family inference via transcribe-cpp.
///
/// This replaced the old whisper-rs engine. transcribe-cpp loads both GGUF
/// models and legacy whisper.cpp `.bin` files (e.g. the PhoWhisper GGML
/// downloads), so no model conversion was needed for the migration.
///
/// `Session` keeps its `Model` alive internally, so one loaded engine serves
/// repeated dictation and file-chunk runs without reloading. Segment times
/// come back in milliseconds and are mapped here to the
/// `transcribe_rs::TranscriptionResult` (seconds) shape the rest of the
/// pipeline — timestamp modes, file transcription, custom words — expects.
struct TranscribeCppEngine {
    session: Session,
    cancel_token: CancelToken,
    /// GGUF `general.architecture` (`"whisper"` for the whisper family,
    /// including legacy `.bin` files). An empty arch is treated as whisper
    /// too, since only whisper-family models are ever routed to this engine.
    arch: String,
}

/// Run once per process: route native transcribe-cpp logs into the `log`
/// facade and register compute backends (a no-op for static builds such as
/// macOS/metal, required before device enumeration on dynamic builds).
static TRANSCRIBE_BACKEND_INIT: OnceLock<()> = OnceLock::new();

fn ensure_transcribe_backends() {
    TRANSCRIBE_BACKEND_INIT.get_or_init(|| {
        transcribe_cpp::init_logging();
        if let Err(e) = transcribe_cpp::init_backends_default() {
            warn!("transcribe-cpp backend init failed: {}", e);
        }
    });
}

/// Map the persisted `WhisperAcceleratorSetting` (+ optional exact GPU index)
/// to a transcribe-cpp backend request.
///
/// `Auto` and `Gpu` both prefer GPU when one is available; `Cpu` forces CPU.
/// An explicit `gpu_device` index (>= 0) pins that exact registered device,
/// otherwise the backend auto-selects. transcribe-cpp has no process-global
/// accelerator switch, so this is resolved at each model load.
fn resolve_transcribe_backend(
    accelerator: WhisperAcceleratorSetting,
    gpu_device: i32,
) -> (Backend, Option<Device>) {
    if accelerator == WhisperAcceleratorSetting::Cpu {
        return (Backend::Cpu, None);
    }
    if gpu_device >= 0 {
        let wanted = gpu_device as usize;
        if let Some(device) = transcribe_cpp::devices()
            .into_iter()
            .find(|d| d.index == Some(wanted))
        {
            info!(
                "Using user-selected compute device {} ({})",
                gpu_device,
                describe_transcribe_device(&device)
            );
            return (Backend::Auto, Some(device));
        }
        warn!(
            "GPU device index {} not found, falling back to automatic selection",
            gpu_device
        );
    }
    (Backend::Auto, None)
}

fn describe_transcribe_device(device: &Device) -> String {
    if device.description.is_empty() {
        device.name.clone()
    } else {
        device.description.clone()
    }
}

impl TranscribeCppEngine {
    fn load(
        model_path: &Path,
        accelerator: WhisperAcceleratorSetting,
        gpu_device: i32,
    ) -> Result<Self> {
        if !model_path.exists() {
            return Err(anyhow::anyhow!(
                "Whisper model not found: {}",
                model_path.display()
            ));
        }

        ensure_transcribe_backends();

        let (backend, device) = resolve_transcribe_backend(accelerator, gpu_device);
        let model = Model::load_with(model_path, &ModelOptions { backend, device }).map_err(|e| {
            anyhow::anyhow!("Failed to initialize Whisper context: {}", e)
        })?;
        let bound_backend = model.backend();
        let bound_device = model
            .device()
            .map(|d| describe_transcribe_device(&d))
            .unwrap_or_else(|_| "unknown".to_string());
        let arch = model.arch();
        info!(
            "Loaded whisper model (backend '{}', device '{}', arch '{}')",
            bound_backend, bound_device, arch
        );

        let mut session = model
            .session()
            .map_err(|e| anyhow::anyhow!("Failed to initialize Whisper state: {}", e))?;
        let cancel_token = CancelToken::new();
        session.set_cancel_token(&cancel_token);

        Ok(Self {
            session,
            cancel_token,
            arch,
        })
    }

    /// Whether this model accepts the whisper run extension (decode prompt +
    /// context knobs). Always true for the whisper family, including legacy
    /// `.bin` files; anything else falls back to the fuzzy post-correction
    /// path for custom words.
    fn takes_initial_prompt(&self) -> bool {
        self.arch.as_str() == "whisper" || self.arch.is_empty()
    }

    fn was_aborted(&self) -> bool {
        self.session.was_aborted()
    }

    fn transcribe(
        &mut self,
        samples: &[f32],
        language: Option<String>,
        translate: bool,
        initial_prompt: Option<String>,
        no_context: bool,
        progress_callback: Option<TranscriptionProgressCallback>,
    ) -> Result<TranscriptionResult> {
        if let Some(ref callback) = progress_callback {
            callback(TranscriptionProgress {
                text: None,
                progress: Some(0),
            });
        }

        // Mirror the old whisper `translate` flag: any non-English source with
        // translation requested becomes an English-translation run.
        let (task, target_language) =
            if translate && language.as_deref() != Some("en") {
                (Task::Translate, Some("en".to_string()))
            } else {
                (Task::Transcribe, None)
            };

        // The whisper run extension carries both the custom-words decode
        // prompt and the anti-hallucination context budget (previously
        // `set_no_context(true)` + `set_n_max_text_ctx(64)` on whisper-rs):
        // never condition on previous-window tokens and cap the rolling
        // context at ~1 sentence so one window's hallucination cannot
        // propagate into the next.
        let family = if self.takes_initial_prompt() {
            Some(RunExtension::Whisper(WhisperRunOptions {
                initial_prompt,
                condition_on_prev_tokens: Some(!no_context),
                max_prev_context_tokens: Some(64),
                ..Default::default()
            }))
        } else {
            None
        };

        let run_options = RunOptions {
            task,
            language,
            target_language,
            timestamps: CppTimestampKind::Segment,
            family,
            ..Default::default()
        };

        // transcribe-cpp runs expose no in-flight progress signal (unlike the
        // old whisper-rs callbacks), so interpolate one from wall-clock time
        // while the synchronous run below blocks. Without this, file
        // transcription progress would jump backwards to the chunk start and
        // then forwards to the chunk end on every chunk. The estimate assumes
        // a conservative 3x realtime factor, is capped at 95% so completion
        // to 100% is always visible, and is strictly monotonic within a run.
        let ticker_done = Arc::new(AtomicBool::new(false));
        let ticker_handle = progress_callback.clone().map(|callback| {
            let done = Arc::clone(&ticker_done);
            let est_secs = (samples.len() as f64
                / crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE as f64
                / 3.0)
                .max(0.5);
            let started = std::time::Instant::now();
            std::thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                    if done.load(Ordering::Relaxed) {
                        break;
                    }
                    let pct =
                        (started.elapsed().as_secs_f64() / est_secs * 100.0).min(95.0) as i32;
                    callback(TranscriptionProgress {
                        text: None,
                        progress: Some(pct.max(0)),
                    });
                    if pct >= 95 {
                        // Hold at the cap until the run finishes instead of
                        // churning the UI with identical values.
                        std::thread::sleep(Duration::from_millis(400));
                    }
                }
            })
        });

        let result = self.session.run(samples, &run_options);
        ticker_done.store(true, Ordering::Relaxed);
        if let Some(handle) = ticker_handle {
            let _ = handle.join();
        }
        // A cancelled run must not poison the next one: the flag is
        // edge-triggered per run, so always clear it once the run ends.
        self.cancel_token.reset();
        // A cancelled run must not poison the next one: the flag is
        // edge-triggered per run, so always clear it once the run ends.
        self.cancel_token.reset();

        let transcript = result.map_err(|e| anyhow::anyhow!("Whisper inference failed: {}", e))?;

        if let Some(ref callback) = progress_callback {
            callback(TranscriptionProgress {
                text: None,
                progress: Some(100),
            });
        }

        let mut segments = Vec::with_capacity(transcript.segments.len());
        let mut full_text = String::new();
        for s in &transcript.segments {
            let trimmed = s.text.trim();
            if trimmed.is_empty() {
                continue;
            }
            segments.push(TranscriptionSegment {
                start: s.t0_ms as f32 / 1000.0,
                end: s.t1_ms as f32 / 1000.0,
                text: trimmed.to_string(),
            });
            if !full_text.is_empty() {
                full_text.push('\n');
            }
            full_text.push_str(trimmed);
        }

        Ok(TranscriptionResult {
            text: if segments.is_empty() {
                transcript.text.trim().to_string()
            } else {
                full_text
            },
            segments: Some(segments),
        })
    }
}

enum LoadedEngine {
    Whisper(TranscribeCppEngine),
    Parakeet(ParakeetModel),
    Moonshine(MoonshineModel),
    MoonshineStreaming(StreamingModel),
    SenseVoice(SenseVoiceModel),
    GigaAM(GigaAMModel),
    Canary(CanaryModel),
    Cohere(CohereModel),
}

/// RAII guard that clears the `is_loading` flag and notifies waiters on drop.
/// Ensures the loading flag is always reset, even on early returns or panics.
pub struct LoadingGuard {
    is_loading: Arc<Mutex<bool>>,
    loading_condvar: Arc<Condvar>,
}

impl Drop for LoadingGuard {
    fn drop(&mut self) {
        let mut is_loading = self.is_loading.lock().unwrap();
        *is_loading = false;
        self.loading_condvar.notify_all();
    }
}

#[derive(Clone)]
pub struct TranscriptionManager {
    engine: Arc<Mutex<Option<LoadedEngine>>>,
    model_manager: Arc<ModelManager>,
    app_handle: AppHandle,
    current_model_id: Arc<Mutex<Option<String>>>,
    last_activity: Arc<AtomicU64>,
    shutdown_signal: Arc<AtomicBool>,
    watcher_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    is_loading: Arc<Mutex<bool>>,
    loading_condvar: Arc<Condvar>,
    file_transcription_cancelled: Arc<AtomicBool>,
    /// Clone of the cancel token installed on the loaded whisper session.
    /// Kept outside the engine mutex so `cancel_file_transcription` can abort
    /// a mid-chunk run while the engine is checked out for transcription.
    transcribe_cancel_token: Arc<Mutex<Option<CancelToken>>>,
}

impl TranscriptionManager {
    pub fn new(app_handle: &AppHandle, model_manager: Arc<ModelManager>) -> Result<Self> {
        let manager = Self {
            engine: Arc::new(Mutex::new(None)),
            model_manager,
            app_handle: app_handle.clone(),
            current_model_id: Arc::new(Mutex::new(None)),
            last_activity: Arc::new(AtomicU64::new(Self::now_ms())),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            watcher_handle: Arc::new(Mutex::new(None)),
            is_loading: Arc::new(Mutex::new(false)),
            loading_condvar: Arc::new(Condvar::new()),
            file_transcription_cancelled: Arc::new(AtomicBool::new(false)),
            transcribe_cancel_token: Arc::new(Mutex::new(None)),
        };

        // Start the idle watcher
        {
            let app_handle_cloned = app_handle.clone();
            let manager_cloned = manager.clone();
            let shutdown_signal = manager.shutdown_signal.clone();
            let handle = thread::spawn(move || {
                debug!("Idle watcher thread started");
                while !shutdown_signal.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_secs(10)); // Check every 10 seconds

                    // Check shutdown signal again after sleep
                    if shutdown_signal.load(Ordering::Relaxed) {
                        break;
                    }

                    let settings = get_settings(&app_handle_cloned);
                    let timeout = settings.model_unload_timeout;

                    // Skip Immediately — that variant is handled by
                    // maybe_unload_immediately() after each transcription.
                    // Treating it as 0s here would unload the model mid-recording.
                    if timeout == ModelUnloadTimeout::Immediately {
                        continue;
                    }

                    // While recording, keep the idle timer fresh so the
                    // model is never unloaded mid-session.
                    let is_recording = app_handle_cloned
                        .try_state::<Arc<AudioRecordingManager>>()
                        .map_or(false, |a| a.is_recording());
                    if is_recording {
                        manager_cloned.touch_activity();
                        continue;
                    }

                    if let Some(limit_seconds) = timeout.to_seconds() {
                        let last = manager_cloned.last_activity.load(Ordering::Relaxed);
                        let now_ms = TranscriptionManager::now_ms();
                        let idle_ms = now_ms.saturating_sub(last);
                        let limit_ms = limit_seconds * 1000;

                        if idle_ms > limit_ms {
                            // idle -> unload
                            if manager_cloned.is_model_loaded() {
                                let unload_start = std::time::Instant::now();
                                info!(
                                    "Model idle for {}s (limit: {}s), unloading",
                                    idle_ms / 1000,
                                    limit_seconds
                                );
                                match manager_cloned.unload_model() {
                                    Ok(()) => {
                                        let unload_duration = unload_start.elapsed();
                                        info!(
                                            "Model unloaded due to inactivity (took {}ms)",
                                            unload_duration.as_millis()
                                        );
                                    }
                                    Err(e) => {
                                        error!("Failed to unload idle model: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
                debug!("Idle watcher thread shutting down gracefully");
            });
            *manager.watcher_handle.lock().unwrap() = Some(handle);
        }

        Ok(manager)
    }

    /// Lock the engine mutex, recovering from poison if a previous transcription panicked.
    fn lock_engine(&self) -> MutexGuard<'_, Option<LoadedEngine>> {
        self.engine.lock().unwrap_or_else(|poisoned| {
            warn!("Engine mutex was poisoned by a previous panic, recovering");
            poisoned.into_inner()
        })
    }

    pub fn is_model_loaded(&self) -> bool {
        let engine = self.lock_engine();
        engine.is_some()
    }

    /// Atomically check whether a model load is in progress and, if not, mark
    /// one as starting. Returns a [`LoadingGuard`] whose [`Drop`] impl will
    /// clear the flag and wake waiters. Returns `None` if a load is already in
    /// progress.
    pub fn try_start_loading(&self) -> Option<LoadingGuard> {
        let mut is_loading = self.is_loading.lock().unwrap();
        if *is_loading {
            return None;
        }
        *is_loading = true;
        Some(LoadingGuard {
            is_loading: self.is_loading.clone(),
            loading_condvar: self.loading_condvar.clone(),
        })
    }

    pub fn unload_model(&self) -> Result<()> {
        let unload_start = std::time::Instant::now();
        debug!("Starting to unload model");

        {
            let mut engine = self.lock_engine();
            // Dropping the engine frees all resources
            *engine = None;
        }
        // The installed cancel token dies with the session; drop our clone too
        // so a later load starts from a fresh, un-cancelled token.
        *self.transcribe_cancel_token.lock().unwrap() = None;
        {
            let mut current_model = self.current_model_id.lock().unwrap();
            *current_model = None;
        }

        // Emit unloaded event
        let _ = self.app_handle.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "unloaded".to_string(),
                model_id: None,
                model_name: None,
                error: None,
            },
        );

        let unload_duration = unload_start.elapsed();
        debug!(
            "Model unloaded manually (took {}ms)",
            unload_duration.as_millis()
        );
        Ok(())
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    /// Reset the idle timer to now.
    fn touch_activity(&self) {
        self.last_activity.store(Self::now_ms(), Ordering::Relaxed);
    }

    /// Unloads the model immediately if the setting is enabled and the model is loaded
    pub fn maybe_unload_immediately(&self, context: &str) {
        let settings = get_settings(&self.app_handle);
        if settings.model_unload_timeout == ModelUnloadTimeout::Immediately
            && self.is_model_loaded()
        {
            info!("Immediately unloading model after {}", context);
            if let Err(e) = self.unload_model() {
                warn!("Failed to immediately unload model: {}", e);
            }
        }
    }

    pub fn load_model(&self, model_id: &str) -> Result<()> {
        let load_start = std::time::Instant::now();
        debug!("Starting to load model: {}", model_id);

        // Emit loading started event
        let _ = self.app_handle.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "loading_started".to_string(),
                model_id: Some(model_id.to_string()),
                model_name: None,
                error: None,
            },
        );

        let model_info = self
            .model_manager
            .get_model_info(model_id)
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

        if !model_info.is_downloaded {
            let error_msg = "Model not downloaded";
            let _ = self.app_handle.emit(
                "model-state-changed",
                ModelStateEvent {
                    event_type: "loading_failed".to_string(),
                    model_id: Some(model_id.to_string()),
                    model_name: Some(model_info.name.clone()),
                    error: Some(error_msg.to_string()),
                },
            );
            return Err(anyhow::anyhow!(error_msg));
        }

        let model_path = self.model_manager.get_model_path(model_id)?;

        // Create appropriate engine based on model type
        let emit_loading_failed = |error_msg: &str| {
            let _ = self.app_handle.emit(
                "model-state-changed",
                ModelStateEvent {
                    event_type: "loading_failed".to_string(),
                    model_id: Some(model_id.to_string()),
                    model_name: Some(model_info.name.clone()),
                    error: Some(error_msg.to_string()),
                },
            );
        };

        let loaded_engine = match model_info.engine_type {
            EngineType::Whisper => {
                let settings = get_settings(&self.app_handle);
                let engine = TranscribeCppEngine::load(
                    &model_path,
                    settings.whisper_accelerator,
                    settings.whisper_gpu_device,
                )
                .map_err(|e| {
                    let error_msg = format!("Failed to load whisper model {}: {}", model_id, e);
                    emit_loading_failed(&error_msg);
                    anyhow::anyhow!(error_msg)
                })?;
                *self.transcribe_cancel_token.lock().unwrap() =
                    Some(engine.cancel_token.clone());
                LoadedEngine::Whisper(engine)
            }
            EngineType::Parakeet => {
                let engine =
                    ParakeetModel::load(&model_path, &Quantization::Int8).map_err(|e| {
                        let error_msg =
                            format!("Failed to load parakeet model {}: {}", model_id, e);
                        emit_loading_failed(&error_msg);
                        anyhow::anyhow!(error_msg)
                    })?;
                LoadedEngine::Parakeet(engine)
            }
            EngineType::Moonshine => {
                let engine = MoonshineModel::load(
                    &model_path,
                    MoonshineVariant::Base,
                    &Quantization::default(),
                )
                .map_err(|e| {
                    let error_msg = format!("Failed to load moonshine model {}: {}", model_id, e);
                    emit_loading_failed(&error_msg);
                    anyhow::anyhow!(error_msg)
                })?;
                LoadedEngine::Moonshine(engine)
            }
            EngineType::MoonshineStreaming => {
                let engine = StreamingModel::load(&model_path, 0, &Quantization::default())
                    .map_err(|e| {
                        let error_msg = format!(
                            "Failed to load moonshine streaming model {}: {}",
                            model_id, e
                        );
                        emit_loading_failed(&error_msg);
                        anyhow::anyhow!(error_msg)
                    })?;
                LoadedEngine::MoonshineStreaming(engine)
            }
            EngineType::SenseVoice => {
                let engine =
                    SenseVoiceModel::load(&model_path, &Quantization::Int8).map_err(|e| {
                        let error_msg =
                            format!("Failed to load SenseVoice model {}: {}", model_id, e);
                        emit_loading_failed(&error_msg);
                        anyhow::anyhow!(error_msg)
                    })?;
                LoadedEngine::SenseVoice(engine)
            }
            EngineType::GigaAM => {
                let engine = GigaAMModel::load(&model_path, &Quantization::Int8).map_err(|e| {
                    let error_msg = format!("Failed to load gigaam model {}: {}", model_id, e);
                    emit_loading_failed(&error_msg);
                    anyhow::anyhow!(error_msg)
                })?;
                LoadedEngine::GigaAM(engine)
            }
            EngineType::Canary => {
                let engine = CanaryModel::load(&model_path, &Quantization::Int8).map_err(|e| {
                    let error_msg = format!("Failed to load canary model {}: {}", model_id, e);
                    emit_loading_failed(&error_msg);
                    anyhow::anyhow!(error_msg)
                })?;
                LoadedEngine::Canary(engine)
            }
            EngineType::Cohere => {
                let engine = CohereModel::load(&model_path, &Quantization::Int8).map_err(|e| {
                    let error_msg = format!("Failed to load cohere model {}: {}", model_id, e);
                    emit_loading_failed(&error_msg);
                    anyhow::anyhow!(error_msg)
                })?;
                LoadedEngine::Cohere(engine)
            }
        };

        // Update the current engine and model ID
        {
            let mut engine = self.lock_engine();
            *engine = Some(loaded_engine);
        }
        {
            let mut current_model = self.current_model_id.lock().unwrap();
            *current_model = Some(model_id.to_string());
        }

        // Reset idle timer so the watcher doesn't immediately unload a just-loaded model
        self.touch_activity();

        // Emit loading completed event
        let _ = self.app_handle.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "loading_completed".to_string(),
                model_id: Some(model_id.to_string()),
                model_name: Some(model_info.name.clone()),
                error: None,
            },
        );

        let load_duration = load_start.elapsed();
        debug!(
            "Successfully loaded transcription model: {} (took {}ms)",
            model_id,
            load_duration.as_millis()
        );
        Ok(())
    }

    /// Kicks off the model loading in a background thread if it's not already loaded
    pub fn initiate_model_load(&self) {
        let mut is_loading = self.is_loading.lock().unwrap();
        if *is_loading || self.is_model_loaded() {
            return;
        }

        *is_loading = true;
        let self_clone = self.clone();
        thread::spawn(move || {
            let settings = get_settings(&self_clone.app_handle);
            if let Err(e) = self_clone.load_model(&settings.selected_model) {
                error!("Failed to load model: {}", e);
            }
            let mut is_loading = self_clone.is_loading.lock().unwrap();
            *is_loading = false;
            self_clone.loading_condvar.notify_all();
        });
    }

    pub fn get_current_model(&self) -> Option<String> {
        let current_model = self.current_model_id.lock().unwrap();
        current_model.clone()
    }

    pub fn cancel_file_transcription(&self) {
        self.file_transcription_cancelled
            .store(true, Ordering::SeqCst);
        // Abort a whisper run that is already in flight; the run returns
        // `Error::Aborted` with its partial transcript.
        if let Some(token) = self.transcribe_cancel_token.lock().unwrap().as_ref() {
            token.cancel();
        }
        info!("File transcription cancellation requested");
    }

    pub fn is_file_transcription_cancelled(&self) -> bool {
        self.file_transcription_cancelled.load(Ordering::SeqCst)
    }

    pub fn reset_file_transcription_cancelled(&self) {
        self.file_transcription_cancelled
            .store(false, Ordering::SeqCst);
        if let Some(token) = self.transcribe_cancel_token.lock().unwrap().as_ref() {
            token.reset();
        }
    }

    pub fn transcribe(&self, audio: Vec<f32>) -> Result<String> {
        self.transcribe_inner(&audio, None, false, true, 0.0, TimestampMode::Plain)
    }

    /// Transcribes each detected speech segment separately and joins the
    /// results with newlines. This resets Whisper's text context between
    /// segments, which reduces hallucination carry-over across long pauses.
    ///
    /// When timestamp mode is enabled, each sentence-level segment is prefixed
    /// with its audio position as `[hh:mm:ss - hh:mm:ss]`. In `GroupTimestamp`
    /// mode consecutive segments are merged into continuous paragraph-sized
    /// blocks so the output is not fragmented.
    pub fn transcribe_segments(
        &self,
        audio: Vec<f32>,
        segments: Vec<(usize, usize)>,
    ) -> Result<String> {
        if segments.is_empty() || audio.is_empty() {
            return Ok(String::new());
        }

        let settings = get_settings(&self.app_handle);
        let timestamp_mode = settings.timestamp_mode;
        let sample_rate = crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE as f64;

        // Fall back to a single inference when there is only one speech segment
        // or when the segments cover essentially the whole buffer.
        if segments.len() == 1 {
            let (start, end) = segments[0];
            if start == 0 && end >= audio.len().saturating_sub(1) {
                return self.transcribe_inner(&audio, None, false, true, 0.0, timestamp_mode);
            }
        }

        let mut texts = Vec::with_capacity(segments.len());
        let last_idx = segments.len().saturating_sub(1);
        for (i, (start, end)) in segments.into_iter().enumerate() {
            let end = end.min(audio.len());
            if start >= end {
                continue;
            }
            let segment_audio = &audio[start..end];
            // Keep the model loaded across segment inferences; only allow the
            // final segment to trigger immediate unload if configured.
            let skip_unload = i != last_idx;
            let base_offset = start as f64 / sample_rate;
            match self.transcribe_inner(
                segment_audio,
                None,
                skip_unload,
                true,
                base_offset,
                timestamp_mode,
            ) {
                Ok(text) if !text.is_empty() => texts.push(text),
                Ok(_) => {}
                Err(e) => {
                    warn!("Failed to transcribe speech segment: {}", e);
                }
            }
        }

        Ok(texts.join("\n\n"))
    }

    #[allow(dead_code)]
    pub fn transcribe_with_progress(
        &self,
        audio: Vec<f32>,
        progress_callback: Option<TranscriptionProgressCallback>,
    ) -> Result<String> {
        self.transcribe_inner(
            &audio,
            progress_callback,
            false,
            true,
            0.0,
            TimestampMode::Plain,
        )
    }

    pub fn transcribe_chunk_with_progress(
        &self,
        audio: &[f32],
        progress_callback: Option<TranscriptionProgressCallback>,
        skip_immediate_unload: bool,
        no_context: bool,
        base_offset_seconds: f64,
        timestamp_mode: TimestampMode,
    ) -> Result<String> {
        self.transcribe_inner(
            audio,
            progress_callback,
            skip_immediate_unload,
            no_context,
            base_offset_seconds,
            timestamp_mode,
        )
    }

    fn transcribe_inner(
        &self,
        audio: &[f32],
        progress_callback: Option<TranscriptionProgressCallback>,
        skip_immediate_unload: bool,
        no_context: bool,
        base_offset_seconds: f64,
        timestamp_mode: TimestampMode,
    ) -> Result<String> {
        #[cfg(debug_assertions)]
        if std::env::var("HANHCUTE_FORCE_TRANSCRIPTION_FAILURE").is_ok() {
            return Err(anyhow::anyhow!(
                "Simulated transcription failure (HANHCUTE_FORCE_TRANSCRIPTION_FAILURE)"
            ));
        }

        // Update last activity timestamp
        self.touch_activity();

        let st = std::time::Instant::now();

        debug!("Audio vector length: {}", audio.len());

        if audio.is_empty() {
            debug!("Empty audio vector");
            self.maybe_unload_immediately("empty audio");
            return Ok(String::new());
        }

        // Check if model is loaded, if not try to load it
        {
            // If the model is loading, wait for it to complete.
            let mut is_loading = self.is_loading.lock().unwrap();
            while *is_loading {
                is_loading = self.loading_condvar.wait(is_loading).unwrap();
            }

            let engine_guard = self.lock_engine();
            if engine_guard.is_none() {
                return Err(anyhow::anyhow!("Model is not loaded for transcription."));
            }
        }

        // Get current settings for configuration
        let settings = get_settings(&self.app_handle);

        // Validate selected language against the model's supported languages.
        // If the language isn't supported, fall back to "auto" to prevent errors.
        let validated_language = if settings.selected_language == "auto" {
            "auto".to_string()
        } else {
            let is_supported = self
                .model_manager
                .get_model_info(&settings.selected_model)
                .map(|info| {
                    info.supported_languages.is_empty()
                        || info
                            .supported_languages
                            .contains(&settings.selected_language)
                })
                .unwrap_or(true);

            if is_supported {
                settings.selected_language.clone()
            } else {
                warn!(
                    "Language '{}' not supported by current model, falling back to auto-detect",
                    settings.selected_language
                );
                "auto".to_string()
            }
        };

        // Perform transcription with the appropriate engine.
        // We use catch_unwind to prevent engine panics from poisoning the mutex,
        // which would make the app hang indefinitely on subsequent operations.
        let result = {
            let mut engine_guard = self.lock_engine();

            // Take the engine out so we own it during transcription.
            // If the engine panics, we simply don't put it back (effectively unloading it)
            // instead of poisoning the mutex.
            let mut engine = match engine_guard.take() {
                Some(e) => e,
                None => {
                    return Err(anyhow::anyhow!(
                        "Model failed to load after auto-load attempt. Please check your model settings."
                    ));
                }
            };

            // Release the lock before transcribing — no mutex held during the engine call
            drop(engine_guard);

            let transcribe_result = catch_unwind(AssertUnwindSafe(
                || -> Result<transcribe_rs::TranscriptionResult> {
                    match &mut engine {
                        LoadedEngine::Whisper(whisper_engine) => {
                            let whisper_language = if validated_language == "auto" {
                                None
                            } else {
                                let normalized = if validated_language == "zh-Hans"
                                    || validated_language == "zh-Hant"
                                {
                                    "zh".to_string()
                                } else {
                                    validated_language.clone()
                                };
                                Some(normalized)
                            };

                            let initial_prompt = if settings.custom_words.is_empty() {
                                None
                            } else {
                                Some(settings.custom_words.join(", "))
                            };

                            match whisper_engine.transcribe(
                                audio,
                                whisper_language,
                                settings.translate_to_english,
                                initial_prompt,
                                no_context,
                                progress_callback.clone(),
                            ) {
                                Ok(result) => Ok(result),
                                // A user-cancelled file run surfaces as the
                                // plain "Cancelled" marker so the file
                                // pipeline takes its cancelled path instead
                                // of reporting a transcription error.
                                Err(_)
                                    if whisper_engine.was_aborted()
                                        && self.is_file_transcription_cancelled() =>
                                {
                                    Err(anyhow::anyhow!("Cancelled"))
                                }
                                Err(e) => Err(anyhow::anyhow!(
                                    "Whisper transcription failed: {}",
                                    e
                                )),
                            }
                        }
                        LoadedEngine::Parakeet(parakeet_engine) => {
                            let params = ParakeetParams {
                                timestamp_granularity: Some(TimestampGranularity::Segment),
                                ..Default::default()
                            };
                            parakeet_engine
                                .transcribe_with(audio, &params)
                                .map_err(|e| {
                                    anyhow::anyhow!("Parakeet transcription failed: {}", e)
                                })
                        }
                        LoadedEngine::Moonshine(moonshine_engine) => moonshine_engine
                            .transcribe(audio, &TranscribeOptions::default())
                            .map_err(|e| anyhow::anyhow!("Moonshine transcription failed: {}", e)),
                        LoadedEngine::MoonshineStreaming(streaming_engine) => streaming_engine
                            .transcribe(audio, &TranscribeOptions::default())
                            .map_err(|e| {
                                anyhow::anyhow!("Moonshine streaming transcription failed: {}", e)
                            }),
                        LoadedEngine::SenseVoice(sense_voice_engine) => {
                            let language = match validated_language.as_str() {
                                "zh" | "zh-Hans" | "zh-Hant" => Some("zh".to_string()),
                                "en" => Some("en".to_string()),
                                "ja" => Some("ja".to_string()),
                                "ko" => Some("ko".to_string()),
                                "yue" => Some("yue".to_string()),
                                _ => None,
                            };
                            let params = SenseVoiceParams {
                                language,
                                use_itn: Some(true),
                            };
                            sense_voice_engine
                                .transcribe_with(audio, &params)
                                .map_err(|e| {
                                    anyhow::anyhow!("SenseVoice transcription failed: {}", e)
                                })
                        }
                        LoadedEngine::GigaAM(gigaam_engine) => gigaam_engine
                            .transcribe(audio, &TranscribeOptions::default())
                            .map_err(|e| anyhow::anyhow!("GigaAM transcription failed: {}", e)),
                        LoadedEngine::Canary(canary_engine) => {
                            let lang = if validated_language == "auto" {
                                None
                            } else {
                                Some(validated_language.clone())
                            };
                            let options = TranscribeOptions {
                                language: lang,
                                translate: settings.translate_to_english,
                                ..Default::default()
                            };
                            canary_engine
                                .transcribe(audio, &options)
                                .map_err(|e| anyhow::anyhow!("Canary transcription failed: {}", e))
                        }
                        LoadedEngine::Cohere(cohere_engine) => {
                            let lang = if validated_language == "auto" {
                                None
                            } else if validated_language == "zh-Hans"
                                || validated_language == "zh-Hant"
                            {
                                Some("zh".to_string())
                            } else {
                                Some(validated_language.clone())
                            };
                            let options = TranscribeOptions {
                                language: lang,
                                ..Default::default()
                            };
                            cohere_engine
                                .transcribe(audio, &options)
                                .map_err(|e| anyhow::anyhow!("Cohere transcription failed: {}", e))
                        }
                    }
                },
            ));

            match transcribe_result {
                Ok(inner_result) => {
                    // Success or normal error — put the engine back
                    let mut engine_guard = self.lock_engine();
                    *engine_guard = Some(engine);
                    inner_result?
                }
                Err(panic_payload) => {
                    // Engine panicked — do NOT put it back (it's in an unknown state).
                    // The engine is dropped here, effectively unloading it.
                    let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    error!(
                        "Transcription engine panicked: {}. Model has been unloaded.",
                        panic_msg
                    );

                    // Clear the model ID so it will be reloaded on next attempt
                    {
                        let mut current_model = self
                            .current_model_id
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        *current_model = None;
                    }

                    let _ = self.app_handle.emit(
                        "model-state-changed",
                        ModelStateEvent {
                            event_type: "unloaded".to_string(),
                            model_id: None,
                            model_name: None,
                            error: Some(format!("Engine panicked: {}", panic_msg)),
                        },
                    );

                    return Err(anyhow::anyhow!(
                        "Transcription engine panicked: {}. The model has been unloaded and will reload on next attempt.",
                        panic_msg
                    ));
                }
            }
        };

        // Reconstruct text from segment-level data for better sentence line breaks.
        // When a timestamp mode is on, use each segment's engine-local start/end
        // plus the audio offset of the current chunk.
        let segments_text = match timestamp_mode {
            TimestampMode::Timestamp => result.segments.as_ref().and_then(|segs| {
                if segs.is_empty() {
                    return None;
                }
                let text = segs
                    .iter()
                    .map(|s| {
                        let text = s.text.trim();
                        if text.is_empty() {
                            return String::new();
                        }
                        let begin = base_offset_seconds + s.start as f64;
                        let end = base_offset_seconds + s.end as f64;
                        format!("{} {}", format_timestamp_range(begin, end), text)
                    })
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            }),
            TimestampMode::GroupTimestamp => result.segments.as_ref().and_then(|segs| {
                if segs.is_empty() {
                    return None;
                }
                let text = group_segments_with_timestamps(segs, base_offset_seconds);
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            }),
            TimestampMode::Plain => result.segments.as_ref().map(|segs| {
                if segs.is_empty() {
                    String::new()
                } else {
                    segs.iter()
                        .map(|s| s.text.trim())
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            }),
        };

        let timestamps_enabled = timestamp_mode != TimestampMode::Plain;
        let raw_text = match segments_text {
            Some(t) if !t.is_empty() => t,
            _ if timestamps_enabled => {
                // Fallback when the engine did not return per-segment timestamps:
                // stamp the whole chunk with its audio range.
                let duration = audio.len() as f64
                    / crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE as f64;
                let text = result.text.trim();
                if text.is_empty() {
                    String::new()
                } else {
                    format!(
                        "{} {}",
                        format_timestamp_range(base_offset_seconds, base_offset_seconds + duration),
                        text
                    )
                }
            }
            _ => result.text,
        };

        // Apply word correction if custom words are configured.
        // Skip for Whisper models whose run actually carried the decode prompt
        // (the extension is rejected on non-whisper architectures, in which
        // case the fuzzy correction below still applies).
        let custom_words_prompted = self
            .model_manager
            .get_model_info(&settings.selected_model)
            .map(|info| matches!(info.engine_type, EngineType::Whisper))
            .unwrap_or(false)
            && self
                .lock_engine()
                .as_ref()
                .map(|engine| match engine {
                    LoadedEngine::Whisper(w) => w.takes_initial_prompt(),
                    _ => false,
                })
                .unwrap_or(false);

        let corrected_result = if !settings.custom_words.is_empty() && !custom_words_prompted {
            apply_custom_words(
                &raw_text,
                &settings.custom_words,
                settings.word_correction_threshold,
            )
        } else {
            raw_text
        };

        // Filter out filler words and hallucinations
        let filtered_result = filter_transcription_output(
            &corrected_result,
            &settings.app_language,
            &settings.custom_filler_words,
        );

        let et = std::time::Instant::now();
        let translation_note = if settings.translate_to_english {
            " (translated)"
        } else {
            ""
        };
        info!(
            "Transcription completed in {}ms{}",
            (et - st).as_millis(),
            translation_note
        );

        let final_result = filtered_result;

        if final_result.is_empty() {
            info!("Transcription result is empty");
        } else {
            info!("Transcription result: {}", final_result);
        }

        if !skip_immediate_unload {
            self.maybe_unload_immediately("transcription");
        }

        Ok(final_result)
    }
}

/// Apply the user's accelerator preferences.
/// Called on startup and whenever the user changes the setting.
///
/// transcribe-cpp has no process-global accelerator switch: the whisper
/// backend is chosen per model load from `whisper_accelerator` /
/// `whisper_gpu_device` (see `TranscribeCppEngine::load`), so a change takes
/// effect on the next load. This only ensures native logging + backend
/// registration happened, then applies the ORT (ONNX) preference as before.
pub fn apply_accelerator_settings(app: &tauri::AppHandle) {
    use transcribe_rs::accel;

    let settings = get_settings(app);

    ensure_transcribe_backends();
    info!(
        "Whisper accelerator preference: {:?}, gpu_device: {} (applied on next model load)",
        settings.whisper_accelerator,
        if settings.whisper_gpu_device < 0 {
            "auto".to_string()
        } else {
            settings.whisper_gpu_device.to_string()
        }
    );

    let ort_pref = match settings.ort_accelerator {
        OrtAcceleratorSetting::Auto => accel::OrtAccelerator::Auto,
        OrtAcceleratorSetting::Cpu => accel::OrtAccelerator::CpuOnly,
        OrtAcceleratorSetting::Cuda => accel::OrtAccelerator::Cuda,
        OrtAcceleratorSetting::DirectMl => accel::OrtAccelerator::DirectMl,
        OrtAcceleratorSetting::Rocm => accel::OrtAccelerator::Rocm,
    };
    accel::set_ort_accelerator(ort_pref);
    info!("ORT accelerator set to: {}", ort_pref);
}

#[derive(Serialize, Clone, Debug, Type)]
pub struct GpuDeviceOption {
    pub id: i32,
    pub name: String,
    pub total_vram_mb: usize,
}

static GPU_DEVICES: OnceLock<Vec<GpuDeviceOption>> = OnceLock::new();

fn cached_gpu_devices() -> &'static [GpuDeviceOption] {
    GPU_DEVICES.get_or_init(|| {
        // ggml's Vulkan backend uses FMA3 instructions internally.
        // On older CPUs without FMA3 (e.g. Sandy Bridge Xeons) this causes
        // a SIGILL crash that cannot be caught. Skip enumeration entirely
        // on those CPUs — GPU-accelerated inference won't work there anyway.
        #[cfg(target_arch = "x86_64")]
        if !std::arch::is_x86_feature_detected!("fma") {
            warn!("CPU lacks FMA3 support — skipping GPU device enumeration");
            return Vec::new();
        }

        ensure_transcribe_backends();
        transcribe_cpp::devices()
            .into_iter()
            .enumerate()
            .filter(|(_, d)| {
                matches!(
                    d.device_type,
                    DeviceType::Gpu | DeviceType::Igpu
                )
            })
            .map(|(pos, d)| GpuDeviceOption {
                id: d.index.unwrap_or(pos) as i32,
                name: describe_transcribe_device(&d),
                total_vram_mb: (d.memory_total / (1024 * 1024)) as usize,
            })
            .collect()
    })
}

#[derive(Serialize, Clone, Debug, Type)]
pub struct AvailableAccelerators {
    pub whisper: Vec<String>,
    pub ort: Vec<String>,
    pub gpu_devices: Vec<GpuDeviceOption>,
}

/// Return which accelerators are compiled into this build.
pub fn get_available_accelerators() -> AvailableAccelerators {
    use transcribe_rs::accel::OrtAccelerator;

    let ort_options: Vec<String> = OrtAccelerator::available()
        .into_iter()
        .map(|a| a.to_string())
        .collect();

    let whisper_options = vec!["auto".to_string(), "cpu".to_string(), "gpu".to_string()];

    AvailableAccelerators {
        whisper: whisper_options,
        ort: ort_options,
        gpu_devices: cached_gpu_devices().to_vec(),
    }
}

impl Drop for TranscriptionManager {
    fn drop(&mut self) {
        // Skip shutdown unless this is the very last clone. TranscriptionManager
        // is cloned by initiate_model_load() and the watcher thread — those
        // clones dropping must not kill the watcher. The watcher thread holds
        // its own clone, so engine's strong_count is always >= 2 while the
        // watcher is alive. When it reaches 1, only this instance remains
        // and we can safely shut down.
        if Arc::strong_count(&self.engine) > 1 {
            return;
        }

        // Signal the watcher thread to shutdown
        self.shutdown_signal.store(true, Ordering::Relaxed);

        // Wait for the thread to finish gracefully
        if let Some(handle) = self.watcher_handle.lock().unwrap().take() {
            if let Err(e) = handle.join() {
                warn!("Failed to join idle watcher thread: {:?}", e);
            } else {
                debug!("Idle watcher thread joined successfully");
            }
        }
    }
}

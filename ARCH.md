# HanhCute Architecture

This document is a concise map of the codebase for AI agents. It describes what every significant source/config file does and how the pieces fit together.

> **Product:** HanhCute (formerly Handy) — a cross-platform desktop speech-to-text app.
> **Stack:** Tauri 2.x (Rust backend + React/TypeScript frontend), Whisper/Parakeet/Moonshine ASR, Silero VAD.
> **Version:** 0.8.3

---

## 1. High-Level Architecture

```
Frontend (React + Vite + Tailwind)
  ├── Main settings window  (App.tsx)
  ├── Recording overlay     (src/overlay/)
  └── State stores/hooks    (Zustand)
            ↕ Tauri commands / events (tauri-specta)
Backend (Rust)
  ├── Commands              (src-tauri/src/commands/)   ← exposed to frontend
  ├── Managers              (src-tauri/src/managers/)   ← core business logic
  ├── Audio toolkit         (src-tauri/src/audio_toolkit/)
  ├── Shortcut handling     (src-tauri/src/shortcut/)
  └── Supporting modules    (settings, tray, overlay, clipboard, input, ...)
```

**Data flow:**

1. User presses global shortcut (or CLI/signal).
2. Shortcut handler → `TranscriptionCoordinator` serializes the request.
3. `AudioRecordingManager` opens microphone stream + VAD.
4. Audio saved → `TranscriptionManager` loads/runs ASR model.
5. Text post-processed (optional LLM) → clipboard/paste/input simulation.
6. Result stored in SQLite history; events update UI/overlay/tray.

---

## 2. Root Configuration Files

| File                                   | Purpose                                                                                                                                                                    |
| -------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `package.json`                         | Bun/npm manifest, scripts (`dev`, `build`, `tauri`, `lint`, `format`, tests), frontend dependencies.                                                                       |
| `vite.config.ts`                       | Vite build: React + Tailwind plugins; path aliases `@/` and `@/bindings`; multi-page build (`index.html` + `src/overlay/index.html`); dev server port 1420.                |
| `tsconfig.json` / `tsconfig.node.json` | TypeScript: strict mode, ES2020, bundler resolution, path aliases, React JSX transform.                                                                                    |
| `tailwind.config.js`                   | Theme colors reference CSS variables (`--color-text`, `--color-background`, etc.).                                                                                         |
| `eslint.config.js`                     | ESLint with `i18next/no-literal-string` rule: hardcoded JSX text is forbidden; all UI strings must come from i18n.                                                         |
| `playwright.config.ts`                 | End-to-end test configuration.                                                                                                                                             |
| `index.html`                           | Main window HTML entry point.                                                                                                                                              |
| `src-tauri/Cargo.toml`                 | Rust crate manifest, Tauri plugins, ASR dependencies (`whisper-rs`, `transcribe-rs`), platform-specific deps (Metal/Vulkan/DirectML, GTK layer shell, Apple Intelligence). |
| `src-tauri/tauri.conf.json`            | Tauri app config: windows, bundle/resources, security CSP/asset protocol, single-instance, autostart, tray.                                                                |
| `src-tauri/build.rs`                   | Build script: generates tray menu translations from `src/i18n/locales/*/translation.json`; compiles Apple Intelligence Swift bridge on macOS ARM64.                        |

---

## 3. Backend (`src-tauri/src/`)

### 3.1 Entry Points & App Lifecycle

| File                    | Module             | Purpose                                                                                                             | Key Items                                                                                                |
| ----------------------- | ------------------ | ------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| `src-tauri/src/main.rs` | `main`             | Binary entry point. Parses CLI and calls `hanhcute_app_lib::run()`.                                                 | `main()`, `CliArgs::parse()`                                                                             |
| `src-tauri/src/lib.rs`  | `hanhcute_app_lib` | Main Tauri library: plugin setup, manager initialization, tray/window creation, command/event collection, run loop. | `run(cli_args)`, `initialize_core_logic`, `show_main_window_command`, `FILE_LOG_LEVEL`, specta `Builder` |
| `src-tauri/src/cli.rs`  | `cli`              | Clap CLI arguments.                                                                                                 | `CliArgs`: `start_hidden`, `no_tray`, `toggle_transcription`, `toggle_post_process`, `cancel`, `debug`   |

### 3.2 Managers (`src-tauri/src/managers/`)

| File                             | Module                              | Purpose                                                                                                                                | Key Items                                                                                                                                                       |
| -------------------------------- | ----------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `managers/mod.rs`                | `managers`                          | Manager module exports.                                                                                                                | `audio`, `history`, `model`, `transcription`                                                                                                                    |
| `managers/audio.rs`              | `managers::audio`                   | Recording state machine, microphone stream lifecycle, device selection, mute output, VAD preloading, lazy stream close.                | `AudioRecordingManager`, `RecordingState`, `MicrophoneMode`, `set_mute`, `start_microphone_stream`, `try_start_recording`, `stop_recording`, `cancel_recording` |
| `managers/model.rs`              | `managers::model`                   | Model catalog, downloadable model metadata, download/resume/extract/verify/delete, custom model discovery.                             | `ModelManager`, `ModelInfo`, `EngineType`, `DownloadProgress`, `download_model`, `delete_model`, `switch_active_model`, `discover_custom_whisper_models`        |
| `managers/transcription.rs`      | `managers::transcription`           | Load/unload/run multiple ASR engines (Whisper/Parakeet/Moonshine/SenseVoice/GigaAM/Canary/Cohere), idle watcher, accelerator settings. | `TranscriptionManager`, `WhisperEngine`, `LoadedEngine`, `load_model`, `unload_model`, `transcribe`, `apply_accelerator_settings`                               |
| `managers/history.rs`            | `managers::history`                 | SQLite history DB with migrations, CRUD, retention cleanup, typed frontend events.                                                     | `HistoryManager`, `HistoryEntry`, `PaginatedHistory`, `HistoryUpdatePayload`, `save_entry`, `get_history_entries`, `cleanup_old_entries`                        |
| `managers/transcription_mock.rs` | `managers::transcription` (CI stub) | No-op replacement for `transcription.rs` used in CI to avoid heavy inference deps.                                                     | Stub `TranscriptionManager`                                                                                                                                     |

### 3.3 Tauri Commands (`src-tauri/src/commands/`)

| File                        | Module                    | Purpose                                                     | Key Items                                                                                                                                                                                |
| --------------------------- | ------------------------- | ----------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `commands/mod.rs`           | `commands`                | Common/general commands.                                    | `cancel_operation`, `get_app_settings`, `get_default_settings`, `set_log_level`, `open_recordings_folder`, `is_portable`, `get_app_dir_path`, `initialize_enigo`, `initialize_shortcuts` |
| `commands/audio.rs`         | `commands::audio`         | Audio device/microphone/output commands.                    | `get_available_microphones`, `set_selected_microphone`, `get_available_output_devices`, `play_test_sound`, `set_clamshell_microphone`, `is_recording`                                    |
| `commands/models.rs`        | `commands::models`        | Model listing/download/delete/active-model commands.        | `get_available_models`, `download_model`, `delete_model`, `set_active_model`, `switch_active_model`, `cancel_download`                                                                   |
| `commands/transcription.rs` | `commands::transcription` | Model unload timeout, load status, file transcription.      | `transcribe_file`, `cancel_file_transcription`, `get_model_load_status`, `unload_model_manually`, `ModelLoadStatus`, `FileTranscriptionResult`                                           |
| `commands/history.rs`       | `commands::history`       | History pagination, save toggle, retry, retention settings. | `get_history_entries`, `toggle_history_entry_saved`, `delete_history_entry`, `retry_history_entry_transcription`, `update_recording_retention_period`                                    |

### 3.4 Audio Toolkit (`src-tauri/src/audio_toolkit/`)

| File                                | Module                             | Purpose                                                                                     | Key Items                                                                                                     |
| ----------------------------------- | ---------------------------------- | ------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| `audio_toolkit/mod.rs`              | `audio_toolkit`                    | Public re-exports.                                                                          | `pub use audio::*`, `text::*`, `vad::*`, `utils::get_cpal_host`                                               |
| `audio_toolkit/audio/mod.rs`        | `audio_toolkit::audio`             | Re-exports of audio submodule.                                                              | `AudioRecorder`, `RecordingResult`, `list_input_devices`, `save_wav_file`, `MediaFileDecoder`, `StreamingVad` |
| `audio_toolkit/audio/recorder.rs`   | `audio_toolkit::audio::recorder`   | CPAL-based recorder worker thread, VAD filtering, spectrum callback, recording commands.    | `AudioRecorder`, `RecordingResult`, `Cmd`, `build_stream`, `run_consumer`                                     |
| `audio_toolkit/audio/device.rs`     | `audio_toolkit::audio::device`     | CPAL input/output device enumeration.                                                       | `CpalDeviceInfo`, `list_input_devices`, `list_output_devices`                                                 |
| `audio_toolkit/audio/resampler.rs`  | `audio_toolkit::audio::resampler`  | Frame-based resampler to 16 kHz.                                                            | `FrameResampler`                                                                                              |
| `audio_toolkit/audio/visualizer.rs` | `audio_toolkit::audio::visualizer` | FFT spectrum visualizer for UI levels.                                                      | `AudioVisualiser`                                                                                             |
| `audio_toolkit/audio/utils.rs`      | `audio_toolkit::audio::utils`      | WAV I/O, media decoding, streaming VAD, streaming WAV writer.                               | `read_wav_samples`, `read_media_file_samples`, `save_wav_file`, `StreamingVad`, `StreamingWavWriter`          |
| `audio_toolkit/vad/mod.rs`          | `audio_toolkit::vad`               | VAD trait and per-frame result enum.                                                        | `VoiceActivityDetector`, `VadFrame`                                                                           |
| `audio_toolkit/vad/silero.rs`       | `audio_toolkit::vad::silero`       | Silero VAD wrapper around `vad_rs`.                                                         | `SileroVad`                                                                                                   |
| `audio_toolkit/vad/smoothed.rs`     | `audio_toolkit::vad::smoothed`     | Smoothing wrapper with prefill, hangover, onset frames.                                     | `SmoothedVad`                                                                                                 |
| `audio_toolkit/text.rs`             | `audio_toolkit::text`              | Post-transcription text cleanup: custom words, filler words, stutter collapse, break modes. | `apply_custom_words`, `filter_transcription_output`, `apply_transcription_breaks`                             |
| `audio_toolkit/utils.rs`            | `audio_toolkit::utils`             | CPAL host selection helper.                                                                 | `get_cpal_host`                                                                                               |
| `audio_toolkit/constants.rs`        | `audio_toolkit::constants`         | Shared audio constants.                                                                     | `WHISPER_SAMPLE_RATE`                                                                                         |
| `audio_toolkit/bin/cli.rs`          | `audio_toolkit::bin::cli`          | Standalone audio recorder CLI for testing the toolkit.                                      | `RecorderMode`, `RecorderState`, interactive commands                                                         |

### 3.5 Shortcut Handling (`src-tauri/src/shortcut/`)

| File                     | Module                 | Purpose                                                                  | Key Items                                                                                                                            |
| ------------------------ | ---------------------- | ------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------ |
| `shortcut/mod.rs`        | `shortcut`             | Shortcut dispatcher, settings-update commands, implementation switching. | `init_shortcuts`, `register_shortcut`, `unregister_shortcut`, `change_binding`, `reset_binding`, `suspend_binding`, `resume_binding` |
| `shortcut/handler.rs`    | `shortcut::handler`    | Shared shortcut event routing used by both backends.                     | `handle_shortcut_event`                                                                                                              |
| `shortcut/handy_keys.rs` | `shortcut::handy_keys` | HandyKeys backend with manager thread and UI key-capture recording mode. | `HandyKeysState`, `ManagerCommand`, `FrontendKeyEvent`, `start_handy_keys_recording`                                                 |
| `shortcut/tauri_impl.rs` | `shortcut::tauri_impl` | Tauri `global-shortcut` plugin backend.                                  | `init_shortcuts`, `register_shortcut`, `validate_shortcut`                                                                           |

### 3.6 Core Pipeline & Helpers

| File                           | Module                      | Purpose                                                                                            | Key Items                                                                                                                                                                                                                          |
| ------------------------------ | --------------------------- | -------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `settings.rs`                  | `settings`                  | Settings schema, serde types, defaults, persistence via `tauri-plugin-store`.                      | `AppSettings`, `LogLevel`, `ShortcutBinding`, `PostProcessProvider`, `LLMPrompt`, `ModelUnloadTimeout`, `PasteMethod`, `ClipboardHandling`, `TranscriptionBreakMode`, `RecordingRetentionPeriod`, `get_settings`, `write_settings` |
| `transcription_coordinator.rs` | `transcription_coordinator` | Single-threaded coordinator serializing recording/transcription lifecycle.                         | `TranscriptionCoordinator`, `Command`, `Stage`, `send_input`, `notify_cancel`                                                                                                                                                      |
| `actions.rs`                   | `actions`                   | Shortcut action implementations, transcribe pipeline, post-processing, Chinese variant conversion. | `ShortcutAction`, `TranscribeAction`, `CancelAction`, `post_process_transcription`, `process_transcription_output`, `RecordingErrorEvent`                                                                                          |
| `signal_handle.rs`             | `signal_handle`             | Unix signal hooks and shared helper to trigger transcription.                                      | `send_transcription_input`, `setup_signal_handler`                                                                                                                                                                                 |
| `overlay.rs`                   | `overlay`                   | Recording overlay window creation, positioning, show/hide, mic-level forwarding.                   | `create_recording_overlay`, `show_recording_overlay`, `hide_recording_overlay`, `update_overlay_position`                                                                                                                          |
| `tray.rs`                      | `tray`                      | Tray icon, themed icons, dynamic menu, visibility.                                                 | `TrayIconState`, `AppTheme`, `change_tray_icon`, `update_tray_menu`, `copy_last_transcript`                                                                                                                                        |
| `tray_i18n.rs`                 | `tray_i18n`                 | Compile-time tray menu translations generated by `build.rs`.                                       | `TrayStrings`, `TRANSLATIONS`, `get_tray_translations`                                                                                                                                                                             |
| `input.rs`                     | `input`                     | Enigo wrapper for managed state and cursor position.                                               | `EnigoState`, `get_cursor_position`                                                                                                                                                                                                |
| `clipboard.rs`                 | `clipboard`                 | Paste text via clipboard restore, direct typing, external scripts, Linux native tools.             | `paste`, `paste_direct`, `paste_via_external_script`, `send_return_key`, `get_available_typing_tools`                                                                                                                              |
| `audio_feedback.rs`            | `audio_feedback`            | Start/stop feedback sounds with theme/output-device selection.                                     | `SoundType`, `play_feedback_sound`, `play_test_sound`                                                                                                                                                                              |
| `llm_client.rs`                | `llm_client`                | OpenAI-compatible chat completions and model listing.                                              | `send_chat_completion`, `fetch_models`, `ChatCompletionRequest`, `ReasoningConfig`                                                                                                                                                 |
| `apple_intelligence.rs`        | `apple_intelligence`        | macOS ARM64 FFI to Apple Intelligence for local LLM post-processing.                               | `check_apple_intelligence_availability`, `process_text_with_system_prompt`                                                                                                                                                         |
| `portable.rs`                  | `portable`                  | Portable mode detection and portable-aware path overrides.                                         | `is_portable`, `app_data_dir`, `app_log_dir`, `store_path`                                                                                                                                                                         |
| `utils.rs`                     | `utils`                     | Re-exports of clipboard/overlay/tray + centralized cancellation + Linux desktop helpers.           | `cancel_current_operation`, `is_wayland`, `is_kde_plasma`; re-exports clipboard/overlay/tray                                                                                                                                       |
| `helpers/mod.rs`               | `helpers`                   | Helper module exports.                                                                             | `pub mod clamshell`                                                                                                                                                                                                                |
| `helpers/clamshell.rs`         | `helpers::clamshell`        | macOS clamshell/laptop detection; stubs on other platforms.                                        | `is_clamshell`, `is_laptop`                                                                                                                                                                                                        |

---

## 4. Frontend (`src/`)

### 4.1 Entry Points

| File                               | Purpose                                                                                              | Key Exports        |
| ---------------------------------- | ---------------------------------------------------------------------------------------------------- | ------------------ |
| `src/main.tsx`                     | Main window bootstrap: sets `data-platform`, initializes i18n and model store, mounts `App`.         | —                  |
| `src/App.tsx`                      | Root component: sidebar, active settings section, footer, debug shortcut, model-load-failure toasts. | `App`              |
| `src/overlay/main.tsx`             | Overlay window bootstrap: initializes i18n and mounts `RecordingOverlay`.                            | —                  |
| `src/overlay/RecordingOverlay.tsx` | Floating overlay UI for recording/transcribing/processing; listens to overlay and mic-level events.  | `RecordingOverlay` |
| `src/overlay/index.html`           | Overlay window HTML entry point.                                                                     | —                  |
| `src/overlay/RecordingOverlay.css` | Overlay-specific styles.                                                                             | —                  |

### 4.2 State (Hooks & Stores)

| File                                   | Purpose                                                                              | Key Exports                 |
| -------------------------------------- | ------------------------------------------------------------------------------------ | --------------------------- |
| `src/hooks/useSettings.ts`             | React hook wrapping `settingsStore`; initializes store and exposes settings/actions. | `useSettings`               |
| `src/hooks/useOsType.ts`               | Returns current OS type for platform-specific UI.                                    | `useOsType`                 |
| `src/stores/settingsStore.ts`          | Zustand store for app settings; maps each change to a Tauri command.                 | `useSettingsStore`          |
| `src/stores/modelStore.ts`             | Zustand store for ASR models; listens to backend model lifecycle events.             | `useModelStore`             |
| `src/stores/fileTranscriptionStore.ts` | Zustand store for file transcription: file selection, progress, cancel.              | `useFileTranscriptionStore` |

### 4.3 Library Utilities & Types

| File                                | Purpose                                              | Key Exports                                                 |
| ----------------------------------- | ---------------------------------------------------- | ----------------------------------------------------------- |
| `src/lib/types/events.ts`           | Frontend event payload types.                        | `ModelStateEvent`, `RecordingErrorEvent`                    |
| `src/lib/utils/format.ts`           | Format model size in MB/GB.                          | `formatModelSize`                                           |
| `src/lib/utils/keyboard.ts`         | Normalize keyboard keys and format shortcut strings. | `getKeyName`, `formatKeyCombination`, `normalizeKey`        |
| `src/lib/utils/modelTranslation.ts` | Translated model name/description lookups.           | `getTranslatedModelName`, `getTranslatedModelDescription`   |
| `src/lib/utils/rtl.ts`              | RTL detection and document `dir`/`lang` management.  | `isRTLLanguage`, `updateDocumentDirection`, `initializeRTL` |
| `src/lib/constants/languages.ts`    | Full list of Whisper transcription languages.        | `LANGUAGES`                                                 |
| `src/utils/dateFormat.ts`           | Locale-aware date/time formatting for history.       | `formatDateTime`, `formatRelativeTime`                      |

### 4.4 Internationalization

| File                                  | Purpose                                                                                                                  | Key Exports                                               |
| ------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------- |
| `src/i18n/index.ts`                   | i18next setup, Vite glob discovery of locale JSONs, sync language from settings/system locale, update HTML `dir`/`lang`. | `i18n`, `SUPPORTED_LANGUAGES`, `syncLanguageFromSettings` |
| `src/i18n/languages.ts`               | Metadata registry for supported UI locales.                                                                              | `LANGUAGE_METADATA`                                       |
| `src/i18n/locales/*/translation.json` | Translation strings per locale (19 locales including `en`, `de`, `es`, `fr`, `ja`, `zh`, `zh-TW`, etc.).                 | data only                                                 |

### 4.5 Bindings & Env Types

| File                | Purpose                                                                               | Key Exports                                                                                           |
| ------------------- | ------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `src/bindings.ts`   | Auto-generated Tauri command/event wrappers and TypeScript types from `tauri-specta`. | `commands`, `events`, `Result`, plus backend types (`AppSettings`, `ModelInfo`, `HistoryEntry`, etc.) |
| `src/vite-env.d.ts` | Vite client type reference.                                                           | —                                                                                                     |
| `src/App.css`       | Global app styles.                                                                    | —                                                                                                     |

### 4.6 Layout Components

| File                               | Purpose                                                                        | Key Exports                  |
| ---------------------------------- | ------------------------------------------------------------------------------ | ---------------------------- |
| `src/components/Sidebar.tsx`       | Left navigation sidebar; defines section config and renders enabled nav items. | `Sidebar`, `SECTIONS_CONFIG` |
| `src/components/footer/Footer.tsx` | Bottom bar with model selector and app version.                                | `Footer`                     |
| `src/components/footer/index.ts`   | Barrel export for Footer.                                                      | `Footer`                     |

### 4.7 Icons

| File                                         | Purpose                  | Key Exports                                         |
| -------------------------------------------- | ------------------------ | --------------------------------------------------- |
| `src/components/icons/index.ts`              | Barrel export.           | `MicrophoneIcon`, `TranscriptionIcon`, `CancelIcon` |
| `src/components/icons/MicrophoneIcon.tsx`    | Microphone SVG.          | `MicrophoneIcon`                                    |
| `src/components/icons/TranscriptionIcon.tsx` | Transcription/brain SVG. | `TranscriptionIcon`                                 |
| `src/components/icons/CancelIcon.tsx`        | Cancel SVG.              | `CancelIcon`                                        |
| `src/components/icons/ResetIcon.tsx`         | Reset arrow SVG.         | `ResetIcon`                                         |
| `src/components/icons/HanhCuteHand.tsx`      | Brand hand logo.         | `HanhCuteHand`                                      |
| `src/components/icons/HanhCuteTextLogo.tsx`  | Brand text logo.         | `HanhCuteTextLogo`                                  |

### 4.8 Shared / UI Primitives

| File                                     | Purpose                                                          | Key Exports                   |
| ---------------------------------------- | ---------------------------------------------------------------- | ----------------------------- |
| `src/components/shared/ProgressBar.tsx`  | Progress bar UI for one or many items.                           | `ProgressBar`, `ProgressData` |
| `src/components/shared/index.ts`         | Barrel export.                                                   | `ProgressBar`                 |
| `src/components/ui/Alert.tsx`            | Alert banner variants.                                           | `Alert`                       |
| `src/components/ui/AudioPlayer.tsx`      | Custom audio player with play/pause/seek.                        | `AudioPlayer`                 |
| `src/components/ui/Badge.tsx`            | Pill badge.                                                      | `Badge`                       |
| `src/components/ui/Button.tsx`           | Styled button variants.                                          | `Button`                      |
| `src/components/ui/Dropdown.tsx`         | Custom single-select dropdown.                                   | `Dropdown`, `DropdownOption`  |
| `src/components/ui/Input.tsx`            | Styled text input.                                               | `Input`                       |
| `src/components/ui/PathDisplay.tsx`      | Read-only path display with Open button.                         | `PathDisplay`                 |
| `src/components/ui/ResetButton.tsx`      | Reset-to-default icon button.                                    | `ResetButton`                 |
| `src/components/ui/Select.tsx`           | `react-select` wrapper with theming; supports creatable options. | `Select`, `SelectOption`      |
| `src/components/ui/SettingContainer.tsx` | Standard row/card layout for a setting.                          | `SettingContainer`            |
| `src/components/ui/SettingsGroup.tsx`    | Bordered card grouping settings.                                 | `SettingsGroup`               |
| `src/components/ui/Slider.tsx`           | Range slider setting.                                            | `Slider`                      |
| `src/components/ui/TextDisplay.tsx`      | Read-only text display with copy.                                | `TextDisplay`                 |
| `src/components/ui/Textarea.tsx`         | Styled textarea.                                                 | `Textarea`                    |
| `src/components/ui/ToggleSwitch.tsx`     | Toggle switch built on `SettingContainer`.                       | `ToggleSwitch`                |
| `src/components/ui/Tooltip.tsx`          | Portal-based tooltip.                                            | `Tooltip`                     |
| `src/components/ui/index.ts`             | Barrel export.                                                   | all UI primitives             |

### 4.9 Model Selector & Onboarding

| File                                                        | Purpose                                                                                   | Key Exports                                                                      |
| ----------------------------------------------------------- | ----------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| `src/components/model-selector/index.ts`                    | Barrel export.                                                                            | `ModelSelector`, `ModelStatusButton`, `ModelDropdown`, `DownloadProgressDisplay` |
| `src/components/model-selector/ModelSelector.tsx`           | Footer model switcher; listens to model lifecycle events, auto-selects downloaded models. | `ModelSelector`, `ModelStatus`                                                   |
| `src/components/model-selector/ModelStatusButton.tsx`       | Status button with colored dot and dropdown chevron.                                      | `ModelStatusButton`                                                              |
| `src/components/model-selector/ModelDropdown.tsx`           | Dropdown listing downloaded models.                                                       | `ModelDropdown`                                                                  |
| `src/components/model-selector/DownloadProgressDisplay.tsx` | Bridges store download progress to `ProgressBar`.                                         | `DownloadProgressDisplay`                                                        |
| `src/components/onboarding/index.ts`                        | Barrel export.                                                                            | `ModelCard`                                                                      |
| `src/components/onboarding/ModelCard.tsx`                   | Rich model card for models page: scores, language tags, download/extract progress.        | `ModelCard`, `ModelCardStatus`                                                   |

### 4.10 Settings Section Wrappers

| File                                                                 | Purpose                                                                     | Key Exports                                            |
| -------------------------------------------------------------------- | --------------------------------------------------------------------------- | ------------------------------------------------------ |
| `src/components/settings/index.ts`                                   | Barrel export for all settings.                                             | many named exports                                     |
| `src/components/settings/general/GeneralSettings.tsx`                | General section: model settings card + file transcription panel.            | `GeneralSettings`                                      |
| `src/components/settings/general/ModelSettingsCard.tsx`              | Language/translation toggles for the active model.                          | `ModelSettingsCard`                                    |
| `src/components/settings/models/ModelsSettings.tsx`                  | Models management page: list, filter, download/select/delete.               | `ModelsSettings`                                       |
| `src/components/settings/advanced/AdvancedSettings.tsx`              | Advanced section: app, output, transcription, history, experimental groups. | `AdvancedSettings`                                     |
| `src/components/settings/history/HistorySettings.tsx`                | History section: paginated entries, playback, copy, save, delete, retry.    | `HistorySettings`                                      |
| `src/components/settings/post-processing/PostProcessingSettings.tsx` | Post-processing section: LLM provider config and prompt editor.             | `PostProcessingSettings`                               |
| `src/components/settings/about/AboutSettings.tsx`                    | About section: version, language, links, data/log dirs, acknowledgments.    | `AboutSettings`                                        |
| `src/components/settings/debug/DebugSettings.tsx`                    | Debug settings section wrapper.                                             | `DebugSettings`                                        |
| `src/components/settings/debug/index.ts`                             | Barrel export.                                                              | `DebugPaths`, `LogDirectory`, `LogLevelSelector`, etc. |

### 4.11 Settings Individual Controls

| File                                                        | Purpose                                                                             |
| ----------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| `src/components/settings/AccelerationSelector.tsx`          | Select Whisper accelerator/GPU device and ORT execution provider.                   |
| `src/components/settings/AlwaysOnMicrophone.tsx`            | Toggle always-on microphone listening mode.                                         |
| `src/components/settings/AppDataDirectory.tsx`              | Display/open app data directory.                                                    |
| `src/components/settings/AppLanguageSelector.tsx`           | Select app UI language.                                                             |
| `src/components/settings/AppendTrailingSpace.tsx`           | Toggle trailing space after pasted text.                                            |
| `src/components/settings/AudioFeedback.tsx`                 | Toggle start/stop audio feedback sounds.                                            |
| `src/components/settings/AutoSubmit.tsx`                    | Auto-submit behavior: off, Enter, Ctrl/Cmd+Enter.                                   |
| `src/components/settings/AutostartToggle.tsx`               | Launch at login toggle.                                                             |
| `src/components/settings/ClamshellMicrophoneSelector.tsx`   | Microphone used when Mac laptop is in clamshell mode.                               |
| `src/components/settings/ClipboardHandling.tsx`             | Clipboard handling after transcription.                                             |
| `src/components/settings/CustomWords.tsx`                   | Add/remove custom vocabulary words.                                                 |
| `src/components/settings/ExperimentalToggle.tsx`            | Enable experimental features group.                                                 |
| `src/components/settings/FileTranscriptionPanel.tsx`        | File picker, transcribe/stop controls, progress, output editor.                     |
| `src/components/settings/GlobalShortcutInput.tsx`           | Shortcut recorder via JS `KeyboardEvent` (Tauri global-shortcut impl).              |
| `src/components/settings/HandyKeysShortcutInput.tsx`        | Shortcut recorder via backend `handy-keys-event` events.                            |
| `src/components/settings/HistoryLimit.tsx`                  | Number of history entries to retain.                                                |
| `src/components/settings/LanguageSelector.tsx`              | Searchable transcription language dropdown.                                         |
| `src/components/settings/LazyStreamClose.tsx`               | Toggle lazy audio stream close.                                                     |
| `src/components/settings/MicrophoneSelector.tsx`            | Select input microphone.                                                            |
| `src/components/settings/ModelUnloadTimeout.tsx`            | When to unload the ASR model from memory.                                           |
| `src/components/settings/MuteWhileRecording.tsx`            | Mute system audio output while recording.                                           |
| `src/components/settings/OutputDeviceSelector.tsx`          | Select audio output device.                                                         |
| `src/components/settings/PasteMethod.tsx`                   | How text is pasted; external script path for Linux.                                 |
| `src/components/settings/PostProcessingSettingsPrompts.tsx` | Prompt editor re-export.                                                            |
| `src/components/settings/PostProcessingToggle.tsx`          | Enable LLM post-processing.                                                         |
| `src/components/settings/PushToTalk.tsx`                    | Toggle push-to-talk mode.                                                           |
| `src/components/settings/RecordingRetentionPeriod.tsx`      | How long to keep saved recordings.                                                  |
| `src/components/settings/ShortcutInput.tsx`                 | Chooses `GlobalShortcutInput` vs `HandyKeysShortcutInput` based on backend setting. |
| `src/components/settings/ShowOverlay.tsx`                   | Select overlay position (`none`/`top`/`bottom`).                                    |
| `src/components/settings/ShowTrayIcon.tsx`                  | Toggle system tray icon visibility.                                                 |
| `src/components/settings/SoundPicker.tsx`                   | Select sound theme and preview sounds.                                              |
| `src/components/settings/StartHidden.tsx`                   | Launch hidden to tray.                                                              |
| `src/components/settings/TranscriptionBreakMode.tsx`        | Streaming break mode (`none`/`sentence`/`word`).                                    |
| `src/components/settings/TranslateToEnglish.tsx`            | Toggle translation to English.                                                      |
| `src/components/settings/TypingTool.tsx`                    | Linux-only selector for direct-paste typing tool.                                   |
| `src/components/settings/VolumeSlider.tsx`                  | Audio feedback volume slider.                                                       |

### 4.12 Debug Controls

| File                                                               | Purpose                                              |
| ------------------------------------------------------------------ | ---------------------------------------------------- |
| `src/components/settings/debug/DebugPaths.tsx`                     | Static display of internal app/model/settings paths. |
| `src/components/settings/debug/KeyboardImplementationSelector.tsx` | Switch shortcut backend (Tauri vs Handy Keys).       |
| `src/components/settings/debug/LogDirectory.tsx`                   | Display/open log directory.                          |
| `src/components/settings/debug/LogLevelSelector.tsx`               | Select application log level.                        |
| `src/components/settings/debug/PasteDelay.tsx`                     | Adjust delay before pasted text is inserted.         |
| `src/components/settings/debug/RecordingBuffer.tsx`                | Adjust extra recording buffer duration.              |
| `src/components/settings/debug/WordCorrectionThreshold.tsx`        | Adjust word-correction threshold slider.             |

### 4.13 Post-Processing API Subcomponents

| File                                                                               | Purpose                                                                           | Key Exports                   |
| ---------------------------------------------------------------------------------- | --------------------------------------------------------------------------------- | ----------------------------- |
| `src/components/settings/PostProcessingSettingsApi/index.tsx`                      | Barrel export.                                                                    | `PostProcessingSettingsApi`   |
| `src/components/settings/PostProcessingSettingsApi/types.ts`                       | Model dropdown option type.                                                       | `ModelOption`                 |
| `src/components/settings/PostProcessingSettingsApi/usePostProcessProviderState.ts` | Hook managing provider selection, base URL, API key, model value, model fetching. | `usePostProcessProviderState` |
| `src/components/settings/PostProcessingSettingsApi/ProviderSelect.tsx`             | Provider dropdown.                                                                | `ProviderSelect`              |
| `src/components/settings/PostProcessingSettingsApi/BaseUrlField.tsx`               | Editable base URL input.                                                          | `BaseUrlField`                |
| `src/components/settings/PostProcessingSettingsApi/ApiKeyField.tsx`                | Password input for API key.                                                       | `ApiKeyField`                 |
| `src/components/settings/PostProcessingSettingsApi/ModelSelect.tsx`                | Creatable model dropdown.                                                         | `ModelSelect`                 |

---

## 5. Important Cross-Cutting Patterns

### 5.1 Command/Event Architecture

- Frontend calls backend via Tauri commands (typed by `tauri-specta` → `src/bindings.ts`).
- Backend emits events to update frontend, overlay, and tray (`ModelStateEvent`, `HistoryUpdatePayload`, mic-level events, overlay events).

### 5.2 Settings Flow

1. UI change → `settingsStore.ts` → Tauri command.
2. Backend writes to `tauri-plugin-store` JSON file in app data dir.
3. Relevant backend modules re-read settings on next action (or are notified).

### 5.3 Single Instance / CLI

- `tauri_plugin_single_instance` ensures only one process runs.
- Second instance sends CLI args to the running instance and exits.
- `signal_handle.rs` provides the shared trigger function used by CLI, signals, and shortcuts.

### 5.4 Model Loading

- `TranscriptionManager` supports multiple engines via `LoadedEngine` enum.
- GPU/CPU acceleration is selected via settings and applied before model load.
- Model idle timeout unloads model after a period of inactivity.

### 5.5 i18n Requirements

- All user-facing strings must use `i18next` translations.
- English source lives in `src/i18n/locales/en/translation.json`.
- Tray menu strings are generated from the `tray` section of each locale at compile time.

---

## 6. Key Directories (Non-Source)

| Directory                      | Purpose                                                                                   |
| ------------------------------ | ----------------------------------------------------------------------------------------- |
| `src-tauri/resources/`         | Bundled assets: feedback sounds, tray icons, VAD model (`silero_vad_v4.onnx`), app icons. |
| `src-tauri/icons/`             | App bundle icons.                                                                         |
| `src-tauri/swift/`             | Apple Intelligence Swift bridge source/header.                                            |
| `src-tauri/vendor/`            | Vendored dependencies (e.g., `whisper-rs-sys` patch).                                     |
| `asset/`                       | Project assets.                                                                           |
| `tests/`                       | Playwright / integration tests.                                                           |
| `scripts/`                     | Build/maintenance scripts.                                                                |
| `.nix/` / `nix/` / `flake.nix` | Nix development environment.                                                              |
| `dist/`                        | Vite build output consumed by Tauri.                                                      |

---

_Generated for AI agent context. For human contributor guidance, see `AGENTS.md`, `README.md`, `BUILD.md`, and `CONTRIBUTING.md`._

// Re-export all audio components
mod device;
mod recorder;
mod resampler;
mod utils;
mod visualizer;

pub use device::{list_input_devices, list_output_devices, CpalDeviceInfo};
pub use recorder::{
    is_microphone_access_denied, is_no_input_device_error, AudioRecorder, RecordingResult,
};
pub use resampler::FrameResampler;
pub use utils::{
    decode_media_file_streaming, read_media_file_samples, read_wav_samples, save_wav_file,
    vad_filter_samples, verify_wav_file, MediaFileDecoder, StreamingVad, StreamingWavWriter,
    FILE_TRANSCRIPTION_CHUNK_SAMPLES,
};
pub use visualizer::AudioVisualiser;

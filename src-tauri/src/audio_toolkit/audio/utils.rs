use anyhow::Result;
use hound::{WavReader, WavSpec, WavWriter};
use log::{debug, warn};
use rubato::{FftFixedIn, Resampler};
use std::fs::File;
use std::io::ErrorKind;
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units;
use symphonia::default::{get_codecs, get_probe};

use crate::audio_toolkit::vad::{SileroVad, SmoothedVad, VadFrame, VoiceActivityDetector};

const TRANSCRIPTION_SAMPLE_RATE: usize = 16_000;
const RESAMPLER_CHUNK_SIZE: usize = 1024;

/// Read a WAV file and return normalised f32 samples.
pub fn read_wav_samples<P: AsRef<Path>>(file_path: P) -> Result<Vec<f32>> {
    let reader = WavReader::open(file_path.as_ref())?;
    let samples = reader
        .into_samples::<i16>()
        .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
        .collect::<Result<Vec<f32>, _>>()?;
    Ok(samples)
}

/// Decode a media file, extract its first audio track, downmix it to mono, and
/// resample it to the 16 kHz input expected by the transcription engines.
pub fn read_media_file_samples<P: AsRef<Path>>(file_path: P) -> Result<Vec<f32>> {
    let file_path = file_path.as_ref();
    let file = Box::new(File::open(file_path)?);
    let mss = MediaSourceStream::new(file, MediaSourceStreamOptions::default());

    let mut hint = Hint::new();
    if let Some(extension) = file_path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(extension);
    }

    let probed = get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;

    let codec_registry = get_codecs();
    let decoder_options = DecoderOptions::default();
    let mut decoder_init_error = None;
    let mut selected_decoder = None;

    for track in format.tracks() {
        if track.codec_params.codec == CODEC_TYPE_NULL
            || codec_registry.get_codec(track.codec_params.codec).is_none()
        {
            continue;
        }

        match codec_registry.make(&track.codec_params, &decoder_options) {
            Ok(decoder) => {
                selected_decoder = Some((track.id, decoder));
                break;
            }
            Err(error) => {
                decoder_init_error = Some(error.to_string());
            }
        }
    }

    let (track_id, mut decoder) = selected_decoder.ok_or_else(|| {
        if let Some(error) = decoder_init_error {
            anyhow::anyhow!("No decodable audio track found: {}", error)
        } else {
            anyhow::anyhow!("No supported audio track found")
        }
    })?;

    let mut mono_samples = Vec::new();
    let mut source_sample_rate = None;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(error)) if error.kind() == ErrorKind::UnexpectedEof => {
                break
            }
            Err(SymphoniaError::IoError(error)) => return Err(error.into()),
            Err(SymphoniaError::ResetRequired) => {
                return Err(anyhow::anyhow!(
                    "Decoder reset is required but not supported"
                ));
            }
            Err(error) => return Err(error.into()),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(error)) => {
                warn!("Skipping undecodable packet in {:?}: {}", file_path, error);
                continue;
            }
            Err(SymphoniaError::IoError(error)) if error.kind() == ErrorKind::UnexpectedEof => {
                break
            }
            Err(SymphoniaError::ResetRequired) => {
                return Err(anyhow::anyhow!(
                    "Decoder reset is required but not supported"
                ));
            }
            Err(error) => return Err(error.into()),
        };

        let spec = *decoded.spec();
        source_sample_rate.get_or_insert(spec.rate);
        let channel_count = spec.channels.count().max(1);
        let mut sample_buffer =
            SampleBuffer::<f32>::new(units::Duration::from(decoded.capacity() as u64), spec);
        sample_buffer.copy_interleaved_ref(decoded);

        for frame in sample_buffer.samples().chunks(channel_count) {
            let sum = frame.iter().copied().sum::<f32>();
            mono_samples.push((sum / channel_count as f32).clamp(-1.0, 1.0));
        }
    }

    if mono_samples.is_empty() {
        return Err(anyhow::anyhow!("No audio samples found in file"));
    }

    let source_sample_rate = source_sample_rate
        .ok_or_else(|| anyhow::anyhow!("Could not determine media sample rate"))?;
    resample_to_transcription_rate(mono_samples, source_sample_rate as usize)
}

fn resample_to_transcription_rate(
    samples: Vec<f32>,
    source_sample_rate: usize,
) -> Result<Vec<f32>> {
    if source_sample_rate == TRANSCRIPTION_SAMPLE_RATE || samples.is_empty() {
        return Ok(samples);
    }

    let mut resampler = FftFixedIn::<f32>::new(
        source_sample_rate,
        TRANSCRIPTION_SAMPLE_RATE,
        RESAMPLER_CHUNK_SIZE,
        1,
        1,
    )?;
    let mut resampled =
        Vec::with_capacity(samples.len() * TRANSCRIPTION_SAMPLE_RATE / source_sample_rate.max(1));

    for chunk in samples.chunks(RESAMPLER_CHUNK_SIZE) {
        if chunk.len() == RESAMPLER_CHUNK_SIZE {
            let output = resampler.process(&[chunk], None)?;
            resampled.extend_from_slice(&output[0]);
            continue;
        }

        let mut padded = chunk.to_vec();
        padded.resize(RESAMPLER_CHUNK_SIZE, 0.0);
        let output = resampler.process(&[&padded], None)?;
        let expected_len = (chunk.len() * TRANSCRIPTION_SAMPLE_RATE).div_ceil(source_sample_rate);
        resampled.extend_from_slice(&output[0][..expected_len.min(output[0].len())]);
    }

    Ok(resampled)
}

/// Verify a WAV file by reading it back and checking the sample count.
pub fn verify_wav_file<P: AsRef<Path>>(file_path: P, expected_samples: usize) -> Result<()> {
    let reader = WavReader::open(file_path.as_ref())?;
    let actual_samples = reader.len() as usize;
    if actual_samples != expected_samples {
        anyhow::bail!(
            "WAV sample count mismatch: expected {}, got {}",
            expected_samples,
            actual_samples
        );
    }
    Ok(())
}

/// Save audio samples as a WAV file
pub fn save_wav_file<P: AsRef<Path>>(file_path: P, samples: &[f32]) -> Result<()> {
    let spec = WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = WavWriter::create(file_path.as_ref(), spec)?;

    // Convert f32 samples to i16 for WAV
    for sample in samples {
        let sample_i16 = (sample * i16::MAX as f32) as i16;
        writer.write_sample(sample_i16)?;
    }

    writer.finalize()?;
    debug!("Saved WAV file: {:?}", file_path.as_ref());
    Ok(())
}

const VAD_FRAME_SIZE: usize = (16000 * 30) / 1000;

/// Apply Voice Activity Detection to remove silence from audio samples.
///
/// Uses the Silero VAD model with smoothing (prefill/hangover) to keep only
/// speech segments. This prevents Whisper models (especially large-v3-turbo)
/// from hallucinating text during silent or non-speech portions of audio.
pub fn vad_filter_samples(samples: &[f32], vad_model_path: &Path) -> Result<Vec<f32>> {
    if samples.is_empty() {
        return Ok(Vec::new());
    }

    let silero = SileroVad::new(vad_model_path, 0.3)?;
    let mut smoothed = SmoothedVad::new(Box::new(silero), 15, 15, 2);

    let mut output = Vec::new();
    let silence_frame = vec![0.0f32; VAD_FRAME_SIZE];
    const TRAILING_FRAMES: usize = 20;

    for chunk in samples.chunks(VAD_FRAME_SIZE) {
        let frame = if chunk.len() < VAD_FRAME_SIZE {
            let mut padded = chunk.to_vec();
            padded.resize(VAD_FRAME_SIZE, 0.0);
            padded
        } else {
            chunk.to_vec()
        };
        match smoothed.push_frame(&frame)? {
            VadFrame::Speech(data) => output.extend_from_slice(data),
            VadFrame::Noise => {}
        }
    }

    for _ in 0..TRAILING_FRAMES {
        match smoothed.push_frame(&silence_frame)? {
            VadFrame::Speech(data) => output.extend_from_slice(data),
            VadFrame::Noise => {}
        }
    }

    Ok(output)
}

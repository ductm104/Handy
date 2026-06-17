use anyhow::Result;
use hound::{WavReader, WavSpec, WavWriter};
use log::warn;
use rubato::{FftFixedIn, Resampler};
use std::fs::File;
use std::io::{BufWriter, ErrorKind};
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units;
use symphonia::default::{get_codecs, get_probe};

use crate::audio_toolkit::vad::{SileroVad, SmoothedVad, VadFrame, VoiceActivityDetector};

const TRANSCRIPTION_SAMPLE_RATE: usize = 16_000;
const RESAMPLER_CHUNK_SIZE: usize = 16_384;

/// Default chunk size used when streaming file transcription. Each chunk
/// represents one minute of 16 kHz mono audio.
pub const FILE_TRANSCRIPTION_CHUNK_SAMPLES: usize = TRANSCRIPTION_SAMPLE_RATE * 60;

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

/// Stateful decoder that reads a media file incrementally, yielding 16 kHz
/// mono f32 chunks instead of loading the whole file into memory.
pub struct MediaFileDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    source_sample_rate: usize,
    resampler: Option<FftFixedIn<f32>>,
    /// Mono samples waiting to be fed to the resampler.
    resampler_input_buffer: Vec<f32>,
    /// Resampled (or directly decoded) samples waiting to be returned.
    pending_output: Vec<f32>,
    eof_reached: bool,
}

impl MediaFileDecoder {
    /// Open a media file and initialise the first decodable audio track.
    pub fn open<P: AsRef<Path>>(file_path: P) -> Result<Self> {
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
        let format = probed.format;

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

        let (track_id, decoder) = selected_decoder.ok_or_else(|| {
            if let Some(error) = decoder_init_error {
                anyhow::anyhow!("No decodable audio track found: {}", error)
            } else {
                anyhow::anyhow!("No supported audio track found")
            }
        })?;

        Ok(Self {
            format,
            decoder,
            track_id,
            source_sample_rate: 0,
            resampler: None,
            resampler_input_buffer: Vec::new(),
            pending_output: Vec::new(),
            eof_reached: false,
        })
    }

    /// Total duration in seconds if the container reports it.
    pub fn duration_seconds(&self) -> Option<f64> {
        let track = self
            .format
            .tracks()
            .iter()
            .find(|t| t.id == self.track_id)?;
        let n_frames = track.codec_params.n_frames?;
        let time_base = track.codec_params.time_base?;
        Some(n_frames as f64 * time_base.numer as f64 / time_base.denom as f64)
    }

    /// Decode and return up to `max_samples` 16 kHz mono samples.
    /// Returns `None` when the file has been fully consumed.
    pub fn decode_chunk(&mut self, max_samples: usize) -> Result<Option<Vec<f32>>> {
        let mut output = Vec::with_capacity(max_samples);

        loop {
            // Drain any buffered output first.
            if !self.pending_output.is_empty() {
                let to_take =
                    max_samples.min(output.len() + self.pending_output.len()) - output.len();
                output.extend_from_slice(&self.pending_output[..to_take]);
                self.pending_output.drain(..to_take);

                if output.len() >= max_samples {
                    return Ok(Some(output));
                }
            }

            if self.eof_reached {
                // Flush the resampler one last time.
                if let Some(ref mut resampler) = self.resampler {
                    if !self.resampler_input_buffer.is_empty() {
                        let mut padded = std::mem::take(&mut self.resampler_input_buffer);
                        padded.resize(RESAMPLER_CHUNK_SIZE, 0.0);
                        let out = resampler.process(&[&padded], None)?;
                        let expected_len = (padded.len() * TRANSCRIPTION_SAMPLE_RATE)
                            .div_ceil(self.source_sample_rate.max(1));
                        self.pending_output
                            .extend_from_slice(&out[0][..expected_len.min(out[0].len())]);
                        continue;
                    }
                }

                return if output.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(output))
                };
            }

            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(error)) if error.kind() == ErrorKind::UnexpectedEof => {
                    self.eof_reached = true;
                    continue;
                }
                Err(SymphoniaError::ResetRequired) => {
                    return Err(anyhow::anyhow!(
                        "Decoder reset is required but not supported"
                    ));
                }
                Err(error) => return Err(error.into()),
            };

            if packet.track_id() != self.track_id {
                continue;
            }

            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(SymphoniaError::DecodeError(error)) => {
                    warn!("Skipping undecodable packet: {}", error);
                    continue;
                }
                Err(SymphoniaError::IoError(error)) if error.kind() == ErrorKind::UnexpectedEof => {
                    self.eof_reached = true;
                    continue;
                }
                Err(SymphoniaError::ResetRequired) => {
                    return Err(anyhow::anyhow!(
                        "Decoder reset is required but not supported"
                    ));
                }
                Err(error) => return Err(error.into()),
            };

            let spec = *decoded.spec();
            let channel_count = spec.channels.count().max(1);

            if self.source_sample_rate == 0 {
                self.source_sample_rate = spec.rate as usize;
                if self.source_sample_rate != TRANSCRIPTION_SAMPLE_RATE {
                    self.resampler = Some(FftFixedIn::<f32>::new(
                        self.source_sample_rate,
                        TRANSCRIPTION_SAMPLE_RATE,
                        RESAMPLER_CHUNK_SIZE,
                        1,
                        1,
                    )?);
                }
            }

            let mut sample_buffer =
                SampleBuffer::<f32>::new(units::Duration::from(decoded.capacity() as u64), spec);
            sample_buffer.copy_interleaved_ref(decoded);

            // Downmix to mono.
            let mono: Vec<f32> = sample_buffer
                .samples()
                .chunks(channel_count)
                .map(|frame| {
                    let sum = frame.iter().copied().sum::<f32>();
                    (sum / channel_count as f32).clamp(-1.0, 1.0)
                })
                .collect();

            // If the source is already at the target rate, queue it directly.
            if self.resampler.is_none() {
                self.pending_output.extend(mono);
                continue;
            }

            self.resampler_input_buffer.extend(mono);

            // Process full resampler input chunks.
            while self.resampler_input_buffer.len() >= RESAMPLER_CHUNK_SIZE {
                let chunk: Vec<f32> = self
                    .resampler_input_buffer
                    .drain(..RESAMPLER_CHUNK_SIZE)
                    .collect();
                let resampler = self.resampler.as_mut().unwrap();
                let out = resampler.process(&[&chunk], None)?;
                self.pending_output.extend_from_slice(&out[0]);
            }
        }
    }
}

/// Decode a media file in chunks, calling `on_chunk` for each block of
/// `chunk_size` 16 kHz mono samples. The callback can return an error to
/// abort decoding early (e.g. on user cancellation).
pub fn decode_media_file_streaming<P, F>(
    file_path: P,
    chunk_size: usize,
    mut on_chunk: F,
) -> Result<()>
where
    P: AsRef<Path>,
    F: FnMut(&[f32]) -> Result<()>,
{
    let mut decoder = MediaFileDecoder::open(file_path)?;
    while let Some(chunk) = decoder.decode_chunk(chunk_size)? {
        on_chunk(&chunk)?;
    }
    Ok(())
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
    let mut writer = StreamingWavWriter::create(file_path)?;
    writer.write_samples(samples)?;
    writer.finalize()?;
    Ok(())
}

/// WAV writer that accepts samples incrementally to avoid keeping the entire
/// audio buffer in memory.
pub struct StreamingWavWriter {
    writer: WavWriter<BufWriter<File>>,
    sample_count: usize,
}

impl StreamingWavWriter {
    pub fn create<P: AsRef<Path>>(file_path: P) -> Result<Self> {
        let spec = WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let writer = WavWriter::create(file_path.as_ref(), spec)?;
        Ok(Self {
            writer,
            sample_count: 0,
        })
    }

    pub fn write_samples(&mut self, samples: &[f32]) -> Result<()> {
        for sample in samples {
            let sample_i16 = (*sample * i16::MAX as f32) as i16;
            self.writer.write_sample(sample_i16)?;
        }
        self.sample_count += samples.len();
        Ok(())
    }

    pub fn finalize(self) -> Result<usize> {
        self.writer.finalize()?;
        Ok(self.sample_count)
    }
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

/// Streaming variant of [`vad_filter_samples`].
///
/// Feeds audio incrementally through the same Silero VAD + smoothing pipeline
/// and returns speech samples as they become available. Use [`StreamingVad::flush`]
/// at the end of the stream to release any buffered hangover frames.
pub struct StreamingVad {
    smoothed: SmoothedVad,
    /// Re-used buffer for the speech data returned by a single push_frame call.
    temp_speech: Vec<f32>,
}

impl StreamingVad {
    pub fn new(vad_model_path: &Path) -> Result<Self> {
        let silero = SileroVad::new(vad_model_path, 0.3)?;
        let smoothed = SmoothedVad::new(Box::new(silero), 15, 15, 2);
        Ok(Self {
            smoothed,
            temp_speech: Vec::new(),
        })
    }

    /// Push a chunk of 16 kHz mono samples. Returns speech samples that are
    /// ready to be passed to the transcription engine.
    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        self.temp_speech.clear();

        for chunk in samples.chunks(VAD_FRAME_SIZE) {
            let frame = if chunk.len() < VAD_FRAME_SIZE {
                let mut padded = chunk.to_vec();
                padded.resize(VAD_FRAME_SIZE, 0.0);
                padded
            } else {
                chunk.to_vec()
            };
            match self.smoothed.push_frame(&frame)? {
                VadFrame::Speech(data) => self.temp_speech.extend_from_slice(data),
                VadFrame::Noise => {}
            }
        }

        Ok(std::mem::take(&mut self.temp_speech))
    }

    /// Flush any buffered hangover frames by feeding silence.
    pub fn flush(&mut self) -> Result<Vec<f32>> {
        self.temp_speech.clear();
        const TRAILING_FRAMES: usize = 20;
        let silence_frame = vec![0.0f32; VAD_FRAME_SIZE];

        for _ in 0..TRAILING_FRAMES {
            match self.smoothed.push_frame(&silence_frame)? {
                VadFrame::Speech(data) => self.temp_speech.extend_from_slice(data),
                VadFrame::Noise => {}
            }
        }

        Ok(std::mem::take(&mut self.temp_speech))
    }

    /// Returns whether the VAD currently considers itself to be inside a
    /// speech segment. Useful for callers that want to split audio at silence
    /// boundaries.
    pub fn is_in_speech(&self) -> bool {
        self.smoothed.is_in_speech()
    }
}

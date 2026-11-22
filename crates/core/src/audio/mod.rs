//! Audio processing: resampling, level metering, voice activity detection, segmentation,
//! encoding and the crash-safe recording file.

pub mod encode;
pub mod level;
pub mod pcm_file;
pub mod resample;
pub mod segment;
pub mod vad;

/// Sample rate used for everything after capture (recording file, VAD, uploads).
pub const SAMPLE_RATE: u32 = 16_000;
/// 20 ms analysis frames.
pub const FRAME_SAMPLES: usize = (SAMPLE_RATE as usize) / 50;

pub fn ms_to_samples(ms: u64) -> usize {
    (ms as usize * SAMPLE_RATE as usize) / 1000
}

pub fn samples_to_ms(samples: usize) -> u64 {
    (samples as u64 * 1000) / SAMPLE_RATE as u64
}

/// Converts f32 samples in [-1, 1] to i16 with clipping.
pub fn f32_to_i16(x: f32) -> i16 {
    let v = (x * 32767.0).round();
    v.clamp(-32768.0, 32767.0) as i16
}

/// Mixes interleaved multi-channel audio to mono.
pub fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|f| f.iter().sum::<f32>() / channels as f32)
        .collect()
}

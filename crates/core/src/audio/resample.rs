//! Streaming windowed-sinc resampler (any rate → 16 kHz). Quality is far beyond what speech
//! recognition needs; cost is a few ms per second of audio.

use std::f64::consts::PI;

const HALF_TAPS: usize = 24;
const PHASES: usize = 256;

pub struct Resampler {
    ratio: f64, // input samples per output sample
    table: Vec<f32>,
    cutoff: f64,
    buf: Vec<f32>,
    /// Position of the next output sample, in input samples relative to `buf[0]`.
    pos: f64,
    passthrough: bool,
}

impl Resampler {
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        let ratio = from_rate as f64 / to_rate as f64;
        let cutoff = if ratio > 1.0 { 0.92 / ratio } else { 0.92 };
        let taps = 2 * HALF_TAPS;
        let mut table = vec![0f32; (PHASES + 1) * taps];
        for p in 0..=PHASES {
            let frac = p as f64 / PHASES as f64;
            for t in 0..taps {
                // distance from the output point to input tap
                let x = (t as f64 - (HALF_TAPS as f64 - 1.0)) - frac;
                table[p * taps + t] = (kernel(x, cutoff)) as f32;
            }
        }
        Self {
            ratio,
            table,
            cutoff,
            buf: vec![0.0; HALF_TAPS - 1],
            pos: (HALF_TAPS - 1) as f64,
            passthrough: from_rate == to_rate,
        }
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if self.passthrough {
            return input.to_vec();
        }
        self.buf.extend_from_slice(input);
        let taps = 2 * HALF_TAPS;
        let mut out = Vec::with_capacity((input.len() as f64 / self.ratio) as usize + 2);
        loop {
            let base = self.pos.floor() as isize - (HALF_TAPS as isize - 1);
            if base < 0 {
                self.pos += self.ratio;
                continue;
            }
            let base = base as usize;
            if base + taps > self.buf.len() {
                break;
            }
            let frac = self.pos - self.pos.floor();
            let pf = frac * PHASES as f64;
            let p0 = pf.floor() as usize;
            let w = (pf - p0 as f64) as f32;
            let row0 = &self.table[p0 * taps..(p0 + 1) * taps];
            let row1 = &self.table[(p0 + 1) * taps..(p0 + 2) * taps];
            let mut acc = 0f32;
            for t in 0..taps {
                let k = row0[t] + (row1[t] - row0[t]) * w;
                acc += self.buf[base + t] * k;
            }
            out.push(acc);
            self.pos += self.ratio;
        }
        // Drop consumed input, keeping enough history for the next call.
        let keep_from = (self.pos.floor() as usize).saturating_sub(HALF_TAPS);
        if keep_from > 0 {
            self.buf.drain(..keep_from);
            self.pos -= keep_from as f64;
        }
        out
    }

    pub fn cutoff(&self) -> f64 {
        self.cutoff
    }
}

fn kernel(x: f64, cutoff: f64) -> f64 {
    let sinc = if x.abs() < 1e-12 { 1.0 } else { (PI * cutoff * x).sin() / (PI * x) };
    let sinc = if x.abs() < 1e-12 { cutoff } else { sinc };
    // Blackman window over [-HALF_TAPS, HALF_TAPS]
    let n = x / HALF_TAPS as f64;
    if n.abs() >= 1.0 {
        return 0.0;
    }
    let w = 0.42 + 0.5 * (PI * n).cos() + 0.08 * (2.0 * PI * n).cos();
    sinc * w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f64, secs: f64) -> Vec<f32> {
        (0..(rate as f64 * secs) as usize)
            .map(|i| (2.0 * PI * freq * i as f64 / rate as f64).sin() as f32 * 0.5)
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn output_length_matches_ratio_48k() {
        let mut r = Resampler::new(48_000, 16_000);
        let input = sine(48_000, 440.0, 1.0);
        let mut out = Vec::new();
        for chunk in input.chunks(480) {
            out.extend(r.process(chunk));
        }
        assert!((out.len() as i64 - 16_000).abs() < 40, "got {}", out.len());
    }

    #[test]
    fn output_length_matches_ratio_44k1() {
        let mut r = Resampler::new(44_100, 16_000);
        let input = sine(44_100, 300.0, 2.0);
        let mut out = Vec::new();
        for chunk in input.chunks(441) {
            out.extend(r.process(chunk));
        }
        assert!((out.len() as i64 - 32_000).abs() < 40, "got {}", out.len());
    }

    #[test]
    fn passband_tone_keeps_its_level() {
        let mut r = Resampler::new(48_000, 16_000);
        let out = r.process(&sine(48_000, 1000.0, 1.0));
        let level = rms(&out[1000..]);
        assert!((level - 0.3536).abs() < 0.02, "rms {level}");
    }

    #[test]
    fn tone_above_new_nyquist_is_removed() {
        let mut r = Resampler::new(48_000, 16_000);
        let out = r.process(&sine(48_000, 12_000.0, 1.0));
        assert!(rms(&out[1000..]) < 0.01);
    }

    #[test]
    fn same_rate_is_passthrough() {
        let mut r = Resampler::new(16_000, 16_000);
        let x = sine(16_000, 200.0, 0.1);
        assert_eq!(r.process(&x), x);
    }
}

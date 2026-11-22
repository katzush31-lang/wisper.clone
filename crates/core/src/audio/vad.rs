//! Energy-based voice activity detection with an adaptive noise floor.
//! Used to split long recordings at natural pauses and to skip silent audio
//! (sending silence to a speech model invites made-up text).

use super::level::rms_dbfs;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameInfo {
    pub energy_db: f32,
    pub speech: bool,
}

pub struct Vad {
    noise_db: f32,
    initialised: bool,
    run: u32,
    hangover: u32,
    in_speech: bool,
}

/// Speech must be this far above the noise floor.
const MARGIN_DB: f32 = 10.0;
/// Anything quieter than this is never speech.
const ABS_MIN_DB: f32 = -55.0;
/// Frames of continued "speech" after the energy drops (300 ms).
const HANGOVER_FRAMES: u32 = 15;
/// Consecutive loud frames needed to start speech (40 ms) — filters clicks.
const ONSET_FRAMES: u32 = 2;

impl Default for Vad {
    fn default() -> Self {
        Self { noise_db: -60.0, initialised: false, run: 0, hangover: 0, in_speech: false }
    }
}

impl Vad {
    pub fn process_frame(&mut self, frame: &[f32]) -> FrameInfo {
        let e = rms_dbfs(frame);
        if !self.initialised {
            self.noise_db = e.max(-90.0).min(-30.0);
            self.initialised = true;
        }
        // Noise floor: follows quiet frames quickly, loud frames very slowly.
        if e < self.noise_db {
            self.noise_db = 0.8 * self.noise_db + 0.2 * e;
        } else {
            self.noise_db = 0.997 * self.noise_db + 0.003 * e;
        }
        self.noise_db = self.noise_db.max(-90.0);
        let loud = e > (self.noise_db + MARGIN_DB).max(ABS_MIN_DB);
        if loud {
            self.run += 1;
            if self.run >= ONSET_FRAMES {
                self.in_speech = true;
                self.hangover = HANGOVER_FRAMES;
            }
        } else {
            self.run = 0;
            if self.hangover > 0 {
                self.hangover -= 1;
            } else {
                self.in_speech = false;
            }
        }
        FrameInfo { energy_db: e, speech: self.in_speech }
    }

    pub fn noise_floor_db(&self) -> f32 {
        self.noise_db
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::FRAME_SAMPLES;
    use super::*;

    /// Deterministic pseudo-noise.
    pub fn noise(n: usize, amp: f32, seed: &mut u32) -> Vec<f32> {
        (0..n)
            .map(|_| {
                *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((*seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * amp
            })
            .collect()
    }

    pub fn voice(n: usize, amp: f32, offset: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = (i + offset) as f32 / 16_000.0;
                amp * ((2.0 * std::f32::consts::PI * 180.0 * t).sin()
                    + 0.5 * (2.0 * std::f32::consts::PI * 360.0 * t).sin())
            })
            .collect()
    }

    #[test]
    fn detects_voice_over_noise_and_releases_after_hangover() {
        let mut vad = Vad::default();
        let mut seed = 7;
        for _ in 0..50 {
            assert!(!vad.process_frame(&noise(FRAME_SAMPLES, 0.002, &mut seed)).speech);
        }
        let mut speech_frames = 0;
        for k in 0..50 {
            if vad.process_frame(&voice(FRAME_SAMPLES, 0.2, k * FRAME_SAMPLES)).speech {
                speech_frames += 1;
            }
        }
        assert!(speech_frames >= 48);
        let mut still = 0;
        for _ in 0..40 {
            if vad.process_frame(&noise(FRAME_SAMPLES, 0.002, &mut seed)).speech {
                still += 1;
            }
        }
        assert_eq!(still, HANGOVER_FRAMES as usize);
    }

    #[test]
    fn single_click_is_not_speech() {
        let mut vad = Vad::default();
        let mut seed = 1;
        for _ in 0..30 {
            vad.process_frame(&noise(FRAME_SAMPLES, 0.002, &mut seed));
        }
        assert!(!vad.process_frame(&voice(FRAME_SAMPLES, 0.5, 0)).speech);
        assert!(!vad.process_frame(&noise(FRAME_SAMPLES, 0.002, &mut seed)).speech);
    }

    #[test]
    fn noisy_room_adapts() {
        let mut vad = Vad::default();
        let mut seed = 3;
        // Loud steady fan noise: should not be speech after adapting.
        let mut speech = 0;
        for i in 0..400 {
            let f = vad.process_frame(&noise(FRAME_SAMPLES, 0.05, &mut seed));
            if i > 100 && f.speech {
                speech += 1;
            }
        }
        assert_eq!(speech, 0);
    }
}

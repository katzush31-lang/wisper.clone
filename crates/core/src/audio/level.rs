//! Microphone level for the bar's live meter.

/// RMS of a block in dBFS (silence → -100).
pub fn rms_dbfs(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -100.0;
    }
    let mean_sq = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    if mean_sq <= 1e-10 {
        -100.0
    } else {
        10.0 * mean_sq.log10()
    }
}

/// Maps dBFS to 0..1 for display: -60 dB and below is 0, -12 dB and above is 1.
pub fn display_level(dbfs: f32) -> f32 {
    ((dbfs + 60.0) / 48.0).clamp(0.0, 1.0)
}

/// Smooths levels for display: fast attack, slow release.
pub struct LevelMeter {
    value: f32,
}

impl Default for LevelMeter {
    fn default() -> Self {
        Self { value: 0.0 }
    }
}

impl LevelMeter {
    pub fn update(&mut self, samples: &[f32]) -> f32 {
        let target = display_level(rms_dbfs(samples));
        self.value = if target > self.value { target } else { self.value * 0.82 + target * 0.18 };
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(rms_dbfs(&[]), -100.0);
        assert_eq!(rms_dbfs(&[0.0; 100]), -100.0);
        let full = vec![1.0f32; 100];
        assert!((rms_dbfs(&full)).abs() < 0.01);
        assert_eq!(display_level(-100.0), 0.0);
        assert_eq!(display_level(0.0), 1.0);
        let mut m = LevelMeter::default();
        assert!(m.update(&[0.5; 320]) > 0.9);
        let after = m.update(&[0.0; 320]);
        assert!(after > 0.5 && after < 1.0, "slow release, got {after}");
    }
}

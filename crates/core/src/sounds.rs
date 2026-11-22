//! Start/stop/error sounds, synthesised so there are no audio files to license.
//! Soft sine tones with smooth envelopes; volume is applied when generating.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    Start,
    Stop,
    Error,
}

const RATE: u32 = 44_100;

fn tone(out: &mut Vec<f32>, start_s: f32, freq: f32, dur_s: f32, amp: f32) {
    let start = (start_s * RATE as f32) as usize;
    let len = (dur_s * RATE as f32) as usize;
    if out.len() < start + len {
        out.resize(start + len, 0.0);
    }
    for i in 0..len {
        let t = i as f32 / RATE as f32;
        // Quick attack, exponential decay: a soft "marimba" ping.
        let attack = (t / 0.008).min(1.0);
        let decay = (-t * 9.0).exp();
        let release = ((len - i) as f32 / (0.02 * RATE as f32)).min(1.0);
        let env = attack * decay * release;
        let v = (2.0 * std::f32::consts::PI * freq * t).sin() + 0.18 * (2.0 * std::f32::consts::PI * freq * 2.0 * t).sin();
        out[start + i] += v * env * amp;
    }
}

/// 16-bit mono WAV bytes for a sound at a volume (0.0–1.0).
pub fn wav(sound: Sound, volume: f32) -> Vec<u8> {
    let mut s = Vec::new();
    match sound {
        Sound::Start => {
            tone(&mut s, 0.0, 659.25, 0.16, 0.5); // E5
            tone(&mut s, 0.07, 987.77, 0.22, 0.45); // B5
        }
        Sound::Stop => {
            tone(&mut s, 0.0, 987.77, 0.14, 0.45);
            tone(&mut s, 0.06, 659.25, 0.22, 0.5);
        }
        Sound::Error => {
            tone(&mut s, 0.0, 330.0, 0.16, 0.5);
            tone(&mut s, 0.14, 262.0, 0.24, 0.5);
        }
    }
    let gain = volume.clamp(0.0, 1.0) * 0.55;
    let samples: Vec<i16> = s.iter().map(|v| ((v * gain).clamp(-1.0, 1.0) * 32767.0) as i16).collect();
    let data_len = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for v in samples {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_wav_and_volume_scales() {
        let loud = wav(Sound::Start, 1.0);
        let quiet = wav(Sound::Start, 0.2);
        assert_eq!(&loud[..4], b"RIFF");
        assert_eq!(loud.len(), quiet.len());
        let peak = |w: &[u8]| w[44..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs()).max().unwrap();
        assert!(peak(&loud) > peak(&quiet) * 3);
        assert!(peak(&loud) < 32000, "no clipping");
        assert!(wav(Sound::Stop, 0.0)[44..].iter().all(|&b| b == 0));
        let secs = (loud.len() - 44) as f32 / 2.0 / 44_100.0;
        assert!(secs > 0.2 && secs < 0.4, "{secs}");
    }
}

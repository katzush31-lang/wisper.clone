//! Encoding 16 kHz mono audio for upload (FLAC: lossless, about half the size of WAV)
//! and for storage in history.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use flacenc::component::BitRepr;
use flacenc::error::Verify;
use flacenc::error::SourceError;
use flacenc::source::{Fill, Source};

use super::SAMPLE_RATE;

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("audio encoding failed: {0}")]
    Encode(String),
    #[error("could not read recording: {0}")]
    Io(#[from] std::io::Error),
}

fn encoder_config() -> flacenc::error::Verified<flacenc::config::Encoder> {
    flacenc::config::Encoder::default()
        .into_verified()
        .expect("default FLAC config is valid")
}

/// Encodes in-memory samples (used for upload segments, at most a minute or two).
pub fn encode_flac(samples: &[i16]) -> Result<Vec<u8>, EncodeError> {
    let wide: Vec<i32> = samples.iter().map(|&s| s as i32).collect();
    let config = encoder_config();
    let source = flacenc::source::MemSource::from_samples(&wide, 1, 16, SAMPLE_RATE as usize);
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| EncodeError::Encode(format!("{e:?}")))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|e| EncodeError::Encode(format!("{e:?}")))?;
    Ok(sink.as_slice().to_vec())
}

/// Streams a raw PCM recording file (any length) into a FLAC file without loading it all.
pub fn pcm_file_to_flac(pcm: &Path, out: &Path) -> Result<u64, EncodeError> {
    let file = std::fs::File::open(pcm)?;
    let config = encoder_config();
    let source = PcmFileSource { file, buf: Vec::new() };
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| EncodeError::Encode(format!("{e:?}")))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|e| EncodeError::Encode(format!("{e:?}")))?;
    let tmp = out.with_extension("flac.part");
    std::fs::write(&tmp, sink.as_slice())?;
    std::fs::rename(&tmp, out)?;
    Ok(sink.as_slice().len() as u64)
}

struct PcmFileSource {
    file: std::fs::File,
    buf: Vec<u8>,
}

impl Source for PcmFileSource {
    fn channels(&self) -> usize {
        1
    }
    fn bits_per_sample(&self) -> usize {
        16
    }
    fn sample_rate(&self) -> usize {
        SAMPLE_RATE as usize
    }
    fn read_samples<F: Fill>(&mut self, block_size: usize, dest: &mut F) -> Result<usize, SourceError> {
        self.buf.resize(block_size * 2, 0);
        let mut filled = 0;
        while filled < self.buf.len() {
            let n = self.file.read(&mut self.buf[filled..]).map_err(SourceError::from_io_error)?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        let filled = filled - filled % 2;
        dest.fill_le_bytes(&self.buf[..filled], 2)?;
        Ok(filled / 2)
    }
}

/// Minimal WAV writer (fallback upload format and for tests).
pub fn encode_wav(samples: &[i16]) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::with_capacity(44 + samples.len() * 2));
    {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::new(&mut cursor, spec).expect("in-memory wav");
        for &s in samples {
            w.write_sample(s).expect("in-memory wav");
        }
        w.finalize().expect("in-memory wav");
    }
    cursor.into_inner()
}

/// Reads a sample range from a raw PCM file.
pub fn read_pcm_range(pcm: &Path, start: usize, end: usize) -> Result<Vec<i16>, EncodeError> {
    let mut f = std::fs::File::open(pcm)?;
    let len = f.metadata()?.len() as usize / 2;
    let end = end.min(len);
    if start >= end {
        return Ok(Vec::new());
    }
    f.seek(SeekFrom::Start(start as u64 * 2))?;
    let mut bytes = vec![0u8; (end - start) * 2];
    f.read_exact(&mut bytes)?;
    Ok(bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize) -> Vec<i16> {
        (0..n).map(|i| ((i as f32 * 0.07).sin() * 8000.0) as i16).collect()
    }

    #[test]
    fn flac_has_magic_and_is_smaller_than_wav() {
        let s = tone(16_000 * 3);
        let flac = encode_flac(&s).unwrap();
        assert_eq!(&flac[..4], b"fLaC");
        assert!(flac.len() < encode_wav(&s).len());
    }

    #[test]
    fn wav_roundtrip() {
        let s = tone(1000);
        let wav = encode_wav(&s);
        let r = hound::WavReader::new(std::io::Cursor::new(wav)).unwrap();
        assert_eq!(r.spec().sample_rate, 16_000);
        let back: Vec<i16> = r.into_samples::<i16>().map(|x| x.unwrap()).collect();
        assert_eq!(back, s);
    }

    #[test]
    fn pcm_file_streams_to_flac_and_ranges_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let pcm = dir.path().join("a.pcm");
        let s = tone(16_000 * 5 + 123);
        let bytes: Vec<u8> = s.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(&pcm, bytes).unwrap();
        let out = dir.path().join("a.flac");
        let size = pcm_file_to_flac(&pcm, &out).unwrap();
        assert!(size > 100);
        assert_eq!(&std::fs::read(&out).unwrap()[..4], b"fLaC");
        let r = read_pcm_range(&pcm, 100, 200).unwrap();
        assert_eq!(r, s[100..200].to_vec());
        assert!(read_pcm_range(&pcm, 10_000_000, 10_000_010).unwrap().is_empty());
    }
}

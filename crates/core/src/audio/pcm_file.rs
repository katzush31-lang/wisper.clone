//! Crash-safe recording file: raw 16-bit little-endian mono samples at 16 kHz, appended
//! and flushed continuously while the user speaks. If the app or PC crashes mid-dictation,
//! everything up to the last flush (≤ ~250 ms) is still on disk and is recovered at startup.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct PcmWriter {
    path: PathBuf,
    out: BufWriter<File>,
    samples: usize,
    last_flush: Instant,
}

const FLUSH_EVERY: Duration = Duration::from_millis(250);

impl PcmWriter {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let f = OpenOptions::new().create(true).write(true).truncate(true).open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            out: BufWriter::with_capacity(64 * 1024, f),
            samples: 0,
            last_flush: Instant::now(),
        })
    }

    pub fn append(&mut self, samples: &[i16]) -> std::io::Result<()> {
        for s in samples {
            self.out.write_all(&s.to_le_bytes())?;
        }
        self.samples += samples.len();
        if self.last_flush.elapsed() >= FLUSH_EVERY {
            self.flush()?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()?;
        self.last_flush = Instant::now();
        Ok(())
    }

    /// Flushes and syncs to disk.
    pub fn finish(mut self) -> std::io::Result<(PathBuf, usize)> {
        self.out.flush()?;
        self.out.get_ref().sync_all()?;
        Ok((self.path, self.samples))
    }

    pub fn samples(&self) -> usize {
        self.samples
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Number of samples stored in a PCM file.
pub fn pcm_len(path: &Path) -> std::io::Result<usize> {
    Ok(std::fs::metadata(path)?.len() as usize / 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_are_visible_after_flush() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("rec").join("x.pcm");
        let mut w = PcmWriter::create(&p).unwrap();
        w.append(&[1, -2, 3]).unwrap();
        w.flush().unwrap();
        assert_eq!(pcm_len(&p).unwrap(), 3);
        w.append(&[4; 1000]).unwrap();
        let (path, n) = w.finish().unwrap();
        assert_eq!(n, 1003);
        assert_eq!(pcm_len(&path).unwrap(), 1003);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(i16::from_le_bytes([bytes[2], bytes[3]]), -2);
    }
}

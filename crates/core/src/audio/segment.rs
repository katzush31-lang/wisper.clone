//! Splits a recording into segments at natural pauses while the user is still talking,
//! so each finished segment can be transcribed immediately. Only the last segment is left
//! for after the user stops, which keeps long dictations fast.

use super::{ms_to_samples, vad::FrameInfo, FRAME_SAMPLES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentPlan {
    pub index: usize,
    /// Sample range [start, end) in the 16 kHz recording.
    pub start: usize,
    pub end: usize,
    /// False if the segment holds no detectable speech (it is then not transcribed).
    pub has_speech: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct SegmentConfig {
    /// A pause this long can end a segment once the segment is at least `min_len_ms` long.
    pub pause_ms: u64,
    pub min_len_ms: u64,
    /// A longer pause allows an earlier cut.
    pub long_pause_ms: u64,
    pub long_pause_min_len_ms: u64,
    /// Hard limit: cut at the quietest point in the last few seconds.
    pub max_len_ms: u64,
    pub forced_search_ms: u64,
    /// Minimum amount of speech (ms) for a segment to count as speech.
    pub min_speech_ms: u64,
}

impl Default for SegmentConfig {
    fn default() -> Self {
        Self {
            // Measured after the VAD's 300 ms hangover, so 450 ms here ≈ 0.75 s of real silence.
            pause_ms: 450,
            min_len_ms: 8_000,
            long_pause_ms: 1_000,
            long_pause_min_len_ms: 4_000,
            max_len_ms: 45_000,
            forced_search_ms: 10_000,
            min_speech_ms: 160,
        }
    }
}

pub struct Segmenter {
    cfg: SegmentConfig,
    seg_start: usize,
    frames_in_seg: Vec<FrameInfo>,
    silence_run: usize,
    total_samples: usize,
    next_index: usize,
    speech_frames_total: usize,
}

impl Segmenter {
    pub fn new(cfg: SegmentConfig) -> Self {
        Self {
            cfg,
            seg_start: 0,
            frames_in_seg: Vec::new(),
            silence_run: 0,
            total_samples: 0,
            next_index: 0,
            speech_frames_total: 0,
        }
    }

    /// Feed the analysis of the next 20 ms frame. Returns a segment if one just closed.
    pub fn push_frame(&mut self, info: FrameInfo) -> Option<SegmentPlan> {
        self.total_samples += FRAME_SAMPLES;
        self.frames_in_seg.push(info);
        if info.speech {
            self.silence_run = 0;
            self.speech_frames_total += 1;
        } else {
            self.silence_run += 1;
        }
        let seg_len = self.total_samples - self.seg_start;
        let silence = self.silence_run * FRAME_SAMPLES;
        let has_speech = self.speech_in(&self.frames_in_seg);

        let pause_cut = has_speech
            && ((silence >= ms_to_samples(self.cfg.pause_ms) && seg_len >= ms_to_samples(self.cfg.min_len_ms))
                || (silence >= ms_to_samples(self.cfg.long_pause_ms)
                    && seg_len >= ms_to_samples(self.cfg.long_pause_min_len_ms)));
        if pause_cut {
            // Cut in the middle of the pause so both sides keep a little silence.
            let cut = self.total_samples - silence / 2;
            return Some(self.close_at(cut));
        }
        if seg_len >= ms_to_samples(self.cfg.max_len_ms) {
            let cut = self.quietest_point();
            return Some(self.close_at(cut));
        }
        None
    }

    /// Closes the final segment at the end of the recording.
    pub fn finish(&mut self) -> Option<SegmentPlan> {
        if self.total_samples <= self.seg_start {
            return None;
        }
        let end = self.total_samples;
        Some(self.close_at(end))
    }

    /// Total samples seen so far.
    pub fn samples(&self) -> usize {
        self.total_samples
    }

    /// The earliest sample at which the open segment could still be cut. Audio before this
    /// point is certain to belong to the open segment, so it can be streamed out (live
    /// transcription) without ever crossing a boundary the segmenter decides later.
    ///
    /// A pause cut lands inside the current silent run (never before it starts); a forced cut
    /// lands within the search window before the length limit.
    pub fn safe_point(&self) -> usize {
        let pause_safe = self.total_samples - self.silence_run * FRAME_SAMPLES;
        let max_len = ms_to_samples(self.cfg.max_len_ms);
        let search = ms_to_samples(self.cfg.forced_search_ms);
        let forced_safe = self.seg_start + max_len.saturating_sub(search);
        pause_safe.min(forced_safe).clamp(self.seg_start, self.total_samples)
    }

    /// True once any speech at all has been detected in the recording.
    pub fn heard_speech(&self) -> bool {
        self.speech_frames_total * FRAME_SAMPLES >= ms_to_samples(self.cfg.min_speech_ms)
    }

    fn speech_in(&self, frames: &[FrameInfo]) -> bool {
        let n = frames.iter().filter(|f| f.speech).count();
        n * FRAME_SAMPLES >= ms_to_samples(self.cfg.min_speech_ms)
    }

    fn quietest_point(&self) -> usize {
        let search = ms_to_samples(self.cfg.forced_search_ms) / FRAME_SAMPLES;
        let n = self.frames_in_seg.len();
        let from = n.saturating_sub(search);
        let mut best = n - 1;
        let mut best_e = f32::MAX;
        for i in from..n {
            // Prefer non-speech frames, then lowest energy.
            let score = self.frames_in_seg[i].energy_db + if self.frames_in_seg[i].speech { 100.0 } else { 0.0 };
            if score < best_e {
                best_e = score;
                best = i;
            }
        }
        self.seg_start + (best + 1) * FRAME_SAMPLES
    }

    fn close_at(&mut self, cut: usize) -> SegmentPlan {
        let cut = cut.clamp(self.seg_start, self.total_samples);
        let frames_before = (cut - self.seg_start) / FRAME_SAMPLES;
        let (closed, rest) = self.frames_in_seg.split_at(frames_before.min(self.frames_in_seg.len()));
        let plan = SegmentPlan {
            index: self.next_index,
            start: self.seg_start,
            end: cut,
            has_speech: self.speech_in(closed),
        };
        self.frames_in_seg = rest.to_vec();
        self.seg_start = cut;
        self.silence_run = self.frames_in_seg.iter().rev().take_while(|f| !f.speech).count();
        self.next_index += 1;
        plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(spec: &[(bool, u64)]) -> Vec<FrameInfo> {
        let mut v = vec![];
        for &(speech, ms) in spec {
            for _ in 0..(ms / 20) {
                v.push(FrameInfo { energy_db: if speech { -20.0 } else { -60.0 }, speech });
            }
        }
        v
    }

    fn run(spec: &[(bool, u64)]) -> Vec<SegmentPlan> {
        let mut s = Segmenter::new(SegmentConfig::default());
        let mut out = vec![];
        for f in frames(spec) {
            out.extend(s.push_frame(f));
        }
        out.extend(s.finish());
        out
    }

    /// Every cut the segmenter makes lands at or after every safe point it reported earlier
    /// for that segment (live transcription relies on this to never stream across a cut).
    #[test]
    fn cuts_never_land_before_a_reported_safe_point() {
        let mut spec = vec![];
        for i in 0..40u64 {
            spec.push((true, 2_000 + (i * 1_370) % 9_000));
            spec.push((false, 200 + (i * 530) % 1_600));
        }
        spec.push((true, 120_000)); // non-stop speech: forced cuts
        spec.push((false, 60_000)); // long silence
        let mut s = Segmenter::new(SegmentConfig::default());
        let mut max_safe = 0;
        let mut cuts = 0;
        for f in frames(&spec) {
            if let Some(p) = s.push_frame(f) {
                assert!(p.end >= max_safe, "cut at {} before safe point {}", p.end, max_safe);
                cuts += 1;
            }
            let safe = s.safe_point();
            assert!(safe <= s.samples());
            max_safe = max_safe.max(safe);
        }
        assert!(cuts > 10);
        // While speaking, the safe point keeps up with the audio (nothing is held back).
        let mut s = Segmenter::new(SegmentConfig::default());
        for f in frames(&[(true, 3_000)]) {
            s.push_frame(f);
        }
        assert_eq!(s.safe_point(), s.samples());
    }

    #[test]
    fn short_dictation_is_one_segment() {
        let segs = run(&[(false, 300), (true, 4000), (false, 500)]);
        assert_eq!(segs.len(), 1);
        assert!(segs[0].has_speech);
        assert_eq!(segs[0].start, 0);
    }

    #[test]
    fn cuts_at_a_pause_after_min_length() {
        let segs = run(&[(true, 11_000), (false, 800), (true, 3000)]);
        assert_eq!(segs.len(), 2);
        // Cut is inside the pause.
        let cut_ms = segs[0].end as u64 * 1000 / 16_000;
        assert!((11_000..11_800).contains(&cut_ms), "cut at {cut_ms}");
        assert_eq!(segs[1].start, segs[0].end);
        assert!(segs.iter().all(|s| s.has_speech));
    }

    #[test]
    fn short_pauses_before_min_length_do_not_cut() {
        let segs = run(&[(true, 3000), (false, 800), (true, 3000), (false, 800), (true, 2000)]);
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn long_pause_cuts_earlier() {
        let segs = run(&[(true, 5000), (false, 1500), (true, 2000)]);
        assert_eq!(segs.len(), 2);
    }

    #[test]
    fn nonstop_speech_is_force_cut_at_max_length() {
        let segs = run(&[(true, 150_000)]);
        assert!(segs.len() >= 3);
        for s in &segs[..segs.len() - 1] {
            let len_ms = (s.end - s.start) as u64 * 1000 / 16_000;
            assert!(len_ms <= 45_000, "segment {len_ms} ms");
        }
        // Segments tile the recording without gaps.
        for w in segs.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
        assert_eq!(segs.last().unwrap().end, 150_000 * 16);
    }

    #[test]
    fn silence_only_recording_has_no_speech() {
        let segs = run(&[(false, 5000)]);
        assert_eq!(segs.len(), 1);
        assert!(!segs[0].has_speech);
    }

    #[test]
    fn thirty_minutes_produces_many_tiled_segments() {
        let mut spec = vec![];
        for _ in 0..150 {
            spec.push((true, 11_000));
            spec.push((false, 1000));
        }
        let segs = run(&spec);
        assert!(segs.len() >= 140);
        assert_eq!(segs[0].start, 0);
        for w in segs.windows(2) {
            assert_eq!(w[0].end, w[1].start);
            assert_eq!(w[0].index + 1, w[1].index);
        }
    }
}

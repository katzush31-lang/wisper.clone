//! The dictation pipeline.
//!
//! Recording: audio is resampled to 16 kHz, appended to a crash-safe file, analysed for
//! speech, and cut into segments at natural pauses. Each finished segment is transcribed in
//! the background while the user keeps talking, so only the last few seconds are left when
//! they stop. Nothing is ever deleted on failure: a dictation that can't be finished is kept
//! with its audio and retried later.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::task::JoinHandle;

use crate::apps::AppInfo;
use crate::audio::{self, encode, level::LevelMeter, pcm_file::PcmWriter, resample::Resampler, segment::{SegmentConfig, SegmentPlan, Segmenter}, vad::Vad};
use crate::db::{Db, HistoryItem, SegmentRow, Status};
use crate::dictionary;
use crate::hotkey::RecordingKind;
use crate::live::{LiveEndpoint, LiveFeed, LiveSetup, LiveShared};
use crate::modes::{self, Mode};
use crate::prompts::{self, CleanupContext, CleanupLevel};
use crate::providers::{
    anthropic::Anthropic, openai::OpenAi, ErrorKind, ProviderError, RetryPolicy, SystemBlock, TextModel, TextRequest,
    TranscribeRequest, Transcriber,
};
use crate::settings::{CleanupProvider, Settings};
use crate::snippets;
use crate::text;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApiKeys {
    pub openai: Option<String>,
    pub anthropic: Option<String>,
}

/// Creates provider clients; swapped for fakes in tests.
pub trait ProviderFactory: Send + Sync {
    fn transcriber(&self, keys: &ApiKeys) -> Arc<dyn Transcriber>;
    fn text_model(&self, keys: &ApiKeys, provider: CleanupProvider) -> Arc<dyn TextModel>;
    /// Where live transcription connects (None = not available).
    fn live_endpoint(&self) -> Option<LiveEndpoint> {
        None
    }
}

pub struct RealProviders;

impl ProviderFactory for RealProviders {
    fn transcriber(&self, keys: &ApiKeys) -> Arc<dyn Transcriber> {
        Arc::new(OpenAi::new(keys.openai.as_deref().unwrap_or("")))
    }
    fn text_model(&self, keys: &ApiKeys, provider: CleanupProvider) -> Arc<dyn TextModel> {
        match provider {
            CleanupProvider::Anthropic => Arc::new(Anthropic::new(keys.anthropic.as_deref().unwrap_or(""))),
            CleanupProvider::OpenAi => Arc::new(OpenAi::new(keys.openai.as_deref().unwrap_or(""))),
        }
    }
    fn live_endpoint(&self) -> Option<LiveEndpoint> {
        Some(LiveEndpoint::openai())
    }
}

/// What we know about where the text is going, captured when recording starts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureContext {
    pub app: AppInfo,
    pub text_before: Option<String>,
    pub text_after: Option<String>,
    pub selection: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Transcribing { done: usize, total: usize },
    Cleaning,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Processed {
    pub id: String,
    pub raw_text: String,
    pub final_text: String,
    pub mode_id: String,
    pub mode_name: String,
    /// Set when AI cleanup failed and the text was produced without it.
    pub cleanup_warning: Option<String>,
    pub cleanup_error_kind: Option<ErrorKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Text(Processed),
    /// No speech was detected; nothing to insert.
    NoSpeech { id: String },
}

#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PipelineError {
    /// Saved; will be retried automatically (offline, provider trouble).
    #[error("{message}")]
    Queued { id: String, message: String, kind: ErrorKind },
    /// Saved; needs the user (bad key, no credit).
    #[error("{message}")]
    Failed { id: String, message: String, kind: Option<ErrorKind> },
}

impl PipelineError {
    pub fn id(&self) -> &str {
        match self {
            PipelineError::Queued { id, .. } | PipelineError::Failed { id, .. } => id,
        }
    }
    pub fn message(&self) -> &str {
        match self {
            PipelineError::Queued { message, .. } | PipelineError::Failed { message, .. } => message,
        }
    }
}

pub struct Engine {
    pub db: Arc<Db>,
    data_dir: PathBuf,
    settings: RwLock<Settings>,
    keys: RwLock<ApiKeys>,
    factory: Arc<dyn ProviderFactory>,
    pub retry: RetryPolicy,
    active: Mutex<HashSet<String>>,
}

pub(crate) const KEYWORD_LIMIT: usize = 60;
const CLEANUP_CHUNK_WORDS: usize = 900;
const CLEANUP_SPLIT_THRESHOLD: usize = 1400;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Engine {
    pub fn new(db: Arc<Db>, data_dir: PathBuf, settings: Settings, keys: ApiKeys, factory: Arc<dyn ProviderFactory>) -> Arc<Engine> {
        Self::new_with_retry(db, data_dir, settings, keys, factory, RetryPolicy::default())
    }

    pub fn new_with_retry(
        db: Arc<Db>,
        data_dir: PathBuf,
        settings: Settings,
        keys: ApiKeys,
        factory: Arc<dyn ProviderFactory>,
        retry: RetryPolicy,
    ) -> Arc<Engine> {
        let _ = db.seed_starter(crate::hebrew::STARTER_VERSION, &dictionary::starter_entries());
        Arc::new(Engine {
            db,
            data_dir,
            settings: RwLock::new(settings),
            keys: RwLock::new(keys),
            factory,
            retry,
            active: Mutex::new(HashSet::new()),
        })
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn set_settings(&self, s: Settings) {
        *self.settings.write().unwrap_or_else(|p| p.into_inner()) = s;
    }

    pub fn keys(&self) -> ApiKeys {
        self.keys.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn set_keys(&self, k: ApiKeys) {
        *self.keys.write().unwrap_or_else(|p| p.into_inner()) = k;
    }

    pub fn recordings_dir(&self) -> PathBuf {
        self.data_dir.join("recordings")
    }

    pub fn modes(&self) -> Vec<Mode> {
        let s = self.settings();
        modes::all_modes(&s.mode_overrides, &s.custom_modes)
    }

    /// Marks a dictation as being worked on for as long as the guard lives, so the queue
    /// never processes it twice — and is released even if the work is cancelled midway.
    fn claim(self: &Arc<Self>, id: &str) -> Option<ClaimGuard> {
        self.try_claim(id).then(|| ClaimGuard { engine: self.clone(), id: id.to_string() })
    }

    fn try_claim(&self, id: &str) -> bool {
        self.active.lock().unwrap_or_else(|p| p.into_inner()).insert(id.to_string())
    }

    fn release(&self, id: &str) {
        self.active.lock().unwrap_or_else(|p| p.into_inner()).remove(id);
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.active.lock().unwrap_or_else(|p| p.into_inner()).contains(id)
    }

    // ------------------------------------------------------------------ recording

    /// Starts a recording: the history row and the audio file exist before any audio arrives.
    pub fn start_session(
        self: &Arc<Self>,
        kind: RecordingKind,
        ctx: CaptureContext,
        device_rate: u32,
        channels: usize,
        rt: tokio::runtime::Handle,
    ) -> std::io::Result<Session> {
        let id = format!("{}-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"), &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let pcm = self.recordings_dir().join(format!("{id}.pcm"));
        let writer = PcmWriter::create(&pcm)?;
        let item = HistoryItem {
            id: id.clone(),
            created_at: now_ms(),
            kind: match kind {
                RecordingKind::Dictation => "dictation".into(),
                RecordingKind::Command => "command".into(),
            },
            status: Status::Recording.as_str().into(),
            app_key: Some(ctx.app.key.clone()).filter(|k| !k.is_empty()),
            app_name: Some(ctx.app.name.clone()).filter(|k| !k.is_empty()),
            audio_path: Some(pcm.to_string_lossy().into_owned()),
            selected_text: ctx.selection.clone(),
            meta: Some(json!({ "context": ctx })),
            ..Default::default()
        };
        self.try_claim(&id);
        if let Err(e) = self.db.insert_dictation(&item) {
            self.release(&id);
            return Err(std::io::Error::other(e));
        }
        // Live transcription connects in the background while the start sound plays; the
        // recording itself never waits for it.
        let live = self.live_setup(&ctx.app.name).map(|setup| LiveFeed::start(setup, device_rate, &rt));
        Ok(Session {
            id,
            kind,
            engine: self.clone(),
            rt,
            ctx,
            started: Instant::now(),
            writer: Some(writer),
            pcm,
            channels: channels.max(1),
            resampler: Resampler::new(device_rate, audio::SAMPLE_RATE),
            vad: Vad::default(),
            segmenter: Segmenter::new(SegmentConfig::default()),
            frame_buf: Vec::with_capacity(audio::FRAME_SAMPLES * 2),
            seg_samples: Vec::new(),
            seg_start: 0,
            level: LevelMeter::default(),
            jobs: Vec::new(),
            prev_text: Arc::new(Mutex::new(String::new())),
            write_error: None,
            live,
        })
    }

    /// Settings for live transcription of a new recording, if it is on and can be used.
    fn live_setup(&self, app_name: &str) -> Option<LiveSetup> {
        let settings = self.settings();
        let model = settings.providers.transcription_model.clone();
        if !settings.providers.live_transcription || !crate::providers::realtime::supports_live(&model) {
            return None;
        }
        let key = self.keys().openai.map(|k| k.trim().to_string()).filter(|k| !k.is_empty())?;
        // Paused after a recent failed connect or stall (bad network): use the normal path.
        let endpoint = self.factory.live_endpoint().filter(|e| !e.breaker.tripped())?;
        let entries = self.db.dict_all().unwrap_or_default();
        // The context of earlier pieces is carried by the live session itself.
        let app_name = Some(app_name).filter(|n| !n.is_empty());
        Some(LiveSetup {
            endpoint,
            key,
            session: crate::providers::realtime::SessionConfig {
                model,
                prompt: Some(prompts::transcription_prompt(None, app_name)),
                keywords: dictionary::keywords(&entries, KEYWORD_LIMIT),
                languages: settings.languages.clone(),
            },
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_segment_job(
        self: &Arc<Self>,
        rt: &tokio::runtime::Handle,
        id: &str,
        plan: SegmentPlan,
        samples: Vec<i16>,
        prev_text: Arc<Mutex<String>>,
        app_name: Option<String>,
        live: Option<Arc<LiveShared>>,
    ) -> JoinHandle<Result<String, ProviderError>> {
        let row = SegmentRow {
            dictation_id: id.to_string(),
            idx: plan.index as i64,
            start_sample: plan.start as i64,
            end_sample: plan.end as i64,
            has_speech: plan.has_speech,
            status: if plan.has_speech { "pending".into() } else { "skipped".into() },
            text: None,
            error: None,
        };
        let _ = self.db.upsert_segment(&row);
        let engine = self.clone();
        rt.spawn(async move {
            if !plan.has_speech {
                return Ok(String::new());
            }
            // Live transcription first, when it's running; otherwise (or if it fails or is
            // late) the normal way, from this piece's saved audio.
            let live_text = match &live {
                Some(l) => l.wait(plan.index).await,
                None => None,
            };
            let result = match live_text {
                Some(t) => Ok(t),
                None => {
                    let prev = prev_text.lock().unwrap_or_else(|p| p.into_inner()).clone();
                    engine.transcribe_samples(samples, Some(prev.as_str()), app_name.as_deref()).await
                }
            };
            let mut row = row;
            match &result {
                Ok(t) => {
                    row.status = "done".into();
                    row.text = Some(t.clone());
                    let mut p = prev_text.lock().unwrap_or_else(|p| p.into_inner());
                    if !p.is_empty() {
                        p.push(' ');
                    }
                    p.push_str(t);
                }
                Err(e) => {
                    row.status = "failed".into();
                    row.error = Some(e.message.clone());
                }
            }
            let _ = engine.db.upsert_segment(&row);
            result
        })
    }

    async fn transcribe_samples(&self, samples: Vec<i16>, prev: Option<&str>, app_name: Option<&str>) -> Result<String, ProviderError> {
        let audio = tokio::task::spawn_blocking(move || encode::encode_flac(&samples))
            .await
            .map_err(|e| ProviderError::new(ErrorKind::BadResponse, "Wisper", e.to_string()))?
            .map_err(|e| ProviderError::new(ErrorKind::BadResponse, "Wisper", e.to_string()))?;
        let settings = self.settings();
        let entries = self.db.dict_all().unwrap_or_default();
        let req = TranscribeRequest {
            audio,
            filename: "speech.flac".into(),
            mime: "audio/flac".into(),
            model: settings.providers.transcription_model.clone(),
            prompt: Some(prompts::transcription_prompt(prev, app_name)),
            keywords: dictionary::keywords(&entries, KEYWORD_LIMIT),
            languages: settings.languages.clone(),
        };
        let transcriber = self.factory.transcriber(&self.keys());
        if self.keys().openai.as_deref().map(str::trim).unwrap_or("").is_empty() {
            return Err(ProviderError::new(ErrorKind::Auth, "OpenAI", "no OpenAI API key set"));
        }
        let t = self.retry.run(|| transcriber.transcribe(req.clone())).await?;
        Ok(t.text)
    }

    // ------------------------------------------------------------------ processing

    /// Finishes a recording: waits for the segment transcriptions, then cleans up the text.
    pub async fn process(self: &Arc<Self>, rec: FinishedRecording, progress: &(dyn Fn(Stage) + Send + Sync)) -> Result<Outcome, PipelineError> {
        // The session claimed the id when it started; this guard releases it however we exit
        // (including when the caller gives up waiting and drops this future).
        let _guard = ClaimGuard { engine: self.clone(), id: rec.id.clone() };
        self.process_inner(rec, progress).await
    }

    async fn process_inner(self: &Arc<Self>, rec: FinishedRecording, progress: &(dyn Fn(Stage) + Send + Sync)) -> Result<Outcome, PipelineError> {
        let id = rec.id.clone();
        let stop_at = Instant::now();
        if !rec.heard_speech {
            let _ = self.db.set_status(&id, Status::Empty);
            self.archive_audio_later(&id, &rec.pcm);
            return Ok(Outcome::NoSpeech { id });
        }
        let _ = self.db.set_status(&id, Status::Transcribing);
        let total = rec.jobs.len();
        let mut texts = Vec::with_capacity(total);
        let mut first_err: Option<ProviderError> = None;
        for (i, job) in rec.jobs.into_iter().enumerate() {
            progress(Stage::Transcribing { done: i, total });
            match job.await {
                Ok(Ok(t)) => texts.push(t),
                Ok(Err(e)) => {
                    first_err.get_or_insert(e);
                }
                Err(join) => {
                    first_err.get_or_insert(ProviderError::new(ErrorKind::BadResponse, "Wisper", join.to_string()));
                }
            }
        }
        if let Some(e) = first_err {
            return Err(self.fail(&id, e));
        }
        let transcribe_wait = stop_at.elapsed().as_millis() as u64;
        let raw = join_segments(&texts);
        let _ = self.db.set_raw_text(&id, &raw);
        if raw.trim().is_empty() {
            let _ = self.db.set_status(&id, Status::Empty);
            self.archive_audio_later(&id, &rec.pcm);
            return Ok(Outcome::NoSpeech { id });
        }
        progress(Stage::Cleaning);
        let _ = self.db.set_status(&id, Status::Cleaning);
        let out = self.finish_text(&id, rec.kind, &raw, &rec.ctx, None, rec.duration_ms, transcribe_wait, stop_at).await;
        if out.is_ok() {
            let _ = self.db.merge_timings(&id, &crate::live::timings_json(rec.live.as_deref()));
        }
        // Compress the recording after the text is ready, so pasting never waits for it.
        if out.is_ok() {
            self.archive_audio_later(&id, &rec.pcm);
        }
        out
    }

    /// Turns a transcript into final text and stores the result.
    #[allow(clippy::too_many_arguments)]
    async fn finish_text(
        self: &Arc<Self>,
        id: &str,
        kind: RecordingKind,
        raw: &str,
        ctx: &CaptureContext,
        forced_mode: Option<&str>,
        duration_ms: u64,
        transcribe_wait_ms: u64,
        stop_at: Instant,
    ) -> Result<Outcome, PipelineError> {
        let clean_start = Instant::now();
        let processed = match kind {
            // "Redo as" (a forced mode) and recordings of unknown length use the normal model.
            RecordingKind::Dictation => self.clean_text_for(raw, ctx, forced_mode, Some(duration_ms).filter(|_| forced_mode.is_none())).await,
            RecordingKind::Command => match self.run_instruction(raw, ctx.selection.as_deref(), ctx).await {
                Ok(t) => CleanResult { text: t.clone(), clean: Some(t), mode_id: "command".into(), mode_name: "Command".into(), fixes: 0, error: None, model: Some(cleanup_model(&self.settings())), fast: false },
                Err(e) => return Err(self.fail(id, e)),
            },
        };
        let timings = json!({
            "recordingMs": duration_ms,
            "transcribeWaitMs": transcribe_wait_ms,
            "cleanupMs": clean_start.elapsed().as_millis() as u64,
            "totalAfterStopMs": stop_at.elapsed().as_millis() as u64,
            // Which model cleaned this up, so History and the Speed card can show it.
            "cleanupModel": processed.model,
            "cleanupFast": processed.fast,
        });
        let settings = self.settings();
        let meta = json!({
            "context": ctx,
            "transcriptionModel": settings.providers.transcription_model,
            "cleanupProvider": settings.providers.cleanup_provider,
            "cleanupModel": processed.model.clone().unwrap_or_else(|| cleanup_model(&settings)),
            "cleanupFast": processed.fast,
            "cleanupWarning": processed.error.as_ref().map(|e| e.user_message()),
        });
        let _ = self.db.set_result(id, raw, processed.clean.as_deref(), &processed.text, &processed.mode_id, processed.fixes, &timings, &meta);
        let entries = self.db.dict_all().unwrap_or_default();
        let _ = self.db.dict_bump_uses(&dictionary::count_uses(&processed.text, &entries));
        Ok(Outcome::Text(Processed {
            id: id.to_string(),
            raw_text: raw.to_string(),
            final_text: processed.text,
            mode_id: processed.mode_id,
            mode_name: processed.mode_name,
            cleanup_warning: processed.error.as_ref().map(|e| e.user_message()),
            cleanup_error_kind: processed.error.map(|e| e.kind),
        }))
    }

    fn fail(&self, id: &str, e: ProviderError) -> PipelineError {
        let kind_str = serde_json::to_value(e.kind).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        if e.is_retryable() {
            let _ = self.db.set_error(id, Status::Queued, &kind_str, &e.user_message());
            PipelineError::Queued { id: id.into(), message: e.user_message(), kind: e.kind }
        } else {
            let _ = self.db.set_error(id, Status::Failed, &kind_str, &e.user_message());
            PipelineError::Failed { id: id.into(), message: e.user_message(), kind: Some(e.kind) }
        }
    }

    /// Picks the mode: spoken switch, then per-app style, then learned style, then the app's
    /// natural mode when the default is Auto, then the default.
    pub fn choose_mode(&self, text: &str, app: &AppInfo, forced: Option<&str>) -> (Mode, String) {
        let modes = self.modes();
        let settings = self.settings();
        let fallback = || modes::find(&modes, modes::AUTO).cloned().expect("auto mode");
        if let Some(f) = forced {
            if let Some(m) = modes::find(&modes, f) {
                return (m.clone(), text.to_string());
            }
        }
        if let Some((id, rest)) = modes::detect_spoken_switch(text, &modes) {
            if let Some(m) = modes::find(&modes, &id) {
                if !app.key.is_empty() && settings.learn_app_styles {
                    let _ = self.db.record_mode_choice(&app.key, &id);
                }
                return (m.clone(), rest);
            }
        }
        if !app.key.is_empty() {
            if let Some(style) = settings.mode_for_app(&app.key) {
                if let Some(m) = modes::find(&modes, &style.mode_id) {
                    return (m.clone(), text.to_string());
                }
            }
            if settings.learn_app_styles {
                if let Ok(Some(learned)) = self.db.learned_mode(&app.key, 3) {
                    if let Some(m) = modes::find(&modes, &learned) {
                        return (m.clone(), text.to_string());
                    }
                }
            }
        }
        if settings.default_mode == modes::AUTO {
            if let Some(cat) = app.category {
                if let Some(m) = modes::find(&modes, cat.default_mode()) {
                    return (m.clone(), text.to_string());
                }
            }
        }
        let m = modes::find(&modes, &settings.default_mode).cloned().unwrap_or_else(fallback);
        (m, text.to_string())
    }

    /// Cleanup. Never fails: if the AI step can't run, returns the transcript with the
    /// deterministic fixes applied and the reason in `error`.
    pub async fn clean_text(&self, raw: &str, ctx: &CaptureContext, forced_mode: Option<&str>) -> CleanResult {
        self.clean_text_for(raw, ctx, forced_mode, None).await
    }

    /// Like `clean_text`; `duration_ms` is the recording length, which lets short dictations
    /// use the fast model (see `dictation_model`). None = always the normal model.
    pub async fn clean_text_for(&self, raw: &str, ctx: &CaptureContext, forced_mode: Option<&str>, duration_ms: Option<u64>) -> CleanResult {
        let settings = self.settings();
        let (mode, text) = self.choose_mode(raw, &ctx.app, forced_mode);
        let entries = self.db.dict_all().unwrap_or_default();
        let snips = self.db.snippets_all().unwrap_or_default();
        let (with_ph, ph_map) = snippets::insert_placeholders(&text, &snips);
        for (_, _, sid) in &ph_map {
            let _ = self.db.snippet_bump(*sid);
        }
        let (fixed, mut fixes) = dictionary::apply_replacements(&with_ph, &entries);

        let excluded = settings.context_excluded_apps.iter().any(|a| a.eq_ignore_ascii_case(&ctx.app.exe));
        let use_ctx = settings.context_awareness && !excluded;
        let before = if use_ctx { ctx.text_before.clone() } else { None };
        let after = if use_ctx { ctx.text_after.clone() } else { None };

        // Tiny dictations with nothing for the AI to fix are tidied right here (instant).
        // Uses the raw text so a spoken mode switch is still noticed and declined.
        if settings.providers.fast_cleanup.skip_ai_for_tiny && forced_mode.is_none() && settings.cleanup_level != CleanupLevel::None {
            let qc = crate::quick::QuickContext::new(&entries, &snips, &self.modes(), &mode, ctx.app.category, settings.cleanup_level)
                .with_text_before(before.as_deref());
            if let Some(t) = crate::quick::quick_clean(raw, &qc) {
                let final_text = text::fit_to_context(&t, before.as_deref(), after.as_deref());
                return CleanResult { text: final_text, clean: Some(t), mode_id: mode.id.clone(), mode_name: mode.name.clone(), fixes: 0, error: None, model: Some("local".into()), fast: true };
            }
        }

        let deterministic = |s: &str| -> String {
            let s = if settings.smart_formatting { text::apply_spoken_line_breaks(s) } else { s.to_string() };
            text::capitalise_first(s.trim())
        };

        let (mut model_id, mut fast) = dictation_model(&settings, duration_ms);
        let llm_used = settings.cleanup_level != CleanupLevel::None;
        let (body, clean, error) = if !llm_used {
            (deterministic(&fixed), None, None)
        } else {
            let mut r = self.llm_cleanup(&fixed, &mode, settings.cleanup_level, ctx, before.clone(), use_ctx, &entries, &model_id).await;
            // The fast model may be unavailable on this account (retired, renamed, no access):
            // try the normal model once before giving up on AI cleanup.
            if fast && matches!(&r, Err(e) if e.kind == ErrorKind::BadRequest) {
                tracing::warn!("fast cleanup model {model_id} failed ({}); retrying with the normal model", r.as_ref().err().map(|e| e.message.as_str()).unwrap_or(""));
                model_id = cleanup_model(&settings);
                fast = false;
                r = self.llm_cleanup(&fixed, &mode, settings.cleanup_level, ctx, before.clone(), use_ctx, &entries, &model_id).await;
            }
            match r {
                Ok(t) => {
                    let (t2, more) = dictionary::apply_replacements(&t, &entries);
                    fixes += more;
                    (t2.clone(), Some(t2), None)
                }
                Err(e) => {
                    tracing::warn!("cleanup failed, using transcript: {e}");
                    (deterministic(&fixed), None, Some(e))
                }
            }
        };
        let (restored, _) = snippets::restore_placeholders(&body, &ph_map);
        let final_text = text::fit_to_context(&restored, before.as_deref(), after.as_deref());
        CleanResult { text: final_text, clean, mode_id: mode.id.clone(), mode_name: mode.name.clone(), fixes: fixes as i64, error, model: llm_used.then_some(model_id), fast: llm_used && fast }
    }

    #[allow(clippy::too_many_arguments)]
    async fn llm_cleanup(
        &self,
        text_in: &str,
        mode: &Mode,
        level: CleanupLevel,
        ctx: &CaptureContext,
        before: Option<String>,
        use_ctx: bool,
        entries: &[dictionary::DictEntry],
        model_id: &str,
    ) -> Result<String, ProviderError> {
        let settings = self.settings();
        let model = self.factory.text_model(&self.keys(), settings.providers.cleanup_provider);
        let system = vec![
            SystemBlock { text: prompts::base_system_block(&dictionary::prompt_listing(entries)), cache: true },
            SystemBlock { text: prompts::mode_block(mode, level), cache: true },
        ];
        let base_ctx = CleanupContext {
            app_name: Some(ctx.app.name.clone()).filter(|n| !n.is_empty()),
            category: ctx.app.category,
            window_title: if use_ctx { Some(ctx.app.title.clone()).filter(|t| !t.is_empty()) } else { None },
            text_before_cursor: before,
            previous_part: None,
            part: None,
        };
        let chunks = if text::word_count(text_in) > CLEANUP_SPLIT_THRESHOLD {
            text::split_for_cleanup(text_in, CLEANUP_CHUNK_WORDS)
        } else {
            vec![text_in.to_string()]
        };
        let n = chunks.len();
        let mut futures = Vec::with_capacity(n);
        for (i, chunk) in chunks.iter().enumerate() {
            let mut c = base_ctx.clone();
            if n > 1 {
                c.part = Some((i + 1, n));
                if i > 0 {
                    c.previous_part = Some(chunks[i - 1].clone());
                    c.text_before_cursor = None;
                }
            }
            let req = TextRequest {
                model: model_id.to_string(),
                system: system.clone(),
                user: prompts::cleanup_user_message(chunk, &c),
                max_tokens: max_tokens_for(chunk),
            };
            let model = model.clone();
            let retry = self.retry;
            futures.push(async move { retry.run(|| model.complete(req.clone())).await });
        }
        let results = futures_join_all(futures).await;
        let mut parts = Vec::with_capacity(n);
        for (r, chunk) in results.into_iter().zip(chunks.iter()) {
            let resp = r?;
            let out = text::sanitize_model_output(&resp.text, chunk);
            if !looks_like_cleanup(chunk, &out) {
                return Err(ProviderError::new(
                    ErrorKind::BadResponse,
                    model.provider_name(),
                    "the AI's reply didn't look like a tidied version of what you said (it may have answered it instead)",
                ));
            }
            parts.push(out);
        }
        Ok(parts.join(if n > 1 { "\n\n" } else { "" }))
    }

    /// Command mode / transforms: apply an instruction to the selected text.
    pub async fn run_instruction(&self, instruction: &str, selection: Option<&str>, ctx: &CaptureContext) -> Result<String, ProviderError> {
        let settings = self.settings();
        let entries = self.db.dict_all().unwrap_or_default();
        let user_words: Vec<dictionary::DictEntry> = entries.iter().filter(|e| e.uses > 0 || e.source != dictionary::EntrySource::Builtin).cloned().collect();
        let model = self.factory.text_model(&self.keys(), settings.providers.cleanup_provider);
        let c = CleanupContext { app_name: Some(ctx.app.name.clone()).filter(|n| !n.is_empty()), ..Default::default() };
        let req = TextRequest {
            model: cleanup_model(&settings),
            system: vec![SystemBlock { text: prompts::command_system_block(&dictionary::prompt_listing(&user_words)), cache: true }],
            user: prompts::command_user_message(selection, instruction, &c),
            max_tokens: max_tokens_for(selection.unwrap_or("")).max(2048),
        };
        let resp = self.retry.run(|| model.complete(req.clone())).await?;
        // No automatic spelling rewrites here: the selection may be someone else's text.
        Ok(text::sanitize_model_output(&resp.text, selection.unwrap_or("")))
    }

    /// Records a transform/command that didn't involve a recording (history + stats).
    pub fn record_transform(&self, name: &str, selection: &str, result: &str, app: &AppInfo) {
        let item = HistoryItem {
            id: format!("t-{}", uuid::Uuid::new_v4().simple()),
            created_at: now_ms(),
            kind: "transform".into(),
            status: "done".into(),
            mode_id: Some(name.to_string()),
            app_key: Some(app.key.clone()).filter(|k| !k.is_empty()),
            app_name: Some(app.name.clone()).filter(|k| !k.is_empty()),
            selected_text: Some(selection.to_string()),
            final_text: Some(result.to_string()),
            word_count: text::word_count(result) as i64,
            ..Default::default()
        };
        let _ = self.db.insert_dictation(&item);
    }

    // ------------------------------------------------------------------ history actions

    /// Re-runs cleanup of a saved dictation in another mode (no new recording).
    pub async fn reprocess(self: &Arc<Self>, id: &str, mode_id: &str) -> Result<Processed, PipelineError> {
        let item = self
            .db
            .get(id)
            .ok()
            .flatten()
            .ok_or_else(|| PipelineError::Failed { id: id.into(), message: "That dictation no longer exists.".into(), kind: None })?;
        let raw = item.raw_text.clone().unwrap_or_default();
        if raw.trim().is_empty() {
            // Never transcribed: do the whole thing from the audio.
            return match self.retry_dictation(id, Some(mode_id)).await? {
                Outcome::Text(p) => Ok(p),
                Outcome::NoSpeech { id } => Err(PipelineError::Failed { id, message: "No speech was found in that recording.".into(), kind: None }),
            };
        }
        let ctx = context_from_item(&item);
        if let Some(key) = &item.app_key {
            if self.settings().learn_app_styles {
                let _ = self.db.record_mode_choice(key, mode_id);
            }
        }
        match self.finish_text(id, RecordingKind::Dictation, &raw, &ctx, Some(mode_id), item.duration_ms as u64, 0, Instant::now()).await? {
            Outcome::Text(p) => Ok(p),
            Outcome::NoSpeech { id } => Err(PipelineError::Failed { id, message: "Nothing to process.".into(), kind: None }),
        }
    }

    /// Re-transcribes a saved dictation from its audio (failed, queued, or crashed mid-way).
    /// Segments that already have text are reused.
    pub async fn retry_dictation(self: &Arc<Self>, id: &str, forced_mode: Option<&str>) -> Result<Outcome, PipelineError> {
        self.retry_dictation_with(id, forced_mode, false).await
    }

    /// `force_all`: also transcribe pieces the speech detector judged silent (the user asked
    /// for this one explicitly, e.g. very quiet speech).
    pub async fn retry_dictation_with(self: &Arc<Self>, id: &str, forced_mode: Option<&str>, force_all: bool) -> Result<Outcome, PipelineError> {
        let Some(_guard) = self.claim(id) else {
            return Err(PipelineError::Queued { id: id.into(), message: "Already being processed.".into(), kind: ErrorKind::Server });
        };
        self.retry_inner(id, forced_mode, force_all).await
    }

    async fn retry_inner(self: &Arc<Self>, id: &str, forced_mode: Option<&str>, force_all: bool) -> Result<Outcome, PipelineError> {
        let missing = || PipelineError::Failed { id: id.into(), message: "That dictation no longer exists.".into(), kind: None };
        let item = self.db.get(id).ok().flatten().ok_or_else(missing)?;
        let ctx = context_from_item(&item);
        let kind = if item.kind == "command" { RecordingKind::Command } else { RecordingKind::Dictation };
        let started = Instant::now();
        if let Some(raw) = item.raw_text.clone().filter(|r| !r.trim().is_empty()) {
            let _ = self.db.clear_error(id);
            return self.finish_text(id, kind, &raw, &ctx, forced_mode, item.duration_ms as u64, 0, started).await;
        }
        let audio = item.audio_path.clone().map(PathBuf::from).filter(|p| p.exists()).ok_or_else(|| PipelineError::Failed {
            id: id.into(),
            message: "The audio for this dictation has been deleted, so it can't be processed again.".into(),
            kind: None,
        })?;
        let samples_total = if audio.extension().map(|e| e == "pcm").unwrap_or(false) {
            audio::pcm_file::pcm_len(&audio).unwrap_or(0)
        } else {
            decode_flac_len(&audio).unwrap_or(0)
        };
        if item.duration_ms == 0 && samples_total > 0 {
            let _ = self.db.set_recording_info(id, audio::samples_to_ms(samples_total) as i64, item.audio_path.as_deref());
        }
        let mut segs = self.db.segments(id).unwrap_or_default();
        let covered = segs.last().map(|s| s.end_sample as usize).unwrap_or(0);
        if segs.is_empty() || covered < samples_total {
            // Crashed mid-recording or never segmented: segment the rest of the file now.
            let samples = read_all_samples(&audio).map_err(|e| PipelineError::Failed { id: id.into(), message: format!("Couldn't read the saved audio: {e}"), kind: None })?;
            let plans = segment_offline(&samples[covered.min(samples.len())..], covered, segs.len());
            for p in plans {
                let row = SegmentRow {
                    dictation_id: id.into(),
                    idx: p.index as i64,
                    start_sample: p.start as i64,
                    end_sample: p.end as i64,
                    has_speech: p.has_speech,
                    status: if p.has_speech { "pending".into() } else { "skipped".into() },
                    text: None,
                    error: None,
                };
                let _ = self.db.upsert_segment(&row);
            }
            segs = self.db.segments(id).unwrap_or_default();
        }
        let _ = self.db.set_status(id, Status::Transcribing);
        let all = read_all_samples(&audio).map_err(|e| PipelineError::Failed { id: id.into(), message: format!("Couldn't read the saved audio: {e}"), kind: None })?;
        let mut texts = Vec::new();
        let mut prev = String::new();
        for mut s in segs {
            if s.status == "done" {
                let t = s.text.clone().unwrap_or_default();
                prev.push(' ');
                prev.push_str(&t);
                texts.push(t);
                continue;
            }
            if !s.has_speech && !force_all {
                continue;
            }
            let a = (s.start_sample as usize).min(all.len());
            let b = (s.end_sample as usize).min(all.len());
            match self.transcribe_samples(all[a..b].to_vec(), Some(prev.trim()), ctx.app.name.as_str().into()).await {
                Ok(t) => {
                    s.status = "done".into();
                    s.text = Some(t.clone());
                    s.error = None;
                    let _ = self.db.upsert_segment(&s);
                    prev.push(' ');
                    prev.push_str(&t);
                    texts.push(t);
                }
                Err(e) => {
                    s.status = "failed".into();
                    s.error = Some(e.message.clone());
                    let _ = self.db.upsert_segment(&s);
                    return Err(self.fail(id, e));
                }
            }
        }
        let raw = join_segments(&texts);
        let _ = self.db.set_raw_text(id, &raw);
        if raw.trim().is_empty() {
            let _ = self.db.set_status(id, Status::Empty);
            return Ok(Outcome::NoSpeech { id: id.into() });
        }
        let _ = self.db.set_status(id, Status::Cleaning);
        let out = self.finish_text(id, kind, &raw, &ctx, forced_mode, item.duration_ms as u64, started.elapsed().as_millis() as u64, started).await;
        if out.is_ok() && audio.extension().map(|e| e == "pcm").unwrap_or(false) {
            self.archive_audio_later(id, &audio);
        }
        out
    }

    /// Processes everything left unfinished (offline queue, crashes). Returns what happened
    /// to each, for notifications.
    pub async fn process_queue(self: &Arc<Self>) -> Vec<(String, Result<Outcome, PipelineError>)> {
        let items = self.db.unfinished().unwrap_or_default();
        let mut out = Vec::new();
        for it in items {
            if self.is_active(&it.id) {
                continue;
            }
            let r = self.retry_dictation(&it.id, None).await;
            let stop = matches!(&r, Err(PipelineError::Queued { kind, .. }) if matches!(kind, ErrorKind::Network | ErrorKind::Timeout));
            out.push((it.id.clone(), r));
            if stop {
                break; // still offline; try again later
            }
        }
        out
    }

    /// Re-queues dictations that failed because of a key problem (after the user fixes keys).
    pub fn requeue_auth_failures(&self) -> usize {
        let items = self.db.list(&crate::db::HistoryQuery { status: Some("failed".into()), limit: Some(200), ..Default::default() }).unwrap_or_default();
        let mut n = 0;
        let cutoff = now_ms() - 7 * 24 * 3600 * 1000;
        for it in items {
            if it.created_at > cutoff && matches!(it.error_kind.as_deref(), Some("auth") | Some("quota")) {
                let _ = self.db.set_status(&it.id, Status::Queued);
                n += 1;
            }
        }
        n
    }

    fn archive_audio_later(self: &Arc<Self>, id: &str, pcm: &Path) {
        let (me, id, pcm) = (self.clone(), id.to_string(), pcm.to_path_buf());
        tokio::task::spawn_blocking(move || me.archive_audio(&id, &pcm));
    }

    /// Converts the raw recording to FLAC and records the new path.
    pub fn archive_audio(&self, id: &str, pcm: &Path) {
        if !pcm.exists() {
            return;
        }
        let flac = pcm.with_extension("flac");
        match encode::pcm_file_to_flac(pcm, &flac) {
            Ok(_) => {
                let _ = self.db.set_audio_path(id, Some(&flac.to_string_lossy()));
                let _ = std::fs::remove_file(pcm);
            }
            Err(e) => tracing::warn!("couldn't compress recording {id}: {e} (keeping the raw file)"),
        }
    }

    /// Deletes old audio/history according to the retention settings.
    pub fn apply_retention(&self) -> (usize, usize) {
        let s = self.settings();
        let mut audio_removed = 0;
        let mut items_removed = 0;
        if let Some(days) = s.keep_audio_days {
            let cutoff = now_ms() - days as i64 * 86_400_000;
            for (id, path) in self.db.audio_older_than(cutoff).unwrap_or_default() {
                let _ = std::fs::remove_file(&path);
                let _ = self.db.set_audio_path(&id, None);
                audio_removed += 1;
            }
        }
        if let Some(days) = s.keep_history_days {
            let cutoff = now_ms() - days as i64 * 86_400_000;
            for id in self.db.ids_older_than(cutoff).unwrap_or_default() {
                if let Ok(Some(path)) = self.db.delete(&id) {
                    let _ = std::fs::remove_file(path);
                }
                items_removed += 1;
            }
        }
        (audio_removed, items_removed)
    }

    pub fn delete_dictation(&self, id: &str) -> bool {
        if self.is_active(id) {
            return false;
        }
        if let Ok(Some(path)) = self.db.delete(id) {
            let _ = std::fs::remove_file(path);
        }
        true
    }

    /// Looks for inconsistent spellings across recent dictations and stores new suggestions.
    pub fn refresh_suggestions(&self) -> Vec<dictionary::Suggestion> {
        let texts = self.db.recent_final_texts(300).unwrap_or_default();
        let entries = self.db.dict_all().unwrap_or_default();
        let mut new = Vec::new();
        for s in dictionary::find_suggestions(&texts, &entries) {
            if self.db.suggestion_add(&s).unwrap_or(false) {
                new.push(s);
            }
        }
        new
    }
}

pub struct ClaimGuard {
    engine: Arc<Engine>,
    id: String,
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        self.engine.release(&self.id);
    }
}

pub struct CleanResult {
    pub text: String,
    pub clean: Option<String>,
    pub mode_id: String,
    pub mode_name: String,
    pub fixes: i64,
    pub error: Option<ProviderError>,
    /// The cleanup model that was asked, when the AI step ran.
    pub model: Option<String>,
    /// Whether that was the fast model for short dictations.
    pub fast: bool,
}

/// Guards against the model answering or rewriting instead of cleaning: the cleaned text
/// should be roughly as long as what was said (fillers removed, formatting added).
pub fn looks_like_cleanup(input: &str, output: &str) -> bool {
    let i = text::word_count(input);
    let o = text::word_count(output);
    if i == 0 {
        return o == 0;
    }
    let too_long = o > (i as f64 * 1.6) as usize + 8;
    let too_short = i >= 12 && o * 3 < i;
    !(too_long || too_short)
}

fn cleanup_model(s: &Settings) -> String {
    match s.providers.cleanup_provider {
        CleanupProvider::Anthropic => s.providers.anthropic_model.clone(),
        CleanupProvider::OpenAi => s.providers.openai_cleanup_model.clone(),
    }
}

/// Picks the model for dictation cleanup: the fast model of the chosen provider when fast
/// cleanup is on and the recording is shorter than the limit, otherwise the normal model.
/// `duration_ms` is None when the length is unknown (always the normal model). Voice commands,
/// transforms and "redo as" don't go through here; they always use `cleanup_model`.
/// Returns the model id and whether it is the fast one.
pub(crate) fn dictation_model(s: &Settings, duration_ms: Option<u64>) -> (String, bool) {
    let f = &s.providers.fast_cleanup;
    let short = duration_ms.map(|d| d > 0 && d < f.max_seconds as u64 * 1000).unwrap_or(false);
    if f.enabled && short {
        let fast = match s.providers.cleanup_provider {
            CleanupProvider::Anthropic => &f.anthropic_model,
            CleanupProvider::OpenAi => &f.openai_model,
        };
        if !fast.trim().is_empty() {
            return (fast.clone(), true);
        }
    }
    (cleanup_model(s), false)
}

fn max_tokens_for(text: &str) -> u32 {
    let words = text::word_count(text) as u32;
    (words * 3 + 600).clamp(1024, 16_000)
}

fn join_segments(texts: &[String]) -> String {
    texts.iter().map(|t| t.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ")
}

fn context_from_item(item: &HistoryItem) -> CaptureContext {
    item.meta
        .as_ref()
        .and_then(|m| m.get("context"))
        .and_then(|c| serde_json::from_value::<CaptureContext>(c.clone()).ok())
        .unwrap_or_else(|| CaptureContext {
            app: AppInfo { key: item.app_key.clone().unwrap_or_default(), name: item.app_name.clone().unwrap_or_default(), ..Default::default() },
            selection: item.selected_text.clone(),
            ..Default::default()
        })
}

async fn futures_join_all<F: std::future::Future>(futs: Vec<F>) -> Vec<F::Output> {
    // Small helper to avoid pulling in the futures crate: run all concurrently.
    let mut handles = Vec::with_capacity(futs.len());
    let mut outputs: Vec<Option<F::Output>> = Vec::new();
    for f in futs {
        handles.push(Box::pin(f));
        outputs.push(None);
    }
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (i, h) in handles.iter_mut().enumerate() {
            if outputs[i].is_none() {
                match h.as_mut().poll(cx) {
                    std::task::Poll::Ready(v) => outputs[i] = Some(v),
                    std::task::Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
    .await;
    outputs.into_iter().map(|o| o.expect("all futures finished")).collect()
}

fn read_all_samples(path: &Path) -> std::io::Result<Vec<i16>> {
    if path.extension().map(|e| e == "pcm").unwrap_or(false) {
        let bytes = std::fs::read(path)?;
        Ok(bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect())
    } else {
        decode_flac(path)
    }
}

fn decode_flac(path: &Path) -> std::io::Result<Vec<i16>> {
    let mut reader = claxon::FlacReader::open(path).map_err(std::io::Error::other)?;
    let mut out = Vec::new();
    for s in reader.samples() {
        out.push(s.map_err(std::io::Error::other)? as i16);
    }
    Ok(out)
}

fn decode_flac_len(path: &Path) -> std::io::Result<usize> {
    let reader = claxon::FlacReader::open(path).map_err(std::io::Error::other)?;
    Ok(reader.streaminfo().samples.unwrap_or(0) as usize)
}

/// Segments already-recorded audio (crash recovery / retries).
fn segment_offline(samples: &[i16], offset: usize, first_index: usize) -> Vec<SegmentPlan> {
    let mut vad = Vad::default();
    let mut seg = Segmenter::new(SegmentConfig::default());
    let mut plans = Vec::new();
    let mut frame = vec![0f32; audio::FRAME_SAMPLES];
    for chunk in samples.chunks(audio::FRAME_SAMPLES) {
        for (i, s) in chunk.iter().enumerate() {
            frame[i] = *s as f32 / 32768.0;
        }
        if chunk.len() < audio::FRAME_SAMPLES {
            break;
        }
        let info = vad.process_frame(&frame);
        if let Some(p) = seg.push_frame(info) {
            plans.push(p);
        }
    }
    if let Some(p) = seg.finish() {
        plans.push(p);
    }
    // The tail that didn't fill a whole frame belongs to the last segment.
    if let Some(last) = plans.last_mut() {
        last.end = samples.len();
    }
    plans
        .into_iter()
        .map(|mut p| {
            p.index += first_index;
            p.start += offset;
            p.end += offset;
            p
        })
        .collect()
}

/// An in-progress recording. Lives on the audio thread; cheap per call.
pub struct Session {
    pub id: String,
    pub kind: RecordingKind,
    engine: Arc<Engine>,
    rt: tokio::runtime::Handle,
    pub ctx: CaptureContext,
    started: Instant,
    writer: Option<PcmWriter>,
    pcm: PathBuf,
    channels: usize,
    resampler: Resampler,
    vad: Vad,
    segmenter: Segmenter,
    frame_buf: Vec<f32>,
    /// Samples of the current (open) segment.
    seg_samples: Vec<i16>,
    seg_start: usize,
    level: LevelMeter,
    jobs: Vec<JoinHandle<Result<String, ProviderError>>>,
    prev_text: Arc<Mutex<String>>,
    write_error: Option<String>,
    /// Live transcription of this recording, when on.
    live: Option<LiveFeed>,
}

pub struct FinishedRecording {
    pub id: String,
    pub kind: RecordingKind,
    pub ctx: CaptureContext,
    pub duration_ms: u64,
    pub heard_speech: bool,
    pub pcm: PathBuf,
    jobs: Vec<JoinHandle<Result<String, ProviderError>>>,
    live: Option<Arc<LiveShared>>,
}

impl Session {
    /// Feeds captured audio (interleaved, at the device rate). Returns the display level 0..1.
    pub fn push(&mut self, interleaved: &[f32]) -> f32 {
        let mono = audio::downmix(interleaved, self.channels);
        let level = self.level.update(&mono);
        if let Some(l) = self.live.as_mut() {
            l.audio(&mono);
        }
        let resampled = self.resampler.process(&mono);
        if resampled.is_empty() {
            return level;
        }
        let pcm: Vec<i16> = resampled.iter().map(|&x| audio::f32_to_i16(x)).collect();
        if let Some(w) = self.writer.as_mut() {
            if let Err(e) = w.append(&pcm) {
                // Keep going in memory; the recording still gets transcribed.
                self.write_error.get_or_insert_with(|| e.to_string());
            }
        }
        self.seg_samples.extend_from_slice(&pcm);
        self.frame_buf.extend_from_slice(&resampled);
        while self.frame_buf.len() >= audio::FRAME_SAMPLES {
            let frame: Vec<f32> = self.frame_buf.drain(..audio::FRAME_SAMPLES).collect();
            let info = self.vad.process_frame(&frame);
            if let Some(plan) = self.segmenter.push_frame(info) {
                self.dispatch(plan);
            }
        }
        if let Some(l) = self.live.as_mut() {
            // Stream everything that can no longer move to another piece.
            l.release_upto(self.segmenter.safe_point());
        }
        level
    }

    fn dispatch(&mut self, plan: SegmentPlan) {
        let take = plan.end.saturating_sub(self.seg_start).min(self.seg_samples.len());
        let samples: Vec<i16> = self.seg_samples.drain(..take).collect();
        self.seg_start = plan.end;
        if let Some(l) = self.live.as_mut() {
            l.segment(&plan);
        }
        let job = self.engine.spawn_segment_job(
            &self.rt,
            &self.id,
            plan,
            samples,
            self.prev_text.clone(),
            Some(self.ctx.app.name.clone()).filter(|n| !n.is_empty()),
            self.live.as_ref().map(|l| l.shared()),
        );
        self.jobs.push(job);
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub fn recorded_ms(&self) -> u64 {
        audio::samples_to_ms(self.segmenter.samples())
    }

    pub fn heard_speech(&self) -> bool {
        self.segmenter.heard_speech()
    }

    pub fn segments_sent(&self) -> usize {
        self.jobs.len()
    }

    /// The recording became a command (e.g. Alt added to Ctrl+Win) or vice versa.
    pub fn set_kind(&mut self, kind: RecordingKind) {
        self.kind = kind;
        let k = match kind {
            RecordingKind::Dictation => "dictation",
            RecordingKind::Command => "command",
        };
        let _ = self.engine.db.set_kind(&self.id, k);
    }

    /// Stops recording: closes the last segment and returns everything needed to process.
    pub fn finish(mut self) -> FinishedRecording {
        // Flush a partial frame into the segmenter as silence-padded audio.
        if !self.frame_buf.is_empty() {
            let mut frame = std::mem::take(&mut self.frame_buf);
            frame.resize(audio::FRAME_SAMPLES, 0.0);
            let info = self.vad.process_frame(&frame);
            if let Some(plan) = self.segmenter.push_frame(info) {
                self.dispatch(plan);
            }
        }
        if let Some(mut plan) = self.segmenter.finish() {
            plan.end = self.seg_start + self.seg_samples.len();
            if plan.end > plan.start {
                self.dispatch(plan);
            }
        }
        let live = self.live.take().map(|l| l.finish());
        let duration_ms = self.recorded_ms();
        if let Some(w) = self.writer.take() {
            if let Err(e) = w.finish() {
                tracing::warn!("recording file: {e}");
            }
        }
        if let Some(e) = &self.write_error {
            tracing::warn!("recording file write problem: {e}");
        }
        let _ = self.engine.db.set_recording_info(&self.id, duration_ms as i64, Some(&self.pcm.to_string_lossy()));
        FinishedRecording {
            id: self.id.clone(),
            kind: self.kind,
            ctx: self.ctx.clone(),
            duration_ms,
            heard_speech: self.segmenter.heard_speech(),
            pcm: self.pcm.clone(),
            jobs: std::mem::take(&mut self.jobs),
            live,
        }
    }

    /// Cancels: transcription jobs are stopped, the audio is kept in history as "cancelled".
    pub fn cancel(mut self) {
        if let Some(l) = self.live.take() {
            l.cancel();
        }
        for j in &self.jobs {
            j.abort();
        }
        let duration_ms = self.recorded_ms();
        if let Some(w) = self.writer.take() {
            let _ = w.finish();
        }
        let db = &self.engine.db;
        let _ = db.set_recording_info(&self.id, duration_ms as i64, Some(&self.pcm.to_string_lossy()));
        if duration_ms < 600 {
            // Accidental taps: nothing worth keeping.
            let _ = db.delete(&self.id);
            let _ = std::fs::remove_file(&self.pcm);
        } else {
            let _ = db.set_status(&self.id, Status::Cancelled);
            let engine = self.engine.clone();
            let id = self.id.clone();
            let pcm = self.pcm.clone();
            self.rt.spawn_blocking(move || engine.archive_audio(&id, &pcm));
        }
        self.engine.release(&self.id);
    }
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;

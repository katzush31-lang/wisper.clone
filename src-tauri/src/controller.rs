//! The dictation controller: reacts to hotkeys and bar buttons, runs recordings, hands
//! finished audio to the core pipeline, inserts the result, and keeps the floating bar and
//! notifications up to date.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use wisper_core::apps::{self, AppInfo};
use wisper_core::hotkey::{Action, CancelReason, OneShot, RecordingKind};
use wisper_core::pipeline::{CaptureContext, Engine, FinishedRecording, Outcome, PipelineError, Processed, Session, Stage};
use wisper_core::providers::ErrorKind;
use wisper_core::settings::Settings;
use wisper_core::sounds::Sound;

use crate::audio_capture::{Capture, Chunk};
use crate::bar::{self, BarState, ModeRef};
use crate::platform::{self, DuckToken, FieldContext, HookShared, InsertOutcome};
use crate::updater::BUSY;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Paste into whatever app has focus.
    App,
    /// Send to the Scratchpad page.
    Scratchpad,
}

enum RecMsg {
    Audio(Chunk),
    Finish(Sender<FinishedRecording>),
    Cancel,
    SetKind(RecordingKind),
}

struct Recording {
    tx: Sender<RecMsg>,
    kind: RecordingKind,
    hands_free: bool,
    target: Target,
    app: AppInfo,
    duck: Arc<Mutex<Option<DuckToken>>>,
    context: Arc<Mutex<Option<FieldContext>>>,
    hands_free_flag: Arc<AtomicBool>,
    kind_flag: Arc<Mutex<RecordingKind>>,
}

enum Phase {
    Idle,
    Recording(Recording),
    Processing { cancelled: Arc<AtomicBool> },
}

#[derive(Clone)]
struct LastResult {
    id: String,
    /// What's currently in the app.
    final_text: String,
    /// The AI-cleaned version (for toggling back after an undo).
    clean_text: String,
    showing_raw: bool,
    raw_text: String,
    exe: String,
    inserted_at: Instant,
}

pub struct Controller {
    pub app: AppHandle,
    pub engine: Arc<Engine>,
    pub hook: Arc<HookShared>,
    pub capture: Capture,
    pub data_dir: PathBuf,
    phase: Mutex<Phase>,
    last: Mutex<Option<LastResult>>,
    bar_generation: AtomicU64,
    last_bar: Mutex<Option<BarState>>,
    single_tap_hints: AtomicU64,
    dictations_since_suggest: AtomicU64,
}

const UNDO_WINDOW: Duration = Duration::from_secs(10 * 60);

impl Controller {
    pub fn new(app: AppHandle, engine: Arc<Engine>, hook: Arc<HookShared>, capture: Capture, data_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            app,
            engine,
            hook,
            capture,
            data_dir,
            phase: Mutex::new(Phase::Idle),
            last: Mutex::new(None),
            bar_generation: AtomicU64::new(0),
            last_bar: Mutex::new(None),
            single_tap_hints: AtomicU64::new(0),
            dictations_since_suggest: AtomicU64::new(0),
        })
    }

    fn settings(&self) -> Settings {
        self.engine.settings()
    }

    fn hotkey_label(&self) -> String {
        self.settings().hotkeys.dictate.first().map(|c| c.label()).unwrap_or_else(|| "Ctrl + Win".into())
    }

    fn duck_file(&self) -> PathBuf {
        self.data_dir.join("muted-sessions.txt")
    }

    // ------------------------------------------------------------------ bar

    fn bar(&self, phase: &'static str) -> BarState {
        BarState::new(phase, &self.hotkey_label())
    }

    /// Shows a bar state. `hide_after` hides it (or returns to idle) unless something newer
    /// was shown in the meantime.
    pub fn show_bar(self: &Arc<Self>, state: BarState, hide_after: Option<Duration>) {
        let gen = self.bar_generation.fetch_add(1, Ordering::SeqCst) + 1;
        bar::emit(&self.app, &state);
        *self.last_bar.lock() = Some(state);
        if let Some(d) = hide_after {
            let me = self.clone();
            std::thread::spawn(move || {
                std::thread::sleep(d);
                if me.bar_generation.load(Ordering::SeqCst) == gen {
                    me.rest_bar();
                }
            });
        }
    }

    /// Back to resting: hidden, or the small idle pill if "always show" is on.
    pub fn rest_bar(self: &Arc<Self>) {
        if matches!(*self.phase.lock(), Phase::Recording(_) | Phase::Processing { .. }) {
            return;
        }
        let s = self.settings();
        bar::set_top(s.bar_position == wisper_core::settings::BarPosition::Top);
        let st = if s.always_show_bar { self.bar("idle") } else { self.bar("hidden") };
        self.bar_generation.fetch_add(1, Ordering::SeqCst);
        bar::emit(&self.app, &st);
        *self.last_bar.lock() = Some(st);
    }

    fn modes_for_bar(&self) -> Vec<ModeRef> {
        self.engine.modes().into_iter().map(|m| ModeRef { id: m.id, name: m.name }).collect()
    }

    // ------------------------------------------------------------------ hotkey actions

    pub fn on_action(self: &Arc<Self>, a: Action) {
        match a {
            Action::Start { kind, hands_free } => self.begin(kind, hands_free, Target::App),
            Action::BecameHandsFree => {
                if let Phase::Recording(r) = &mut *self.phase.lock() {
                    r.hands_free = true;
                    r.hands_free_flag.store(true, Ordering::SeqCst);
                }
            }
            Action::BecameCommand => {
                if let Phase::Recording(r) = &mut *self.phase.lock() {
                    r.kind = RecordingKind::Command;
                    *r.kind_flag.lock() = RecordingKind::Command;
                    let _ = r.tx.send(RecMsg::SetKind(RecordingKind::Command));
                }
            }
            Action::Finish => self.finish(),
            Action::Cancel(reason) => self.cancel(reason),
            Action::OneShot(s) => self.one_shot(s),
            Action::SuppressStartMenu => {}
        }
    }

    fn one_shot(self: &Arc<Self>, s: OneShot) {
        let me = self.clone();
        std::thread::spawn(move || match s {
            OneShot::PasteLast => me.paste_last(),
            OneShot::CopyLast => me.copy_last(),
            OneShot::UndoCleanup => me.undo_cleanup(),
            OneShot::RerunLast => me.offer_rerun(),
            OneShot::OpenScratchpad => {
                crate::show_main_window(&me.app);
                let _ = me.app.emit_to("main", "navigate", "scratchpad");
            }
            OneShot::Transform(i) => me.run_transform_index(i as usize),
        });
    }

    // ------------------------------------------------------------------ recording

    pub fn begin(self: &Arc<Self>, kind: RecordingKind, hands_free: bool, target: Target) {
        {
            let phase = self.phase.lock();
            if !matches!(*phase, Phase::Idle) {
                return;
            }
        }
        let settings = self.settings();
        let (exe, title) = platform::foreground_app();
        let app_info = if target == Target::Scratchpad { apps::identify("wisper.exe", "Scratchpad") } else { apps::identify(&exe, &title) };
        let ctx = CaptureContext { app: app_info.clone(), ..Default::default() };

        let (tx, rx) = channel::<RecMsg>();
        let audio_tx = tx.clone();
        let sink = Box::new(move |chunk: Chunk| {
            let _ = audio_tx.send(RecMsg::Audio(chunk));
        });
        let (format, preroll) = match self.capture.start(settings.microphone.clone(), settings.keep_mic_warm, sink) {
            Ok(v) => v,
            Err(e) => {
                self.hook.engine.lock().reset_recording();
                platform::play_sound(Sound::Error, settings.sounds.volume);
                self.show_bar(self.bar("error").msg("Microphone problem").detail(e).actions(&["settings"]), Some(Duration::from_secs(8)));
                return;
            }
        };
        let session = match self.engine.start_session(kind, ctx, format.rate, format.channels, tauri::async_runtime::handle().inner().clone()) {
            Ok(s) => s,
            Err(e) => {
                self.capture.stop();
                self.hook.engine.lock().reset_recording();
                self.show_bar(
                    self.bar("error").msg("Couldn't start recording").detail(format!("Wisper couldn't save audio to disk: {e}")),
                    Some(Duration::from_secs(8)),
                );
                return;
            }
        };
        BUSY.store(true, Ordering::SeqCst);
        if settings.sounds.enabled {
            platform::play_sound(Sound::Start, settings.sounds.volume);
        }

        let duck = Arc::new(Mutex::new(None));
        {
            let duck = duck.clone();
            let mode = settings.other_audio;
            let file = self.duck_file();
            // Give the start sound a moment before pausing other audio.
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(120));
                *duck.lock() = Some(platform::duck_other_audio(mode, &file));
            });
        }
        let context = Arc::new(Mutex::new(None));
        let excluded = settings.context_excluded_apps.iter().any(|a| a.eq_ignore_ascii_case(&app_info.exe));
        if target == Target::App && ((settings.context_awareness && !excluded) || kind == RecordingKind::Command) {
            let context = context.clone();
            std::thread::spawn(move || {
                *context.lock() = Some(platform::read_context(900));
            });
        }
        let hands_free_flag = Arc::new(AtomicBool::new(hands_free));
        let kind_flag = Arc::new(Mutex::new(kind));
        {
            let me = self.clone();
            let hf = hands_free_flag.clone();
            let kf = kind_flag.clone();
            std::thread::Builder::new()
                .name("wisper-recorder".into())
                .spawn(move || me.recorder(session, preroll, rx, hf, kf))
                .expect("spawn recorder");
        }
        *self.phase.lock() = Phase::Recording(Recording {
            tx,
            kind,
            hands_free,
            target,
            app: app_info,
            duck,
            context,
            hands_free_flag,
            kind_flag,
        });
        {
            let mut e = self.hook.engine.lock();
            if !e.is_recording() {
                e.start_external(kind, now_ms());
            }
        }
        let mut st = self.bar("listening");
        st.hands_free = hands_free;
        st.kind = if kind == RecordingKind::Command { "command" } else { "dictation" };
        self.show_bar(st, None);
    }

    /// Runs on its own thread: feeds audio to the session and drives the bar's meter.
    fn recorder(self: Arc<Self>, mut session: Session, preroll: Chunk, rx: Receiver<RecMsg>, hands_free: Arc<AtomicBool>, kind: Arc<Mutex<RecordingKind>>) {
        if !preroll.is_empty() {
            session.push(&preroll);
        }
        let started = Instant::now();
        let mut last_emit = Instant::now() - Duration::from_secs(1);
        let mut level = 0.0f32;
        let mut peak = 0.0f32;
        let mut warned_silent = false;
        loop {
            match rx.recv_timeout(Duration::from_millis(30)) {
                Ok(RecMsg::Audio(chunk)) => {
                    level = session.push(&chunk);
                    peak = peak.max(level);
                }
                Ok(RecMsg::Finish(reply)) => {
                    // Take whatever audio is still queued, then close.
                    while let Ok(RecMsg::Audio(chunk)) = rx.try_recv() {
                        session.push(&chunk);
                    }
                    let _ = reply.send(session.finish());
                    return;
                }
                Ok(RecMsg::Cancel) => {
                    session.cancel();
                    return;
                }
                Ok(RecMsg::SetKind(k)) => session.set_kind(k),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = session.finish();
                    return;
                }
            }
            if last_emit.elapsed() >= Duration::from_millis(33) {
                last_emit = Instant::now();
                let mut st = self.bar("listening");
                st.level = level;
                st.elapsed_ms = started.elapsed().as_millis() as u64;
                st.hands_free = hands_free.load(Ordering::SeqCst);
                st.kind = if *kind.lock() == RecordingKind::Command { "command" } else { "dictation" };
                if !warned_silent && started.elapsed() > Duration::from_millis(3500) && peak < 0.02 {
                    warned_silent = true;
                }
                if warned_silent && peak < 0.02 {
                    st.message = Some("No sound from the microphone. Is it muted?".into());
                }
                if let Some(err) = self.capture.take_error() {
                    st.message = Some(format!("Microphone problem: {err}"));
                }
                let _ = self.app.emit_to(bar::LABEL, "bar-state", &st);
            }
        }
    }

    pub fn finish(self: &Arc<Self>) {
        let rec = {
            let mut phase = self.phase.lock();
            match std::mem::replace(&mut *phase, Phase::Idle) {
                Phase::Recording(r) => {
                    let cancelled = Arc::new(AtomicBool::new(false));
                    *phase = Phase::Processing { cancelled: cancelled.clone() };
                    Some((r, cancelled))
                }
                other => {
                    *phase = other;
                    None
                }
            }
        };
        let Some((rec, cancelled)) = rec else { return };
        self.hook.engine.lock().set_processing(true);
        let (reply_tx, reply_rx) = channel();
        let _ = rec.tx.send(RecMsg::Finish(reply_tx));
        self.capture.stop();
        let settings = self.settings();
        if settings.sounds.enabled {
            platform::play_sound(Sound::Stop, settings.sounds.volume);
        }
        self.restore_audio(&rec);
        let mut st = self.bar("processing").msg("Transcribing…");
        st.kind = if rec.kind == RecordingKind::Command { "command" } else { "dictation" };
        self.show_bar(st, None);
        let me = self.clone();
        std::thread::spawn(move || {
            let Ok(mut finished) = reply_rx.recv_timeout(Duration::from_secs(10)) else {
                me.done_processing();
                me.show_bar(me.bar("error").msg("Recording didn't finish properly").detail("The audio is saved in History.").actions(&["history"]), Some(Duration::from_secs(8)));
                return;
            };
            // Merge in what UI Automation found around the cursor (waits briefly if it's still reading).
            let deadline = Instant::now() + Duration::from_millis(600);
            let ctx = loop {
                if let Some(c) = rec.context.lock().take() {
                    break Some(c);
                }
                if Instant::now() > deadline {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            if let Some(c) = ctx {
                if !c.is_password {
                    finished.ctx.text_before = c.before;
                    finished.ctx.text_after = c.after;
                    if finished.ctx.selection.is_none() {
                        finished.ctx.selection = c.selection;
                    }
                }
            }
            if finished.kind == RecordingKind::Command && finished.ctx.selection.is_none() && rec.target == Target::App {
                finished.ctx.selection = platform::copy_selection_via_clipboard();
            }
            me.process(finished, rec.target, rec.app, cancelled);
        });
    }

    fn restore_audio(&self, rec: &Recording) {
        let duck = rec.duck.clone();
        let file = self.duck_file();
        std::thread::spawn(move || {
            // The ducking thread may still be running; wait for its token.
            for _ in 0..50 {
                if let Some(t) = duck.lock().take() {
                    platform::restore_other_audio(t, &file);
                    return;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
        });
    }

    fn process(self: &Arc<Self>, rec: FinishedRecording, target: Target, app: AppInfo, cancelled: Arc<AtomicBool>) {
        let me = self.clone();
        let kind = rec.kind;
        tauri::async_runtime::spawn(async move {
            let progress_me = me.clone();
            let progress = move |stage: Stage| {
                let msg = match stage {
                    Stage::Transcribing { done, total } if total > 2 => format!("Transcribing… {}/{}", done + 1, total),
                    Stage::Transcribing { .. } => "Transcribing…".to_string(),
                    Stage::Cleaning => {
                        if kind == RecordingKind::Command {
                            "Working on it…".to_string()
                        } else {
                            "Cleaning up…".to_string()
                        }
                    }
                };
                let mut st = progress_me.bar("processing").msg(msg);
                st.kind = if kind == RecordingKind::Command { "command" } else { "dictation" };
                let _ = progress_me.app.emit_to(bar::LABEL, "bar-state", &st);
            };
            let result = me.engine.process(rec, &progress).await;
            let _ = me.app.emit("history-changed", "");
            let me2 = me.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || me2.deliver(result, target, app, cancelled, kind)).await;
        });
    }

    fn deliver(self: &Arc<Self>, result: Result<Outcome, PipelineError>, target: Target, app: AppInfo, cancelled: Arc<AtomicBool>, kind: RecordingKind) {
        self.done_processing();
        if cancelled.load(Ordering::SeqCst) {
            self.show_bar(self.bar("notice").msg("Cancelled").detail("The dictation is saved in History."), Some(Duration::from_secs(3)));
            return;
        }
        match result {
            Ok(Outcome::Text(p)) => self.insert_result(p, target, &app, kind),
            Ok(Outcome::NoSpeech { .. }) => {
                self.show_bar(self.bar("notice").msg("Didn't catch anything").detail("No speech was detected. Check your microphone if this keeps happening."), Some(Duration::from_secs(3)));
            }
            Err(e) => self.report_error(&e),
        }
    }

    fn insert_result(self: &Arc<Self>, p: Processed, target: Target, app: &AppInfo, kind: RecordingKind) {
        if target == Target::Scratchpad {
            let _ = self.app.emit_to("main", "scratchpad-text", &p.final_text);
            self.show_bar(self.bar("done").msg("Added to Scratchpad"), Some(Duration::from_secs(2)));
            return;
        }
        let outcome = platform::insert_text(&p.final_text, &app.exe);
        match outcome {
            InsertOutcome::Pasted => {
                let _ = self.engine.db.set_inserted(&p.id, true);
                *self.last.lock() = Some(LastResult {
                    id: p.id.clone(),
                    final_text: p.final_text.clone(),
                    clean_text: p.final_text.clone(),
                    showing_raw: false,
                    raw_text: p.raw_text.clone(),
                    exe: app.exe.clone(),
                    inserted_at: Instant::now(),
                });
                let mut st = self.bar("done");
                st.dictation_id = Some(p.id.clone());
                st.text = Some(p.final_text.clone());
                st.mode_name = Some(p.mode_name.clone());
                st.modes = self.modes_for_bar();
                if let Some(w) = &p.cleanup_warning {
                    st.message = Some("Inserted without AI cleanup".into());
                    st.detail = Some(w.clone());
                    st.actions = vec!["copy", "settings"];
                    self.notify_key_problem_if_needed(w);
                    self.show_bar(st, Some(Duration::from_secs(7)));
                } else {
                    st.message = Some(if kind == RecordingKind::Command { "Done".into() } else { p.mode_name.clone() });
                    st.actions = if kind == RecordingKind::Command { vec!["undo", "copy"] } else { vec!["undo", "rerun", "copy"] };
                    self.show_bar(st, Some(Duration::from_millis(3500)));
                }
                self.maybe_suggest();
            }
            InsertOutcome::NoTextField => {
                *self.last.lock() = Some(LastResult {
                    id: p.id.clone(),
                    final_text: p.final_text.clone(),
                    clean_text: p.final_text.clone(),
                    showing_raw: false,
                    raw_text: p.raw_text.clone(),
                    exe: String::new(),
                    inserted_at: Instant::now() - UNDO_WINDOW,
                });
                let mut st = self.bar("notice").msg("No text box was selected").detail("Your text is ready. Copy it here, or click into a text box and press Alt+Shift+Z.");
                st.text = Some(p.final_text.clone());
                st.dictation_id = Some(p.id.clone());
                st.actions = vec!["copy", "history"];
                self.show_bar(st, Some(Duration::from_secs(20)));
            }
            InsertOutcome::Failed(why) => {
                let mut st = self.bar("error").msg("Couldn't paste").detail(why);
                st.text = Some(p.final_text.clone());
                st.dictation_id = Some(p.id.clone());
                st.actions = vec!["copy", "history"];
                self.show_bar(st, Some(Duration::from_secs(20)));
            }
        }
    }

    fn report_error(self: &Arc<Self>, e: &PipelineError) {
        let settings = self.settings();
        platform::play_sound(Sound::Error, if settings.sounds.enabled { settings.sounds.volume } else { 0.0 });
        match e {
            PipelineError::Queued { message, .. } => {
                let st = self.bar("error").msg("Saved — will finish when possible").detail(message.clone()).actions(&["history"]);
                self.show_bar(st, Some(Duration::from_secs(8)));
                if settings.notifications.waiting_offline {
                    self.notify("Dictation saved for later", message);
                }
            }
            PipelineError::Failed { message, kind, .. } => {
                let key_problem = matches!(kind, Some(ErrorKind::Auth) | Some(ErrorKind::Quota));
                let actions: &[&'static str] = if key_problem { &["settings", "history"] } else { &["retry", "history"] };
                let st = self.bar("error").msg(if key_problem { "AI key problem" } else { "Dictation failed" }).detail(message.clone()).actions(actions);
                self.show_bar(st, Some(Duration::from_secs(12)));
                if key_problem && settings.notifications.api_key_problem {
                    self.notify("An API key needs attention", message);
                } else if settings.notifications.dictation_failed {
                    self.notify("Dictation failed", message);
                }
            }
        }
    }

    fn notify_key_problem_if_needed(&self, warning: &str) {
        if warning.contains("API key") && self.settings().notifications.api_key_problem {
            self.notify("An API key needs attention", warning);
        }
    }

    pub fn notify(&self, title: &str, body: &str) {
        let _ = self.app.notification().builder().title(title).body(body).show();
    }

    fn done_processing(&self) {
        *self.phase.lock() = Phase::Idle;
        self.hook.engine.lock().set_processing(false);
        BUSY.store(false, Ordering::SeqCst);
    }

    pub fn cancel(self: &Arc<Self>, reason: CancelReason) {
        let mut phase = self.phase.lock();
        match std::mem::replace(&mut *phase, Phase::Idle) {
            Phase::Recording(r) => {
                drop(phase);
                let _ = r.tx.send(RecMsg::Cancel);
                self.capture.stop();
                self.restore_audio(&r);
                self.hook.engine.lock().reset_recording();
                BUSY.store(false, Ordering::SeqCst);
                match reason {
                    CancelReason::SingleTap => {
                        // Show the hint only the first few times.
                        if self.single_tap_hints.fetch_add(1, Ordering::SeqCst) < 3 {
                            let hint = format!("Hold {} to talk, or tap it twice for hands-free.", self.hotkey_label());
                            self.show_bar(self.bar("notice").msg("Tap detected").detail(hint), Some(Duration::from_secs(3)));
                        } else {
                            self.rest_bar();
                        }
                    }
                    CancelReason::OtherShortcut => self.rest_bar(),
                    CancelReason::User => {
                        self.show_bar(self.bar("notice").msg("Cancelled"), Some(Duration::from_millis(1200)));
                    }
                }
            }
            Phase::Processing { cancelled } => {
                cancelled.store(true, Ordering::SeqCst);
                *phase = Phase::Processing { cancelled };
                drop(phase);
                self.show_bar(self.bar("notice").msg("Cancelled").detail("The dictation is saved in History."), Some(Duration::from_secs(2)));
            }
            Phase::Idle => {}
        }
    }

    // ------------------------------------------------------------------ after-the-fact actions

    pub fn paste_last(self: &Arc<Self>) {
        let Ok(Some(item)) = self.engine.db.last_with_text() else {
            self.show_bar(self.bar("notice").msg("Nothing to paste yet"), Some(Duration::from_secs(2)));
            return;
        };
        let text = item.final_text.unwrap_or_default();
        let (exe, _) = platform::foreground_app();
        match platform::insert_text(&text, &exe) {
            InsertOutcome::Pasted => {}
            InsertOutcome::NoTextField => {
                let mut st = self.bar("notice").msg("No text box was selected").detail("Click into a text box and try again, or copy it here.");
                st.text = Some(text);
                st.actions = vec!["copy"];
                self.show_bar(st, Some(Duration::from_secs(10)));
            }
            InsertOutcome::Failed(w) => {
                self.show_bar(self.bar("error").msg("Couldn't paste").detail(w), Some(Duration::from_secs(8)));
            }
        }
    }

    pub fn copy_last(self: &Arc<Self>) {
        let text = self
            .last_bar
            .lock()
            .as_ref()
            .and_then(|b| b.text.clone())
            .or_else(|| self.engine.db.last_with_text().ok().flatten().and_then(|i| i.final_text));
        let Some(text) = text else {
            self.show_bar(self.bar("notice").msg("Nothing to copy yet"), Some(Duration::from_secs(2)));
            return;
        };
        match platform::set_clipboard_text(&text) {
            Ok(_) => self.show_bar(self.bar("done").msg("Copied to clipboard"), Some(Duration::from_millis(1500))),
            Err(e) => self.show_bar(self.bar("error").msg("Couldn't copy").detail(e), Some(Duration::from_secs(5))),
        }
    }

    fn recent_insert(&self) -> Option<LastResult> {
        self.last.lock().clone().filter(|l| l.inserted_at.elapsed() < UNDO_WINDOW && !l.exe.is_empty())
    }

    /// Swaps the last inserted text for the original transcript; pressing again swaps back.
    pub fn undo_cleanup(self: &Arc<Self>) {
        let Some(last) = self.recent_insert() else {
            self.show_bar(self.bar("notice").msg("Nothing recent to undo"), Some(Duration::from_secs(2)));
            return;
        };
        if last.raw_text.trim() == last.clean_text.trim() {
            self.show_bar(self.bar("notice").msg("Already the original wording"), Some(Duration::from_secs(2)));
            return;
        }
        if last.showing_raw {
            self.replace_last(&last, last.clean_text.clone(), "Cleaned-up version", false, None);
        } else {
            self.replace_last(&last, last.raw_text.trim().to_string(), "Original transcript", true, None);
        }
    }

    fn replace_last(self: &Arc<Self>, last: &LastResult, new_text: String, label: &str, showing_raw: bool, new_clean: Option<String>) {
        platform::wait_for_modifiers_released(1500);
        platform::send_undo();
        std::thread::sleep(Duration::from_millis(120));
        match platform::insert_text(&new_text, &last.exe) {
            InsertOutcome::Pasted => {
                let _ = self.engine.db.set_final_text(&last.id, &new_text);
                *self.last.lock() = Some(LastResult {
                    final_text: new_text.clone(),
                    clean_text: new_clean.unwrap_or_else(|| last.clean_text.clone()),
                    showing_raw,
                    inserted_at: Instant::now(),
                    ..last.clone()
                });
                let mut st = self.bar("done").msg(label.to_string());
                st.text = Some(new_text);
                st.dictation_id = Some(last.id.clone());
                st.modes = self.modes_for_bar();
                st.actions = vec!["undo", "rerun", "copy"];
                self.show_bar(st, Some(Duration::from_secs(4)));
                let _ = self.app.emit("history-changed", "");
            }
            _ => {
                let mut st = self.bar("error").msg("Couldn't replace the text").detail("Copy the new version here instead.");
                st.text = Some(new_text);
                st.actions = vec!["copy"];
                self.show_bar(st, Some(Duration::from_secs(10)));
            }
        }
    }

    /// Alt+Shift+R: show the bar with the "Redo as" menu for the last dictation.
    pub fn offer_rerun(self: &Arc<Self>) {
        let Some(last) = self.recent_insert() else {
            self.show_bar(self.bar("notice").msg("Nothing recent to redo"), Some(Duration::from_secs(2)));
            return;
        };
        let mut st = self.bar("done").msg("Redo in another mode");
        st.text = Some(last.final_text.clone());
        st.dictation_id = Some(last.id.clone());
        st.modes = self.modes_for_bar();
        st.actions = vec!["rerun", "undo", "copy"];
        self.show_bar(st, Some(Duration::from_secs(10)));
    }

    pub fn rerun_last(self: &Arc<Self>, mode_id: String) {
        let Some(last) = self.recent_insert() else {
            self.show_bar(self.bar("notice").msg("Nothing recent to redo"), Some(Duration::from_secs(2)));
            return;
        };
        self.show_bar(self.bar("processing").msg("Rewriting…"), None);
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            match me.engine.reprocess(&last.id, &mode_id).await {
                Ok(p) => {
                    let me2 = me.clone();
                    let _ = tauri::async_runtime::spawn_blocking(move || me2.replace_last(&last, p.final_text.clone(), &p.mode_name, false, Some(p.final_text.clone()))).await;
                }
                Err(e) => me.report_error(&e),
            }
        });
    }

    pub fn retry_last_failed(self: &Arc<Self>) {
        let id = self.last_bar.lock().as_ref().and_then(|b| b.dictation_id.clone()).or_else(|| self.engine.db.last().ok().flatten().map(|i| i.id));
        let Some(id) = id else { return };
        self.show_bar(self.bar("processing").msg("Trying again…"), None);
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            let r = me.engine.retry_dictation(&id, None).await;
            let _ = me.app.emit("history-changed", "");
            match r {
                Ok(Outcome::Text(p)) => {
                    let mut st = me.bar("done").msg("Ready — copy it or insert it from History");
                    st.text = Some(p.final_text);
                    st.actions = vec!["copy", "history"];
                    me.show_bar(st, Some(Duration::from_secs(10)));
                }
                Ok(Outcome::NoSpeech { .. }) => me.show_bar(me.bar("notice").msg("No speech found in that recording"), Some(Duration::from_secs(3))),
                Err(e) => me.report_error(&e),
            }
        });
    }

    // ------------------------------------------------------------------ transforms

    fn run_transform_index(self: &Arc<Self>, i: usize) {
        let t = self.settings().transforms.get(i).cloned();
        if let Some(t) = t {
            self.run_transform(&t.id);
        }
    }

    pub fn run_transform(self: &Arc<Self>, id: &str) {
        if !matches!(*self.phase.lock(), Phase::Idle) {
            return;
        }
        let Some(t) = self.settings().transforms.into_iter().find(|t| t.id == id) else { return };
        let (exe, title) = platform::foreground_app();
        let app_info = apps::identify(&exe, &title);
        self.show_bar(self.bar("processing").msg(format!("{}…", t.name)), None);
        let ctx = platform::read_context(700);
        let selection = ctx.selection.filter(|s| !s.trim().is_empty()).or_else(platform::copy_selection_via_clipboard);
        let Some(selection) = selection else {
            self.show_bar(self.bar("notice").msg("Select some text first").detail(format!("Highlight text in any app, then use {}.", t.name)), Some(Duration::from_secs(4)));
            return;
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        *self.phase.lock() = Phase::Processing { cancelled: cancelled.clone() };
        self.hook.engine.lock().set_processing(true);
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            let cctx = CaptureContext { app: app_info.clone(), selection: Some(selection.clone()), ..Default::default() };
            let r = me.engine.run_instruction(&t.instructions, Some(&selection), &cctx).await;
            let me2 = me.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                me2.done_processing();
                if cancelled.load(Ordering::SeqCst) {
                    return;
                }
                match r {
                    Ok(text) => {
                        me2.engine.record_transform(&t.name, &selection, &text, &app_info);
                        let _ = me2.app.emit("history-changed", "");
                        match platform::insert_text(&text, &app_info.exe) {
                            InsertOutcome::Pasted => {
                                let mut st = me2.bar("done").msg(t.name.clone());
                                st.text = Some(text);
                                st.actions = vec!["copy"];
                                me2.show_bar(st, Some(Duration::from_secs(3)));
                            }
                            _ => {
                                let mut st = me2.bar("notice").msg("Couldn't replace the selection").detail("Copy the result here.");
                                st.text = Some(text);
                                st.actions = vec!["copy"];
                                me2.show_bar(st, Some(Duration::from_secs(15)));
                            }
                        }
                    }
                    Err(e) => {
                        let st = me2.bar("error").msg(format!("{} failed", t.name)).detail(e.user_message()).actions(&["settings"]);
                        me2.show_bar(st, Some(Duration::from_secs(8)));
                    }
                }
            })
            .await;
        });
    }

    // ------------------------------------------------------------------ background work

    fn maybe_suggest(self: &Arc<Self>) {
        if self.dictations_since_suggest.fetch_add(1, Ordering::SeqCst) % 10 != 9 {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            let new = me.engine.refresh_suggestions();
            if !new.is_empty() {
                let _ = me.app.emit("suggestions-changed", ());
                if me.settings().notifications.dictionary_suggestion {
                    let s = &new[0];
                    me.notify(
                        "Add a word to your dictionary?",
                        &format!("\"{}\" was also written as \"{}\". Open Dictionary to choose a spelling.", s.word, s.variants.join("\", \"")),
                    );
                }
            }
        });
    }

    /// Processes saved dictations that couldn't finish earlier (offline, crash). Runs at
    /// start-up and every minute while anything is waiting.
    pub fn spawn_queue_worker(self: &Arc<Self>) {
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(8)).await;
            loop {
                let idle = matches!(*me.phase.lock(), Phase::Idle);
                let waiting = me.engine.db.unfinished().map(|v| !v.is_empty()).unwrap_or(false);
                if idle && waiting {
                    let results = me.engine.process_queue().await;
                    let ok = results.iter().filter(|(_, r)| matches!(r, Ok(Outcome::Text(_)))).count();
                    if !results.is_empty() {
                        let _ = me.app.emit("history-changed", "");
                    }
                    if ok > 0 && me.settings().notifications.waiting_offline {
                        me.notify(
                            if ok == 1 { "A saved dictation is ready" } else { "Saved dictations are ready" },
                            "Wisper finished processing what you recorded earlier. Copy or insert it from History.",
                        );
                    }
                }
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
        let me = self.clone();
        std::thread::spawn(move || loop {
            let (a, h) = me.engine.apply_retention();
            if a + h > 0 {
                tracing::info!("retention removed {a} audio files and {h} history items");
            }
            std::thread::sleep(Duration::from_secs(6 * 3600));
        });
    }

    /// Drives the gesture engine's timers (single tap → cancel) and keeps modifier state
    /// honest if a key-up was missed.
    pub fn spawn_hotkey_pump(self: &Arc<Self>, rx: Receiver<platform::HookEvent>) {
        let me = self.clone();
        std::thread::Builder::new()
            .name("wisper-hotkeys".into())
            .spawn(move || {
                let mut last_sync = Instant::now();
                loop {
                    match rx.recv_timeout(Duration::from_millis(25)) {
                        Ok(platform::HookEvent::Action(a)) => me.on_action(a),
                        Ok(platform::HookEvent::Captured { keys, done }) => {
                            let label = wisper_core::hotkey::KeyCombo::new(&keys).label();
                            let _ = me.app.emit_to("main", "hotkey-captured", serde_json::json!({ "keys": keys, "label": label, "done": done }));
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                    let now = now_ms();
                    let actions = {
                        let mut e = me.hook.engine.lock();
                        if e.next_deadline().map(|d| now >= d).unwrap_or(false) {
                            e.tick(now)
                        } else {
                            Vec::new()
                        }
                    };
                    for a in actions {
                        me.on_action(a);
                    }
                    if last_sync.elapsed() > Duration::from_secs(2) {
                        last_sync = Instant::now();
                        let down = platform::modifiers_down();
                        me.hook.engine.lock().sync_modifiers(&down);
                    }
                }
            })
            .expect("spawn hotkey pump");
    }
}

#[cfg(windows)]
fn now_ms() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

#[cfg(not(windows))]
fn now_ms() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Keeps the controller reachable from Tauri commands.
pub struct ControllerState(pub Arc<Controller>);

impl ControllerState {
    pub fn get(app: &AppHandle) -> Arc<Controller> {
        app.state::<ControllerState>().0.clone()
    }
}

//! Tauri commands called by the UI (see src/shared/api.ts for the TypeScript side).
//! Errors are returned as plain-English strings.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use wisper_core::db::{HistoryItem, HistoryQuery, Note, Stats};
use wisper_core::dictionary::{self, DictEntry, EntrySource, Suggestion};
use wisper_core::hotkey::{KeyCombo, OneShot};
use wisper_core::modes::{self, Mode};
use wisper_core::pipeline::{ApiKeys, Outcome, Processed};
use wisper_core::providers::{anthropic::Anthropic, models, openai::OpenAi, ModelInfo, ProviderError};
use wisper_core::settings::Settings;
use wisper_core::snippets::{self, Snippet};

use crate::controller::{Controller, ControllerState, Target};
use crate::platform;

type Res<T> = Result<T, String>;

fn ctl(app: &AppHandle) -> Arc<Controller> {
    ControllerState::get(app)
}

fn dberr<E: std::fmt::Display>(e: E) -> String {
    format!("Couldn't read or save local data: {e}")
}

pub struct Paths {
    pub data_dir: std::path::PathBuf,
    pub settings_file: std::path::PathBuf,
}

// ---------------------------------------------------------------- settings

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Settings {
    ctl(&app).engine.settings()
}

#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Settings) -> Res<Settings> {
    let old = ctl(&app).engine.settings();
    let mut s = settings;
    // The key-storage choice changes only through account_set_key_storage.
    s.account.key_storage = old.account.key_storage;
    commit_settings(&app, &old, s)
}

/// The one way to change and save settings from inside the app (the UI, the tray, learned
/// styles, restore): normalises, keeps the app-managed sync bookkeeping, stamps the synced
/// sections that changed, saves, applies and schedules a sync.
fn commit_settings(app: &AppHandle, old: &Settings, new: Settings) -> Res<Settings> {
    let mut s = new.normalised();
    s.section_times = old.section_times.clone();
    s.stamp_changed_sections(old, chrono::Utc::now().timestamp_millis());
    let file = app.state::<Paths>().settings_file.clone();
    s.save(&file).map_err(|e| format!("Couldn't save settings: {e}"))?;
    apply_settings(app, &s);
    crate::account::notify_changed(app);
    Ok(s)
}

/// Changes the current settings with `f` and saves them the standard way (see `commit_settings`).
pub fn update_settings(app: &AppHandle, f: impl FnOnce(&mut Settings)) -> Res<Settings> {
    let old = ctl(app).engine.settings();
    let mut s = old.clone();
    f(&mut s);
    commit_settings(app, &old, s)
}

/// Pushes new settings to every part of the app that caches them.
pub fn apply_settings(app: &AppHandle, s: &Settings) {
    let c = ctl(app);
    let old = c.engine.settings();
    c.engine.set_settings(s.clone());
    // Only touch the shortcut engine when shortcuts changed (always the first time), so applying
    // unrelated settings (e.g. from a sync) can't reset a push-to-talk key that is being held.
    static SHORTCUTS_CONFIGURED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if old.shortcuts_differ(s) || !SHORTCUTS_CONFIGURED.swap(true, Ordering::SeqCst) {
        let mut e = c.hook.engine.lock();
        e.set_config(s.hotkeys.clone());
        e.set_extra(
            s.transforms
                .iter()
                .enumerate()
                .filter_map(|(i, t)| t.hotkey.clone().map(|k| (OneShot::Transform(i as u8), k)))
                .collect(),
        );
    }
    // "Shortcuts enabled" switch: the hook keeps running but ignores every shortcut.
    let disabled = !s.shortcuts_enabled;
    c.hook.paused.store(disabled, Ordering::SeqCst);
    if disabled {
        if c.is_recording() {
            c.cancel(wisper_core::hotkey::CancelReason::OtherShortcut);
        }
        let mut e = c.hook.engine.lock();
        e.reset_recording();
        e.clear_pressed();
    }
    crate::tray::sync_shortcuts(app, s.shortcuts_enabled);
    let uses_mouse = all_combos(s).iter().any(|c| c.keys.iter().any(|k| matches!(*k, 0x04..=0x06)));
    if c.hook.mouse_enabled.swap(uses_mouse, Ordering::SeqCst) != uses_mouse {
        platform::refresh_hooks();
    }
    if old.launch_at_login != s.launch_at_login {
        let al = app.autolaunch();
        let _ = if s.launch_at_login { al.enable() } else { al.disable() };
    }
    if old.keep_mic_warm != s.keep_mic_warm || old.microphone != s.microphone {
        // Takes effect after any recording in progress, never in the middle of it.
        c.capture.close_when_free();
    }
    let _ = app.emit("settings-changed", s);
    c.rest_bar();
}

fn all_combos(s: &Settings) -> Vec<KeyCombo> {
    let h = &s.hotkeys;
    let mut v: Vec<KeyCombo> = [&h.dictate, &h.hands_free, &h.command, &h.paste_last, &h.copy_last, &h.undo_cleanup, &h.rerun_last, &h.open_scratchpad]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    v.extend(s.transforms.iter().filter_map(|t| t.hotkey.clone()));
    v
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfoDto {
    version: String,
    data_dir: String,
}

#[tauri::command]
pub fn app_info(app: AppHandle, paths: State<'_, Paths>) -> AppInfoDto {
    AppInfoDto { version: app.package_info().version.to_string(), data_dir: paths.data_dir.to_string_lossy().into_owned() }
}

#[tauri::command]
pub fn open_data_folder(app: AppHandle, paths: State<'_, Paths>) -> Res<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_path(paths.data_dir.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- history

#[tauri::command]
pub fn list_history(app: AppHandle, query: HistoryQuery) -> Res<Vec<HistoryItem>> {
    ctl(&app).engine.db.list(&query).map_err(dberr)
}

#[tauri::command]
pub fn get_history_item(app: AppHandle, id: String) -> Res<Option<HistoryItem>> {
    ctl(&app).engine.db.get(&id).map_err(dberr)
}

#[tauri::command]
pub fn delete_history_item(app: AppHandle, id: String) -> Res<bool> {
    let ok = ctl(&app).engine.delete_dictation(&id);
    if !ok {
        return Err("That dictation is still being processed. Try again in a moment.".into());
    }
    let _ = app.emit("history-changed", "");
    Ok(true)
}

#[tauri::command]
pub async fn reinsert_history_item(app: AppHandle, id: String, raw: bool) -> Res<()> {
    let c = ctl(&app);
    let item = c.engine.db.get(&id).map_err(dberr)?.ok_or("That dictation no longer exists.")?;
    let text = if raw { item.raw_text } else { item.final_text.or(item.raw_text) }.unwrap_or_default();
    if text.trim().is_empty() {
        return Err("There's no text to insert for this dictation.".into());
    }
    // Hand focus back to the app the user was in before opening Wisper, then paste.
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.minimize();
    }
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::sleep(Duration::from_millis(350));
        let (exe, _) = platform::foreground_app();
        match platform::insert_text(&text, &exe) {
            platform::InsertOutcome::Pasted => Ok(()),
            platform::InsertOutcome::NoTextField => Err("No text box was selected in the app behind Wisper. Click into one and use Alt+Shift+Z, or copy instead.".to_string()),
            platform::InsertOutcome::Failed(w) => Err(w),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn reprocess_history_item(app: AppHandle, id: String, mode_id: String) -> Res<Processed> {
    let c = ctl(&app);
    let r = c.engine.reprocess(&id, &mode_id).await.map_err(|e| e.message().to_string());
    let _ = app.emit("history-changed", "");
    r
}

#[tauri::command]
pub async fn retry_history_item(app: AppHandle, id: String) -> Res<Option<Processed>> {
    let c = ctl(&app);
    // Retrying by hand also transcribes pieces the speech detector thought were silent.
    let r = c.engine.retry_dictation_with(&id, None, true).await;
    let _ = app.emit("history-changed", "");
    match r {
        Ok(Outcome::Text(p)) => Ok(Some(p)),
        Ok(Outcome::NoSpeech { .. }) => Ok(None),
        Err(e) => Err(e.message().to_string()),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryWaitingResult {
    done: usize,
    still_waiting: usize,
}

/// "Try all again" on the waiting-recordings row: every queued or failed recording, oldest
/// first. Stops early when the internet is still down.
#[tauri::command]
pub async fn retry_waiting(app: AppHandle) -> Res<RetryWaitingResult> {
    use wisper_core::providers::ErrorKind;
    use wisper_core::pipeline::PipelineError;
    let c = ctl(&app);
    let mut ids = Vec::new();
    for status in ["queued", "failed"] {
        let q = HistoryQuery { status: Some(status.into()), limit: Some(500), ..Default::default() };
        ids.extend(c.engine.db.list(&q).map_err(dberr)?);
    }
    ids.sort_by_key(|i| i.created_at);
    let total = ids.len();
    let mut done = 0;
    for it in ids {
        let r = c.engine.retry_dictation(&it.id, None).await;
        let offline = matches!(&r, Err(PipelineError::Queued { kind, .. }) if matches!(kind, ErrorKind::Network | ErrorKind::Timeout));
        if matches!(r, Ok(_)) {
            done += 1;
        }
        if offline {
            break;
        }
    }
    let _ = app.emit("history-changed", "");
    Ok(RetryWaitingResult { done, still_waiting: total - done })
}

#[tauri::command]
pub fn copy_text(text: String) -> Res<()> {
    platform::set_clipboard_text(&text)
}

// ---------------------------------------------------------------- dictionary

#[tauri::command]
pub fn list_dictionary(app: AppHandle) -> Res<Vec<DictEntry>> {
    ctl(&app).engine.db.dict_all().map_err(dberr)
}

fn clean_list(v: &[String]) -> Vec<String> {
    v.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && !s.contains(['<', '>'])).collect()
}

#[tauri::command]
pub fn add_dictionary_entry(app: AppHandle, entry: DictEntry) -> Res<i64> {
    let word = dictionary::validate_word(&entry.word)?;
    let e = DictEntry { word, replace: clean_list(&entry.replace), sounds_like: clean_list(&entry.sounds_like), source: EntrySource::User, ..entry };
    let id = ctl(&app).engine.db.dict_add(&e).map_err(dberr)?;
    crate::account::notify_changed(&app);
    Ok(id)
}

#[tauri::command]
pub fn update_dictionary_entry(app: AppHandle, entry: DictEntry) -> Res<()> {
    let word = dictionary::validate_word(&entry.word)?;
    let e = DictEntry { word, replace: clean_list(&entry.replace), sounds_like: clean_list(&entry.sounds_like), ..entry };
    ctl(&app).engine.db.dict_update(&e).map_err(|e| {
        if e.to_string().contains("UNIQUE") {
            "That word is already in your dictionary.".to_string()
        } else {
            dberr(e)
        }
    })?;
    crate::account::notify_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn delete_dictionary_entry(app: AppHandle, id: i64) -> Res<()> {
    ctl(&app).engine.db.dict_delete(id).map_err(dberr)?;
    crate::account::notify_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn list_suggestions(app: AppHandle) -> Res<Vec<(i64, Suggestion)>> {
    ctl(&app).engine.db.suggestions_pending().map_err(dberr)
}

#[tauri::command]
pub fn resolve_suggestion(app: AppHandle, id: i64, accept: bool) -> Res<()> {
    let c = ctl(&app);
    if let Some(s) = c.engine.db.suggestion_resolve(id, accept).map_err(dberr)? {
        if accept {
            let mut e = DictEntry::user(&s.word);
            e.source = EntrySource::Learned;
            e.replace = s.variants;
            c.engine.db.dict_add(&e).map_err(dberr)?;
        }
    }
    let _ = app.emit("suggestions-changed", ());
    if accept {
        crate::account::notify_changed(&app);
    }
    Ok(())
}

#[derive(Serialize)]
pub struct ImportResult {
    added: usize,
    skipped: usize,
}

#[tauri::command]
pub fn import_dictionary_csv(app: AppHandle, text: String) -> Res<ImportResult> {
    let c = ctl(&app);
    let existing: Vec<String> = c.engine.db.dict_all().map_err(dberr)?.into_iter().map(|e| e.word.to_lowercase()).collect();
    let (mut added, mut skipped) = (0, 0);
    for line in text.lines().take(5000) {
        let cols: Vec<&str> = line.splitn(2, ',').map(|s| s.trim().trim_matches('"')).collect();
        let (word, wrong) = match cols.as_slice() {
            [w] => (*w, None),
            [wrong, right] => (*right, Some(*wrong)),
            _ => continue,
        };
        let Ok(word) = dictionary::validate_word(word) else {
            skipped += 1;
            continue;
        };
        let mut e = DictEntry::user(&word);
        if let Some(w) = wrong.filter(|w| !w.is_empty() && !w.eq_ignore_ascii_case(&word)) {
            e.replace = vec![w.to_string()];
        }
        if existing.contains(&word.to_lowercase()) && e.replace.is_empty() {
            skipped += 1;
            continue;
        }
        if c.engine.db.dict_add(&e).is_ok() {
            added += 1;
        } else {
            skipped += 1;
        }
    }
    crate::account::notify_changed(&app);
    Ok(ImportResult { added, skipped })
}

// ---------------------------------------------------------------- snippets

#[tauri::command]
pub fn list_snippets(app: AppHandle) -> Res<Vec<Snippet>> {
    ctl(&app).engine.db.snippets_all().map_err(dberr)
}

#[tauri::command]
pub fn save_snippet(app: AppHandle, snippet: Snippet) -> Res<i64> {
    let (trigger, expansion) = snippets::validate(&snippet.trigger, &snippet.expansion)?;
    let id = ctl(&app).engine.db.snippet_save(&Snippet { trigger, expansion, ..snippet }).map_err(|e| {
        if e.to_string().contains("UNIQUE") {
            "Another snippet already uses that phrase.".to_string()
        } else {
            dberr(e)
        }
    })?;
    crate::account::notify_changed(&app);
    Ok(id)
}

#[tauri::command]
pub fn delete_snippet(app: AppHandle, id: i64) -> Res<()> {
    ctl(&app).engine.db.snippet_delete(id).map_err(dberr)?;
    crate::account::notify_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn import_snippets_json(app: AppHandle, text: String) -> Res<ImportResult> {
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|_| "That isn't valid JSON. Paste a list like [{\"name\": \"my email\", \"text\": \"me@example.com\"}].".to_string())?;
    let arr = v.as_array().ok_or("Expected a list of snippets.")?;
    let c = ctl(&app);
    let (mut added, mut skipped) = (0, 0);
    for item in arr.iter().take(2000) {
        let trigger = item.get("trigger").or_else(|| item.get("name")).and_then(|x| x.as_str()).unwrap_or("");
        let expansion = item.get("expansion").or_else(|| item.get("text")).and_then(|x| x.as_str()).unwrap_or("");
        let saved = snippets::validate(trigger, expansion)
            .ok()
            .map(|(t, e)| c.engine.db.snippet_save(&Snippet { id: 0, trigger: t, expansion: e, uses: 0 }).is_ok())
            .unwrap_or(false);
        if saved {
            added += 1;
        } else {
            skipped += 1;
        }
    }
    crate::account::notify_changed(&app);
    Ok(ImportResult { added, skipped })
}

// ---------------------------------------------------------------- modes & stats

#[tauri::command]
pub fn list_modes(app: AppHandle) -> Vec<Mode> {
    ctl(&app).engine.modes()
}

/// "AI instructions" page: the real blocks the cleanup AI is sent, built with the same
/// functions the pipeline uses.
#[tauri::command]
pub fn prompt_preview(
    app: AppHandle,
    mode_id: String,
    app_category: Option<wisper_core::apps::AppCategory>,
    cleanup_level: Option<wisper_core::prompts::CleanupLevel>,
) -> Res<wisper_core::prompt_preview::PromptPreview> {
    use wisper_core::prompt_preview::{build, sample_for, PreviewRequest};
    let c = ctl(&app);
    let entries = c.engine.db.dict_all().map_err(dberr)?;
    let req = PreviewRequest { mode_id, app_category, cleanup_level };
    build(&c.engine.settings(), &c.engine.modes(), &entries, &req, &sample_for(app_category)).ok_or_else(|| "That mode no longer exists.".to_string())
}

#[tauri::command]
pub fn default_mode_instructions(id: String) -> Option<String> {
    modes::default_instructions(&id).map(String::from)
}

#[tauri::command]
pub fn get_stats(app: AppHandle) -> Res<Stats> {
    let c = ctl(&app);
    // Signed in: numbers from all your computers combined; otherwise just this one.
    let all = crate::account::combined_stats(&app);
    c.engine.db.stats_with(c.engine.settings().typing_wpm, chrono::Local::now(), all).map_err(dberr)
}

// ---------------------------------------------------------------- providers

#[derive(Serialize)]
pub struct KeyStatus {
    openai: bool,
    anthropic: bool,
}

#[tauri::command]
pub fn key_status(app: AppHandle) -> KeyStatus {
    let k = ctl(&app).engine.keys();
    KeyStatus { openai: k.openai.is_some(), anthropic: k.anthropic.is_some() }
}

pub fn load_keys() -> ApiKeys {
    ApiKeys { openai: crate::secrets::get("openai"), anthropic: crate::secrets::get("anthropic") }
}

#[tauri::command]
pub fn set_api_key(app: AppHandle, provider: String, key: String) -> Res<()> {
    let key = key.trim().to_string();
    if !key.is_empty() && (key.len() < 20 || key.contains(char::is_whitespace)) {
        return Err("That doesn't look like a complete API key. Copy the whole key and paste it again.".into());
    }
    crate::secrets::set(&provider, &key)?;
    let c = ctl(&app);
    c.engine.set_keys(load_keys());
    // Signed in with keys kept in the account: upload this one too (in the background).
    crate::account::key_changed(&app, &provider, &key);
    // Dictations that failed because of a key problem get another go.
    if c.engine.requeue_auth_failures() > 0 {
        let _ = app.emit("history-changed", "");
    }
    Ok(())
}

#[derive(Serialize)]
pub struct KeyTest {
    ok: bool,
    message: String,
}

fn test_message(provider: &str, r: Result<Vec<String>, ProviderError>, wanted: &str) -> KeyTest {
    match r {
        Ok(models) => {
            let has = models.is_empty() || models.iter().any(|m| m == wanted);
            KeyTest {
                ok: true,
                message: if has {
                    format!("It works. Your {provider} key is valid.")
                } else {
                    format!("The key works, but your account can't use {wanted}. Pick another model below.")
                },
            }
        }
        Err(e) => KeyTest { ok: false, message: e.user_message() },
    }
}

#[tauri::command]
pub async fn test_api_key(app: AppHandle, provider: String, key: Option<String>) -> Res<KeyTest> {
    let c = ctl(&app);
    let s = c.engine.settings();
    let stored = c.engine.keys();
    if provider == "openai" {
        let Some(k) = key.filter(|k| !k.trim().is_empty()).or(stored.openai) else {
            return Ok(KeyTest { ok: false, message: "Paste your OpenAI key first.".into() });
        };
        Ok(test_message("OpenAI", OpenAi::new(&k).list_models().await, &s.providers.transcription_model))
    } else {
        let Some(k) = key.filter(|k| !k.trim().is_empty()).or(stored.anthropic) else {
            return Ok(KeyTest { ok: false, message: "Paste your Claude key first.".into() });
        };
        Ok(test_message("Claude", Anthropic::new(&k).list_models().await, &s.providers.anthropic_model))
    }
}

#[tauri::command]
pub async fn list_models(app: AppHandle, list: String) -> Vec<ModelInfo> {
    let keys = ctl(&app).engine.keys();
    match list.as_str() {
        "transcription" => {
            let live = match keys.openai {
                Some(k) => OpenAi::new(&k).list_models().await.unwrap_or_default(),
                None => vec![],
            };
            models::merge_live(models::transcription_models(), &live, |id| id.contains("transcribe"))
        }
        "openai" => {
            let live = match keys.openai {
                Some(k) => OpenAi::new(&k).list_models().await.unwrap_or_default(),
                None => vec![],
            };
            models::merge_live(models::openai_cleanup_models(), &live, |id| id.starts_with("gpt-6") && !id.contains("audio") && !id.contains("realtime") && !id.contains("transcribe"))
        }
        _ => {
            let live = match keys.anthropic {
                Some(k) => Anthropic::new(&k).list_models().await.unwrap_or_default(),
                None => vec![],
            };
            models::merge_live(models::anthropic_models(), &live, |id| id.starts_with("claude-") && (id.contains("-5") || id.contains("fable")))
        }
    }
}

// ---------------------------------------------------------------- microphone & hotkeys

#[tauri::command]
pub async fn list_microphones() -> Vec<crate::audio_capture::Microphone> {
    tauri::async_runtime::spawn_blocking(crate::audio_capture::list_microphones).await.unwrap_or_default()
}

#[tauri::command]
pub async fn start_mic_test(app: AppHandle, device_id: Option<String>) -> Res<()> {
    tauri::async_runtime::spawn_blocking(move || start_mic_test_blocking(app, device_id)).await.map_err(|e| e.to_string())?
}

fn start_mic_test_blocking(app: AppHandle, device_id: Option<String>) -> Res<()> {
    let c = ctl(&app);
    if c.is_recording() {
        return Err("Finish your dictation first, then test the microphone.".into());
    }
    let old = c.mic_test.lock().take();
    if let Some(old) = old {
        c.capture.stop(old);
    }
    let app2 = app.clone();
    let mut meter = wisper_core::audio::level::LevelMeter::default();
    let mut last = std::time::Instant::now();
    let sink = Box::new(move |chunk: Vec<f32>| {
        let level = meter.update(&chunk);
        if last.elapsed() >= Duration::from_millis(50) {
            last = std::time::Instant::now();
            let _ = app2.emit_to("main", "mic-level", level);
        }
    });
    let warm = c.settings().keep_mic_warm;
    let (_, _, id) = c.capture.start(device_id, warm, sink)?;
    *c.mic_test.lock() = Some(id);
    Ok(())
}

/// Stops only the test's own microphone session, so it can never cut off a dictation.
#[tauri::command]
pub fn stop_mic_test(app: AppHandle) {
    let c = ctl(&app);
    let id = c.mic_test.lock().take();
    if let Some(id) = id {
        c.capture.stop(id);
    }
}

/// The bar page reports where its visible pill/card is, so the rest of the window lets clicks through.
#[tauri::command]
pub fn bar_hit_area(app: AppHandle, area: Option<crate::bar::HitArea>) {
    crate::bar::set_hit_area(&app, area);
}

#[tauri::command]
pub fn start_hotkey_capture(app: AppHandle) {
    ctl(&app).hook.start_capture();
}

#[tauri::command]
pub fn stop_hotkey_capture(app: AppHandle) {
    ctl(&app).hook.stop_capture();
}

#[derive(Serialize)]
pub struct ComboDescription {
    label: String,
    error: Option<String>,
}

#[tauri::command]
pub fn describe_combo(keys: Vec<u32>) -> ComboDescription {
    let c = KeyCombo::new(&keys);
    ComboDescription { label: c.label(), error: c.validate().err() }
}

// ---------------------------------------------------------------- scratchpad & transforms

#[tauri::command]
pub fn list_notes(app: AppHandle, search: Option<String>) -> Res<Vec<Note>> {
    ctl(&app).engine.db.notes(search.as_deref()).map_err(dberr)
}

#[tauri::command]
pub fn save_note(app: AppHandle, note: Note) -> Res<i64> {
    ctl(&app).engine.db.note_save(&note).map_err(dberr)
}

#[tauri::command]
pub fn delete_note(app: AppHandle, id: i64) -> Res<()> {
    ctl(&app).engine.db.note_delete(id).map_err(dberr)
}

#[tauri::command]
pub fn dictate_into_scratchpad(app: AppHandle) {
    let c = ctl(&app);
    std::thread::spawn(move || c.scratchpad_toggle());
}

#[tauri::command]
pub fn run_transform(app: AppHandle, id: String) {
    let c = ctl(&app);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.minimize();
    }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(350));
        c.run_transform(&id);
    });
}

// ---------------------------------------------------------------- bar buttons

#[tauri::command]
pub fn bar_start(app: AppHandle) {
    let c = ctl(&app);
    std::thread::spawn(move || c.begin(wisper_core::hotkey::RecordingKind::Dictation, true, Target::App));
}

#[tauri::command]
pub fn bar_finish(app: AppHandle) {
    let c = ctl(&app);
    c.hook.engine.lock().reset_recording();
    c.finish();
}

#[tauri::command]
pub fn bar_cancel(app: AppHandle) {
    ctl(&app).cancel(wisper_core::hotkey::CancelReason::User);
}

#[tauri::command]
pub fn bar_copy(app: AppHandle) {
    let c = ctl(&app);
    std::thread::spawn(move || c.copy_last());
}

#[tauri::command]
pub fn bar_undo(app: AppHandle) {
    let c = ctl(&app);
    std::thread::spawn(move || c.undo_cleanup());
}

#[tauri::command]
pub fn bar_rerun(app: AppHandle, mode_id: String) {
    ctl(&app).rerun_last(mode_id);
}

#[tauri::command]
pub fn bar_retry(app: AppHandle) {
    ctl(&app).retry_last_failed();
}

#[tauri::command]
pub fn bar_open_hub(app: AppHandle, page: Option<String>) {
    crate::show_main_window(&app);
    if let Some(p) = page {
        let _ = app.emit_to("main", "navigate", p);
    }
    ctl(&app).rest_bar();
}

#[tauri::command]
pub fn bar_dismiss(app: AppHandle) {
    ctl(&app).rest_bar();
}

// ---------------------------------------------------------------- backup

// File dialogs block until closed, so these must never run on the UI thread.
#[tauri::command]
pub async fn export_backup(app: AppHandle) -> Res<String> {
    tauri::async_runtime::spawn_blocking(move || crate::backup::export(&app)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn import_backup(app: AppHandle) -> Res<String> {
    tauri::async_runtime::spawn_blocking(move || crate::backup::import(&app)).await.map_err(|e| e.to_string())?
}

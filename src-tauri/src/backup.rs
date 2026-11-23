//! Backup and restore: settings, your own dictionary words, snippets and Scratchpad notes in
//! one JSON file. API keys are never included.

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use wisper_core::db::Note;
use wisper_core::dictionary::{DictEntry, EntrySource};
use wisper_core::settings::Settings;
use wisper_core::snippets::Snippet;

use crate::controller::ControllerState;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Backup {
    app: String,
    format: u32,
    exported_at: String,
    settings: Settings,
    dictionary: Vec<DictEntry>,
    snippets: Vec<Snippet>,
    notes: Vec<Note>,
}

pub fn export(app: &AppHandle) -> Result<String, String> {
    let c = ControllerState::get(app);
    let db = &c.engine.db;
    let backup = Backup {
        app: "Wisper".into(),
        format: 1,
        exported_at: chrono::Local::now().to_rfc3339(),
        settings: c.engine.settings(),
        dictionary: db.dict_all().map_err(|e| e.to_string())?.into_iter().filter(|e| e.source != EntrySource::Builtin).collect(),
        snippets: db.snippets_all().map_err(|e| e.to_string())?,
        notes: db.notes(None).map_err(|e| e.to_string())?,
    };
    let name = format!("Wisper backup {}.json", chrono::Local::now().format("%Y-%m-%d"));
    let Some(path) = app.dialog().file().set_file_name(&name).add_filter("Wisper backup", &["json"]).blocking_save_file() else {
        return Err("Backup cancelled.".into());
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    let json = serde_json::to_vec_pretty(&backup).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("Couldn't write the backup: {e}"))?;
    Ok(path.to_string_lossy().into_owned())
}

pub fn import(app: &AppHandle) -> Result<String, String> {
    let Some(path) = app.dialog().file().add_filter("Wisper backup", &["json"]).blocking_pick_file() else {
        return Err("Restore cancelled.".into());
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Couldn't read that file: {e}"))?;
    let b: Backup = serde_json::from_str(&text).map_err(|_| "That file isn't a Wisper backup.".to_string())?;
    if b.app != "Wisper" {
        return Err("That file isn't a Wisper backup.".into());
    }
    let c = ControllerState::get(app);
    let db = &c.engine.db;
    let restored = b.settings;
    // This computer keeps its own account choices (e.g. where API keys live).
    crate::commands::update_settings(app, |s| {
        let account = s.account.clone();
        *s = restored;
        s.account = account;
    })?;
    let mut words = 0;
    for mut e in b.dictionary {
        e.id = 0;
        if db.dict_add(&e).is_ok() {
            words += 1;
        }
    }
    let mut snips = 0;
    for mut s in b.snippets {
        s.id = 0;
        if db.snippet_save(&s).is_ok() {
            snips += 1;
        }
    }
    let existing: Vec<(String, String)> = db.notes(None).unwrap_or_default().into_iter().map(|n| (n.title, n.body)).collect();
    let mut notes = 0;
    for mut n in b.notes {
        if existing.contains(&(n.title.clone(), n.body.clone())) {
            continue;
        }
        n.id = 0;
        if db.note_save(&n).is_ok() {
            notes += 1;
        }
    }
    crate::account::notify_changed(app);
    Ok(format!("Restored settings, {words} dictionary words, {snips} snippets and {notes} notes."))
}

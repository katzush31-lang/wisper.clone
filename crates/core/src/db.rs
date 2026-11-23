//! Local SQLite database: dictation history (text + audio path), segments of long
//! recordings, dictionary, snippets, scratchpad notes, and usage statistics.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use chrono::{Datelike, Local, NaiveDate, TimeZone};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::dictionary::{DictEntry, EntrySource, Suggestion};
use crate::snippets::Snippet;

pub type DbResult<T> = Result<T, rusqlite::Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Recording,
    Transcribing,
    Cleaning,
    Done,
    /// Waiting for the internet / a retry.
    Queued,
    Failed,
    Cancelled,
    /// No speech was detected.
    Empty,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Recording => "recording",
            Status::Transcribing => "transcribing",
            Status::Cleaning => "cleaning",
            Status::Done => "done",
            Status::Queued => "queued",
            Status::Failed => "failed",
            Status::Cancelled => "cancelled",
            Status::Empty => "empty",
        }
    }

    pub fn parse(s: &str) -> Status {
        match s {
            "recording" => Status::Recording,
            "transcribing" => Status::Transcribing,
            "cleaning" => Status::Cleaning,
            "done" => Status::Done,
            "queued" => Status::Queued,
            "cancelled" => Status::Cancelled,
            "empty" => Status::Empty,
            _ => Status::Failed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub id: String,
    pub created_at: i64,
    /// "dictation" | "command" | "transform"
    pub kind: String,
    pub status: String,
    pub mode_id: Option<String>,
    pub app_key: Option<String>,
    pub app_name: Option<String>,
    pub duration_ms: i64,
    pub raw_text: Option<String>,
    pub clean_text: Option<String>,
    pub final_text: Option<String>,
    /// Command mode: the text that was selected.
    pub selected_text: Option<String>,
    pub word_count: i64,
    /// Recording file: `.pcm` while in progress, `.flac` once stored; None once deleted.
    pub audio_path: Option<String>,
    pub error: Option<String>,
    pub error_kind: Option<String>,
    pub inserted: bool,
    pub attempts: i64,
    pub timings: Option<serde_json::Value>,
    pub meta: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HistoryQuery {
    pub text: Option<String>,
    /// Inclusive range, unix ms.
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub status: Option<String>,
    /// Leave out items with any of these statuses.
    #[serde(default)]
    pub exclude_status: Option<Vec<String>>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SegmentRow {
    pub dictation_id: String,
    pub idx: i64,
    pub start_sample: i64,
    pub end_sample: i64,
    pub has_speech: bool,
    /// "pending" | "done" | "failed" | "skipped"
    pub status: String,
    pub text: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub pinned: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DayCount {
    /// "YYYY-MM-DD" in local time.
    pub date: String,
    pub words: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppCount {
    pub name: String,
    pub words: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub total_words: i64,
    pub words_this_week: i64,
    pub words_today: i64,
    pub total_dictations: i64,
    /// Average speaking speed in words per minute.
    pub wpm: f64,
    /// Estimated minutes saved compared with typing.
    pub minutes_saved: f64,
    pub minutes_saved_this_week: f64,
    pub streak_days: i64,
    pub longest_streak: i64,
    /// Last 365 days, oldest first.
    pub daily: Vec<DayCount>,
    pub top_apps: Vec<AppCount>,
    pub fixes: i64,
    /// "this-computer" or "all-computers" (numbers from every signed-in computer combined).
    pub scope: String,
    /// How quickly text appeared after recent dictations on this computer.
    pub speed: Speed,
}

/// Waiting time after you stop talking, over the last (up to) 50 completed dictations on this
/// computer. All times are medians in milliseconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Speed {
    pub samples: u32,
    /// Waiting for the transcription to finish after you stopped.
    pub transcribe_wait_median_ms: u64,
    pub cleanup_median_ms: u64,
    /// Key release to text ready to paste.
    pub total_wait_median_ms: u64,
    /// Share (0 to 1) of those dictations cleaned up by the fast model.
    pub fast_model_share: f64,
}

const SPEED_WINDOW: i64 = 50;

fn median(v: &mut [u64]) -> u64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2
    }
}

/// Builds the speed summary from stored `timings` JSON strings. Entries without a measured
/// wait after stopping (e.g. re-processed or retried from saved text) are ignored.
pub fn compute_speed(timings: &[String]) -> Speed {
    let (mut tr, mut cl, mut tot) = (Vec::new(), Vec::new(), Vec::new());
    let mut fast = 0u32;
    for t in timings {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(t) else { continue };
        let (Some(a), Some(b), Some(c)) = (v["transcribeWaitMs"].as_u64(), v["cleanupMs"].as_u64(), v["totalAfterStopMs"].as_u64()) else { continue };
        if a == 0 {
            continue;
        }
        tr.push(a);
        cl.push(b);
        tot.push(c);
        if v["cleanupFast"].as_bool().unwrap_or(false) {
            fast += 1;
        }
    }
    let n = tot.len() as u32;
    Speed {
        samples: n,
        transcribe_wait_median_ms: median(&mut tr),
        cleanup_median_ms: median(&mut cl),
        total_wait_median_ms: median(&mut tot),
        fast_model_share: if n == 0 { 0.0 } else { fast as f64 / n as f64 },
    }
}

/// One synced dictionary word or snippet, in the account's API shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncItem {
    /// "dictionary" | "snippet"
    pub kind: String,
    pub uid: String,
    /// The fields (dictionary: word, replace, soundsLike, source, starred, keyword;
    /// snippet: trigger, expansion). May be empty for a deleted item.
    #[serde(default, deserialize_with = "data_value")]
    pub data: serde_json::Value,
    pub updated_at: i64,
    #[serde(default, deserialize_with = "bool_or_int", serialize_with = "bool_as_int")]
    pub deleted: bool,
}

// The account may send `data` as an object or as a JSON string, and `deleted` as 0/1 or a bool.
fn data_value<'de, D: serde::Deserializer<'de>>(d: D) -> Result<serde_json::Value, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::String(s) => serde_json::from_str(&s).unwrap_or(serde_json::Value::Null),
        other => other,
    })
}

fn bool_or_int<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Bool(b) => b,
        serde_json::Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
        _ => false,
    })
}

fn bool_as_int<S: serde::Serializer>(b: &bool, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_i64(*b as i64)
}

/// A completed dictation reduced to numbers (never text), for usage stats across computers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatRow {
    pub uid: String,
    pub created_at: i64,
    pub words: i64,
    pub duration_ms: i64,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub fixes: i64,
    /// Filled in by the account when reading rows back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// Server time the row was stored; only present when reading rows back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_at: Option<i64>,
}

pub struct Db {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS dictations (
  id TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  kind TEXT NOT NULL DEFAULT 'dictation',
  status TEXT NOT NULL,
  mode_id TEXT, app_key TEXT, app_name TEXT,
  duration_ms INTEGER NOT NULL DEFAULT 0,
  raw_text TEXT, clean_text TEXT, final_text TEXT, selected_text TEXT,
  word_count INTEGER NOT NULL DEFAULT 0,
  audio_path TEXT,
  error TEXT, error_kind TEXT,
  inserted INTEGER NOT NULL DEFAULT 0,
  attempts INTEGER NOT NULL DEFAULT 0,
  fixes INTEGER NOT NULL DEFAULT 0,
  timings TEXT, meta TEXT
);
CREATE INDEX IF NOT EXISTS idx_dictations_created ON dictations(created_at);
CREATE INDEX IF NOT EXISTS idx_dictations_status ON dictations(status);
CREATE TABLE IF NOT EXISTS segments (
  dictation_id TEXT NOT NULL, idx INTEGER NOT NULL,
  start_sample INTEGER NOT NULL, end_sample INTEGER NOT NULL,
  has_speech INTEGER NOT NULL, status TEXT NOT NULL,
  text TEXT, error TEXT,
  PRIMARY KEY (dictation_id, idx)
);
CREATE TABLE IF NOT EXISTS dictionary (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  word TEXT NOT NULL UNIQUE COLLATE NOCASE,
  replace_json TEXT NOT NULL DEFAULT '[]',
  sounds_like_json TEXT NOT NULL DEFAULT '[]',
  source TEXT NOT NULL,
  starred INTEGER NOT NULL DEFAULT 0,
  keyword INTEGER NOT NULL DEFAULT 1,
  uses INTEGER NOT NULL DEFAULT 0,
  hidden INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS suggestions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  word TEXT NOT NULL UNIQUE COLLATE NOCASE,
  variants_json TEXT NOT NULL,
  occurrences INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending',
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS snippets (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  trigger TEXT NOT NULL UNIQUE COLLATE NOCASE,
  expansion TEXT NOT NULL,
  uses INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS notes (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  title TEXT NOT NULL DEFAULT '',
  body TEXT NOT NULL DEFAULT '',
  pinned INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS remote_stats (
  uid TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  words INTEGER NOT NULL DEFAULT 0,
  duration_ms INTEGER NOT NULL DEFAULT 0,
  app TEXT,
  fixes INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS mode_choices (
  app_key TEXT NOT NULL, mode_id TEXT NOT NULL, count INTEGER NOT NULL DEFAULT 0,
  last_at INTEGER NOT NULL,
  PRIMARY KEY (app_key, mode_id)
);
"#;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// A deleted row keeps its UNIQUE word/trigger slot. When another row needs that exact text,
/// the deleted row gives it up by getting its uid appended to the text.
fn retire_tombstone(conn: &Connection, table: &str, col: &str, text: &str, except_id: i64) -> DbResult<()> {
    conn.execute(
        &format!("UPDATE {table} SET {col}={col} || '~' || uid WHERE {col}=?1 AND deleted=1 AND id<>?2"),
        params![text, except_id],
    )?;
    Ok(())
}

fn str_list(v: &serde_json::Value, key: &str) -> String {
    let list: Vec<String> = v.get(key).and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default();
    serde_json::to_string(&list).unwrap()
}

fn apply_dict_item(tx: &Connection, it: &SyncItem) -> DbResult<bool> {
    let d = &it.data;
    let word = d.get("word").and_then(|w| w.as_str()).unwrap_or("").trim().to_string();
    let replace = str_list(d, "replace");
    let sounds = str_list(d, "soundsLike");
    let source = match d.get("source").and_then(|s| s.as_str()) {
        Some("learned") => "learned",
        _ => "user",
    };
    let starred = d.get("starred").and_then(|b| b.as_bool()).unwrap_or(false) as i64;
    let keyword = d.get("keyword").and_then(|b| b.as_bool()).unwrap_or(true) as i64;
    let by_uid: Option<(i64, i64)> = tx.query_row("SELECT id, updated_at FROM dictionary WHERE uid=?1", [&it.uid], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    if let Some((id, mine)) = by_uid {
        if it.updated_at <= mine {
            return Ok(false);
        }
        if it.deleted || word.is_empty() {
            tx.execute("UPDATE dictionary SET deleted=1, dirty=0, updated_at=?2 WHERE id=?1", params![id, it.updated_at])?;
            return Ok(true);
        }
        retire_other_word(tx, "dictionary", "word", &word, id)?;
        tx.execute(
            "UPDATE dictionary SET word=?2, replace_json=?3, sounds_like_json=?4, source=?5, starred=?6, keyword=?7, hidden=0, deleted=0, dirty=0, updated_at=?8 WHERE id=?1",
            params![id, word, replace, sounds, source, starred, keyword, it.updated_at],
        )?;
        return Ok(true);
    }
    if word.is_empty() {
        return Ok(false); // a deletion for something we never had
    }
    let by_word: Option<(i64, String, i64, String)> = tx
        .query_row("SELECT id, uid, updated_at, source FROM dictionary WHERE word=?1", [&word], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .optional()?;
    match by_word {
        None => {
            tx.execute(
                "INSERT INTO dictionary (word, replace_json, sounds_like_json, source, starred, keyword, created_at, uid, updated_at, deleted, dirty) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?7,?9,0)",
                params![word, replace, sounds, source, starred, keyword, it.updated_at, it.uid, it.deleted as i64],
            )?;
            Ok(true)
        }
        Some((id, my_uid, mine, my_source)) => {
            // A starter word edited on another computer, or the same word added on two computers:
            // the newer one wins and everyone ends up with one shared identity.
            let remote_wins = my_source == "builtin" || it.updated_at > mine || (it.updated_at == mine && it.uid < my_uid);
            if !remote_wins {
                return Ok(false);
            }
            tx.execute(
                "UPDATE dictionary SET uid=?2, replace_json=?3, sounds_like_json=?4, source=?5, starred=?6, keyword=?7, hidden=0, deleted=?8, dirty=0, updated_at=?9 WHERE id=?1",
                params![id, it.uid, replace, sounds, source, starred, keyword, it.deleted as i64, it.updated_at],
            )?;
            Ok(true)
        }
    }
}

fn apply_snippet_item(tx: &Connection, it: &SyncItem) -> DbResult<bool> {
    let trigger = it.data.get("trigger").and_then(|w| w.as_str()).unwrap_or("").trim().to_string();
    let expansion = it.data.get("expansion").and_then(|w| w.as_str()).unwrap_or("").to_string();
    let by_uid: Option<(i64, i64)> = tx.query_row("SELECT id, updated_at FROM snippets WHERE uid=?1", [&it.uid], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    if let Some((id, mine)) = by_uid {
        if it.updated_at <= mine {
            return Ok(false);
        }
        if it.deleted || trigger.is_empty() {
            tx.execute("UPDATE snippets SET deleted=1, dirty=0, updated_at=?2 WHERE id=?1", params![id, it.updated_at])?;
            return Ok(true);
        }
        retire_other_word(tx, "snippets", "trigger", &trigger, id)?;
        tx.execute("UPDATE snippets SET trigger=?2, expansion=?3, deleted=0, dirty=0, updated_at=?4 WHERE id=?1", params![id, trigger, expansion, it.updated_at])?;
        return Ok(true);
    }
    if trigger.is_empty() {
        return Ok(false);
    }
    let by_trigger: Option<(i64, String, i64)> = tx
        .query_row("SELECT id, uid, updated_at FROM snippets WHERE trigger=?1", [&trigger], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    match by_trigger {
        None => {
            tx.execute(
                "INSERT INTO snippets (trigger, expansion, created_at, uid, updated_at, deleted, dirty) VALUES (?1,?2,?3,?4,?3,?5,0)",
                params![trigger, expansion, it.updated_at, it.uid, it.deleted as i64],
            )?;
            Ok(true)
        }
        Some((id, my_uid, mine)) => {
            if !(it.updated_at > mine || (it.updated_at == mine && it.uid < my_uid)) {
                return Ok(false);
            }
            tx.execute("UPDATE snippets SET uid=?2, expansion=?3, deleted=?4, dirty=0, updated_at=?5 WHERE id=?1", params![id, it.uid, expansion, it.deleted as i64, it.updated_at])?;
            Ok(true)
        }
    }
}

/// An incoming rename wants a word/trigger another local row already has: that other row loses
/// (it is marked deleted and gives up the text), because the incoming change is newer.
fn retire_other_word(tx: &Connection, table: &str, col: &str, text: &str, keep_id: i64) -> DbResult<()> {
    tx.execute(
        &format!("UPDATE {table} SET {col}={col} || '~' || uid, deleted=1, dirty=1, updated_at=?3 WHERE {col}=?1 AND id<>?2"),
        params![text, keep_id, now_ms()],
    )?;
    Ok(())
}

fn json_vec(s: &str) -> Vec<String> {
    serde_json::from_str(s).unwrap_or_default()
}

fn opt_json(s: Option<String>) -> Option<serde_json::Value> {
    s.and_then(|t| serde_json::from_str(&t).ok())
}

const HISTORY_COLS: &str = "id, created_at, kind, status, mode_id, app_key, app_name, duration_ms, raw_text, clean_text, final_text, selected_text, word_count, audio_path, error, error_kind, inserted, attempts, timings, meta";

fn history_row(r: &Row) -> DbResult<HistoryItem> {
    Ok(HistoryItem {
        id: r.get(0)?,
        created_at: r.get(1)?,
        kind: r.get(2)?,
        status: r.get(3)?,
        mode_id: r.get(4)?,
        app_key: r.get(5)?,
        app_name: r.get(6)?,
        duration_ms: r.get(7)?,
        raw_text: r.get(8)?,
        clean_text: r.get(9)?,
        final_text: r.get(10)?,
        selected_text: r.get(11)?,
        word_count: r.get(12)?,
        audio_path: r.get(13)?,
        error: r.get(14)?,
        error_kind: r.get(15)?,
        inserted: r.get::<_, i64>(16)? != 0,
        attempts: r.get(17)?,
        timings: opt_json(r.get(18)?),
        meta: opt_json(r.get(19)?),
    })
}

fn dict_row(r: &Row) -> DbResult<DictEntry> {
    let source: String = r.get(4)?;
    Ok(DictEntry {
        id: r.get(0)?,
        word: r.get(1)?,
        replace: json_vec(&r.get::<_, String>(2)?),
        sounds_like: json_vec(&r.get::<_, String>(3)?),
        source: match source.as_str() {
            "builtin" => EntrySource::Builtin,
            "learned" => EntrySource::Learned,
            _ => EntrySource::User,
        },
        starred: r.get::<_, i64>(5)? != 0,
        keyword: r.get::<_, i64>(6)? != 0,
        uses: r.get(7)?,
    })
}

fn source_str(s: EntrySource) -> &'static str {
    match s {
        EntrySource::Builtin => "builtin",
        EntrySource::User => "user",
        EntrySource::Learned => "learned",
    }
}

fn new_uid() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn has_column(conn: &Connection, table: &str, col: &str) -> DbResult<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names: Vec<String> = stmt.query_map([], |r| r.get::<_, String>(1))?.collect::<DbResult<_>>()?;
    Ok(names.iter().any(|n| n == col))
}

/// Upgrades older databases in place. Safe to run on every start. Dictionary words and snippets
/// gain a random `uid` (their identity across computers), `updated_at` (for newest-wins sync)
/// and `deleted` (deleting is a soft delete so the deletion can sync).
fn migrate(conn: &Connection) -> DbResult<()> {
    for table in ["dictionary", "snippets"] {
        if !has_column(conn, table, "uid")? {
            conn.execute(&format!("ALTER TABLE {table} ADD COLUMN uid TEXT"), [])?;
        }
        if !has_column(conn, table, "updated_at")? {
            conn.execute(&format!("ALTER TABLE {table} ADD COLUMN updated_at INTEGER NOT NULL DEFAULT 0"), [])?;
        }
        if !has_column(conn, table, "deleted")? {
            conn.execute(&format!("ALTER TABLE {table} ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0"), [])?;
        }
        // 1 = changed on this computer and not yet uploaded. Unlike a time cursor this stays
        // correct if the computer's clock jumps backwards.
        if !has_column(conn, table, "dirty")? {
            conn.execute(&format!("ALTER TABLE {table} ADD COLUMN dirty INTEGER NOT NULL DEFAULT 1"), [])?;
        }
        let ids: Vec<i64> = {
            let mut stmt = conn.prepare(&format!("SELECT id FROM {table} WHERE uid IS NULL"))?;
            let rows = stmt.query_map([], |r| r.get(0))?.collect::<DbResult<_>>()?;
            rows
        };
        for id in ids {
            conn.execute(&format!("UPDATE {table} SET uid=?2 WHERE id=?1"), params![id, new_uid()])?;
        }
        conn.execute(&format!("UPDATE {table} SET updated_at=created_at WHERE updated_at=0"), [])?;
        conn.execute(&format!("CREATE UNIQUE INDEX IF NOT EXISTS idx_{table}_uid ON {table}(uid)"), [])?;
    }
    Ok(())
}

impl Db {
    pub fn open(path: &Path) -> DbResult<Db> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> DbResult<Db> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> DbResult<Db> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Db { conn: Mutex::new(conn) })
    }

    fn c(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ---------- meta ----------
    pub fn get_meta(&self, key: &str) -> DbResult<Option<String>> {
        self.c().query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0)).optional()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> DbResult<()> {
        self.c().execute(
            "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn delete_meta(&self, key: &str) -> DbResult<()> {
        self.c().execute("DELETE FROM meta WHERE key=?1", [key])?;
        Ok(())
    }

    // ---------- dictations ----------
    pub fn insert_dictation(&self, item: &HistoryItem) -> DbResult<()> {
        self.c().execute(
            "INSERT INTO dictations (id, created_at, kind, status, mode_id, app_key, app_name, duration_ms, raw_text, clean_text, final_text, selected_text, word_count, audio_path, error, error_kind, inserted, attempts, timings, meta)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
            params![
                item.id, item.created_at, item.kind, item.status, item.mode_id, item.app_key, item.app_name,
                item.duration_ms, item.raw_text, item.clean_text, item.final_text, item.selected_text,
                item.word_count, item.audio_path, item.error, item.error_kind, item.inserted as i64,
                item.attempts, item.timings.as_ref().map(|t| t.to_string()), item.meta.as_ref().map(|m| m.to_string())
            ],
        )?;
        Ok(())
    }

    pub fn set_kind(&self, id: &str, kind: &str) -> DbResult<()> {
        self.c().execute("UPDATE dictations SET kind=?2 WHERE id=?1", params![id, kind])?;
        Ok(())
    }

    pub fn set_status(&self, id: &str, status: Status) -> DbResult<()> {
        self.c().execute("UPDATE dictations SET status=?2 WHERE id=?1", params![id, status.as_str()])?;
        Ok(())
    }

    pub fn set_error(&self, id: &str, status: Status, kind: &str, message: &str) -> DbResult<()> {
        self.c().execute(
            "UPDATE dictations SET status=?2, error_kind=?3, error=?4, attempts=attempts+1 WHERE id=?1",
            params![id, status.as_str(), kind, message],
        )?;
        Ok(())
    }

    pub fn clear_error(&self, id: &str) -> DbResult<()> {
        self.c().execute("UPDATE dictations SET error=NULL, error_kind=NULL WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn set_recording_info(&self, id: &str, duration_ms: i64, audio_path: Option<&str>) -> DbResult<()> {
        self.c().execute(
            "UPDATE dictations SET duration_ms=?2, audio_path=?3 WHERE id=?1",
            params![id, duration_ms, audio_path],
        )?;
        Ok(())
    }

    pub fn set_audio_path(&self, id: &str, audio_path: Option<&str>) -> DbResult<()> {
        self.c().execute("UPDATE dictations SET audio_path=?2 WHERE id=?1", params![id, audio_path])?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_result(
        &self,
        id: &str,
        raw: &str,
        clean: Option<&str>,
        final_text: &str,
        mode_id: &str,
        fixes: i64,
        timings: &serde_json::Value,
        meta: &serde_json::Value,
    ) -> DbResult<()> {
        let words = crate::text::word_count(final_text) as i64;
        self.c().execute(
            "UPDATE dictations SET raw_text=?2, clean_text=?3, final_text=?4, word_count=?5, mode_id=?6, fixes=?7,
             timings=?8, meta=?9, status='done', error=NULL, error_kind=NULL WHERE id=?1",
            params![id, raw, clean, final_text, words, mode_id, fixes, timings.to_string(), meta.to_string()],
        )?;
        Ok(())
    }

    pub fn set_raw_text(&self, id: &str, raw: &str) -> DbResult<()> {
        self.c().execute("UPDATE dictations SET raw_text=?2 WHERE id=?1", params![id, raw])?;
        Ok(())
    }

    /// Adds fields to a dictation's `timings` JSON (keeping what is there).
    pub fn merge_timings(&self, id: &str, extra: &serde_json::Value) -> DbResult<()> {
        self.c().execute(
            "UPDATE dictations SET timings=json_patch(COALESCE(timings, '{}'), ?2) WHERE id=?1",
            params![id, extra.to_string()],
        )?;
        Ok(())
    }

    pub fn set_final_text(&self, id: &str, final_text: &str) -> DbResult<()> {
        let words = crate::text::word_count(final_text) as i64;
        self.c().execute(
            "UPDATE dictations SET final_text=?2, word_count=?3 WHERE id=?1",
            params![id, final_text, words],
        )?;
        Ok(())
    }

    pub fn set_inserted(&self, id: &str, inserted: bool) -> DbResult<()> {
        self.c().execute("UPDATE dictations SET inserted=?2 WHERE id=?1", params![id, inserted as i64])?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> DbResult<Option<HistoryItem>> {
        self.c()
            .query_row(&format!("SELECT {HISTORY_COLS} FROM dictations WHERE id=?1"), [id], history_row)
            .optional()
    }

    pub fn list(&self, q: &HistoryQuery) -> DbResult<Vec<HistoryItem>> {
        let mut sql = format!("SELECT {HISTORY_COLS} FROM dictations WHERE 1=1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(t) = q.text.as_ref().filter(|t| !t.trim().is_empty()) {
            let like = format!("%{}%", t.trim().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            sql.push_str(" AND (final_text LIKE ? ESCAPE '\\' OR raw_text LIKE ? ESCAPE '\\' OR app_name LIKE ? ESCAPE '\\')");
            args.push(Box::new(like.clone()));
            args.push(Box::new(like.clone()));
            args.push(Box::new(like));
        }
        if let Some(f) = q.from {
            sql.push_str(" AND created_at >= ?");
            args.push(Box::new(f));
        }
        if let Some(t) = q.to {
            sql.push_str(" AND created_at <= ?");
            args.push(Box::new(t));
        }
        if let Some(s) = &q.status {
            sql.push_str(" AND status = ?");
            args.push(Box::new(s.clone()));
        }
        if let Some(list) = q.exclude_status.as_ref().filter(|l| !l.is_empty()) {
            sql.push_str(&format!(" AND status NOT IN ({})", vec!["?"; list.len()].join(",")));
            for st in list {
                args.push(Box::new(st.clone()));
            }
        }
        sql.push_str(" ORDER BY created_at DESC LIMIT ? OFFSET ?");
        args.push(Box::new(q.limit.unwrap_or(100).clamp(1, 1000)));
        args.push(Box::new(q.offset.unwrap_or(0).max(0)));
        let conn = self.c();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter().map(|a| a.as_ref())), history_row)?;
        rows.collect()
    }

    /// The most recent dictation that produced text.
    pub fn last_with_text(&self) -> DbResult<Option<HistoryItem>> {
        self.c()
            .query_row(
                &format!("SELECT {HISTORY_COLS} FROM dictations WHERE final_text IS NOT NULL AND final_text != '' ORDER BY created_at DESC LIMIT 1"),
                [],
                history_row,
            )
            .optional()
    }

    pub fn last(&self) -> DbResult<Option<HistoryItem>> {
        self.c()
            .query_row(&format!("SELECT {HISTORY_COLS} FROM dictations ORDER BY created_at DESC LIMIT 1"), [], history_row)
            .optional()
    }

    /// Dictations that still need processing (queued, interrupted mid-way, or left recording
    /// by a crash), oldest first.
    pub fn unfinished(&self) -> DbResult<Vec<HistoryItem>> {
        let conn = self.c();
        let mut stmt = conn.prepare(&format!(
            "SELECT {HISTORY_COLS} FROM dictations WHERE status IN ('queued','recording','transcribing','cleaning') ORDER BY created_at ASC"
        ))?;
        let rows = stmt.query_map([], history_row)?;
        rows.collect()
    }

    /// Deletes a dictation; returns its audio path so the caller can remove the file.
    pub fn delete(&self, id: &str) -> DbResult<Option<String>> {
        let conn = self.c();
        let path: Option<String> = conn
            .query_row("SELECT audio_path FROM dictations WHERE id=?1", [id], |r| r.get(0))
            .optional()?
            .flatten();
        conn.execute("DELETE FROM segments WHERE dictation_id=?1", [id])?;
        conn.execute("DELETE FROM dictations WHERE id=?1", [id])?;
        Ok(path)
    }

    /// Old "No speech" rows where the speech detector never found speech in any piece:
    /// they hold nothing, so they can go. (Rows where it did hear speech stay.)
    pub fn silent_empty_items(&self) -> DbResult<Vec<String>> {
        let conn = self.c();
        let mut stmt = conn.prepare(
            "SELECT id FROM dictations d WHERE status='empty' AND (raw_text IS NULL OR TRIM(raw_text)='') \
             AND NOT EXISTS (SELECT 1 FROM segments s WHERE s.dictation_id=d.id AND s.has_speech=1)",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect()
    }

    /// Audio files older than the cutoff (only for finished dictations).
    pub fn audio_older_than(&self, cutoff_ms: i64) -> DbResult<Vec<(String, String)>> {
        let conn = self.c();
        let mut stmt = conn.prepare(
            "SELECT id, audio_path FROM dictations WHERE created_at < ?1 AND audio_path IS NOT NULL AND status IN ('done','cancelled','empty')",
        )?;
        let rows = stmt.query_map([cutoff_ms], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    pub fn ids_older_than(&self, cutoff_ms: i64) -> DbResult<Vec<String>> {
        let conn = self.c();
        // Failed dictations are kept until they're sorted out (retried or deleted by hand).
        let mut stmt = conn.prepare("SELECT id FROM dictations WHERE created_at < ?1 AND status NOT IN ('queued','recording','transcribing','cleaning','failed')")?;
        let rows = stmt.query_map([cutoff_ms], |r| r.get(0))?;
        rows.collect()
    }

    pub fn recent_final_texts(&self, limit: i64) -> DbResult<Vec<String>> {
        let conn = self.c();
        let mut stmt = conn.prepare(
            "SELECT final_text FROM dictations WHERE status='done' AND final_text IS NOT NULL ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |r| r.get(0))?;
        rows.collect()
    }

    // ---------- segments ----------
    pub fn upsert_segment(&self, s: &SegmentRow) -> DbResult<()> {
        self.c().execute(
            "INSERT INTO segments (dictation_id, idx, start_sample, end_sample, has_speech, status, text, error)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(dictation_id, idx) DO UPDATE SET start_sample=excluded.start_sample, end_sample=excluded.end_sample,
             has_speech=excluded.has_speech, status=excluded.status, text=excluded.text, error=excluded.error",
            params![s.dictation_id, s.idx, s.start_sample, s.end_sample, s.has_speech as i64, s.status, s.text, s.error],
        )?;
        Ok(())
    }

    pub fn segments(&self, id: &str) -> DbResult<Vec<SegmentRow>> {
        let conn = self.c();
        let mut stmt = conn.prepare(
            "SELECT dictation_id, idx, start_sample, end_sample, has_speech, status, text, error FROM segments WHERE dictation_id=?1 ORDER BY idx",
        )?;
        let rows = stmt.query_map([id], |r| {
            Ok(SegmentRow {
                dictation_id: r.get(0)?,
                idx: r.get(1)?,
                start_sample: r.get(2)?,
                end_sample: r.get(3)?,
                has_speech: r.get::<_, i64>(4)? != 0,
                status: r.get(5)?,
                text: r.get(6)?,
                error: r.get(7)?,
            })
        })?;
        rows.collect()
    }

    pub fn clear_segments(&self, id: &str) -> DbResult<()> {
        self.c().execute("DELETE FROM segments WHERE dictation_id=?1", [id])?;
        Ok(())
    }

    // ---------- dictionary ----------
    pub fn dict_all(&self) -> DbResult<Vec<DictEntry>> {
        let conn = self.c();
        let mut stmt = conn.prepare(
            "SELECT id, word, replace_json, sounds_like_json, source, starred, keyword, uses FROM dictionary WHERE hidden=0 AND deleted=0 ORDER BY starred DESC, source='builtin', word COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], dict_row)?;
        rows.collect()
    }

    /// Adds the built-in vocabulary once per starter-list version. Words the user deleted
    /// stay deleted (they are hidden, not removed).
    pub fn seed_starter(&self, version: u32, entries: &[DictEntry]) -> DbResult<usize> {
        let current: u32 = self.get_meta("starter_version")?.and_then(|v| v.parse().ok()).unwrap_or(0);
        if current >= version {
            return Ok(0);
        }
        let mut added = 0;
        {
            let mut conn = self.c();
            let tx = conn.transaction()?;
            for e in entries {
                let r = serde_json::to_string(&e.replace).unwrap();
                let sl = serde_json::to_string(&e.sounds_like).unwrap();
                added += tx.execute(
                    "INSERT OR IGNORE INTO dictionary (word, replace_json, sounds_like_json, source, keyword, created_at, uid, updated_at, dirty) VALUES (?1,?2,?3,'builtin',?4,?5,?6,?5,0)",
                    params![e.word, r, sl, e.keyword as i64, now_ms(), new_uid()],
                )?;
                // Refresh built-in words the user hasn't edited (edits turn them into "user").
                tx.execute(
                    "UPDATE dictionary SET replace_json=?2, sounds_like_json=?3, keyword=?4 WHERE word=?1 AND source='builtin'",
                    params![e.word, r, sl, e.keyword as i64],
                )?;
            }
            tx.commit()?;
        }
        self.set_meta("starter_version", &version.to_string())?;
        Ok(added)
    }

    pub fn dict_add(&self, e: &DictEntry) -> DbResult<i64> {
        let conn = self.c();
        // Re-adding a hidden built-in word or a deleted word brings it back (the UNIQUE word slot
        // is still taken by the old row). A built-in word added by hand becomes the user's own.
        let existing: Option<i64> = conn.query_row("SELECT id FROM dictionary WHERE word=?1", [&e.word], |r| r.get(0)).optional()?;
        if let Some(id) = existing {
            conn.execute(
                "UPDATE dictionary SET hidden=0, deleted=0, dirty=1, replace_json=?2, sounds_like_json=?3, starred=?4, keyword=?5, updated_at=?6,
                 source=CASE WHEN source='builtin' AND ?7<>'builtin' THEN ?7 ELSE source END WHERE id=?1",
                params![id, serde_json::to_string(&e.replace).unwrap(), serde_json::to_string(&e.sounds_like).unwrap(), e.starred as i64, e.keyword as i64, now_ms(), source_str(e.source)],
            )?;
            return Ok(id);
        }
        conn.execute(
            "INSERT INTO dictionary (word, replace_json, sounds_like_json, source, starred, keyword, created_at, uid, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?7)",
            params![e.word, serde_json::to_string(&e.replace).unwrap(), serde_json::to_string(&e.sounds_like).unwrap(), source_str(e.source), e.starred as i64, e.keyword as i64, now_ms(), new_uid()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Editing a built-in word makes it the user's own, so starter-list updates won't undo it.
    pub fn dict_update(&self, e: &DictEntry) -> DbResult<()> {
        let conn = self.c();
        retire_tombstone(&conn, "dictionary", "word", &e.word, e.id)?;
        conn.execute(
            "UPDATE dictionary SET word=?2, replace_json=?3, sounds_like_json=?4, starred=?5, keyword=?6, updated_at=?7, dirty=1,
             source=CASE WHEN source='builtin' THEN 'user' ELSE source END WHERE id=?1",
            params![e.id, e.word, serde_json::to_string(&e.replace).unwrap(), serde_json::to_string(&e.sounds_like).unwrap(), e.starred as i64, e.keyword as i64, now_ms()],
        )?;
        Ok(())
    }

    pub fn dict_delete(&self, id: i64) -> DbResult<()> {
        let conn = self.c();
        let source: Option<String> = conn.query_row("SELECT source FROM dictionary WHERE id=?1", [id], |r| r.get(0)).optional()?;
        if source.as_deref() == Some("builtin") {
            conn.execute("UPDATE dictionary SET hidden=1 WHERE id=?1", [id])?;
        } else {
            // Soft delete: the row stays (hidden from every list) so the deletion can sync.
            conn.execute("UPDATE dictionary SET deleted=1, dirty=1, updated_at=?2 WHERE id=?1", params![id, now_ms()])?;
        }
        Ok(())
    }

    pub fn dict_bump_uses(&self, uses: &[(i64, i64)]) -> DbResult<()> {
        let mut conn = self.c();
        let tx = conn.transaction()?;
        for (id, n) in uses {
            tx.execute("UPDATE dictionary SET uses=uses+?2 WHERE id=?1", params![id, n])?;
        }
        tx.commit()
    }

    // ---------- suggestions ----------
    pub fn suggestions_pending(&self) -> DbResult<Vec<(i64, Suggestion)>> {
        let conn = self.c();
        let mut stmt = conn.prepare("SELECT id, word, variants_json, occurrences FROM suggestions WHERE status='pending' ORDER BY occurrences DESC")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get(0)?,
                Suggestion { word: r.get(1)?, variants: json_vec(&r.get::<_, String>(2)?), occurrences: r.get::<_, i64>(3)? as usize },
            ))
        })?;
        rows.collect()
    }

    /// Records a suggestion unless it was already suggested (pending, accepted or dismissed).
    pub fn suggestion_add(&self, s: &Suggestion) -> DbResult<bool> {
        let n = self.c().execute(
            "INSERT OR IGNORE INTO suggestions (word, variants_json, occurrences, created_at) VALUES (?1,?2,?3,?4)",
            params![s.word, serde_json::to_string(&s.variants).unwrap(), s.occurrences as i64, now_ms()],
        )?;
        Ok(n > 0)
    }

    pub fn suggestion_resolve(&self, id: i64, accepted: bool) -> DbResult<Option<Suggestion>> {
        let conn = self.c();
        let s: Option<Suggestion> = conn
            .query_row("SELECT word, variants_json, occurrences FROM suggestions WHERE id=?1", [id], |r| {
                Ok(Suggestion { word: r.get(0)?, variants: json_vec(&r.get::<_, String>(1)?), occurrences: r.get::<_, i64>(2)? as usize })
            })
            .optional()?;
        conn.execute(
            "UPDATE suggestions SET status=?2 WHERE id=?1",
            params![id, if accepted { "accepted" } else { "dismissed" }],
        )?;
        Ok(s)
    }

    // ---------- snippets ----------
    pub fn snippets_all(&self) -> DbResult<Vec<Snippet>> {
        let conn = self.c();
        let mut stmt = conn.prepare("SELECT id, trigger, expansion, uses FROM snippets WHERE deleted=0 ORDER BY trigger COLLATE NOCASE")?;
        let rows = stmt.query_map([], |r| Ok(Snippet { id: r.get(0)?, trigger: r.get(1)?, expansion: r.get(2)?, uses: r.get(3)? }))?;
        rows.collect()
    }

    pub fn snippet_save(&self, s: &Snippet) -> DbResult<i64> {
        let conn = self.c();
        if s.id > 0 {
            retire_tombstone(&conn, "snippets", "trigger", &s.trigger, s.id)?;
            conn.execute("UPDATE snippets SET trigger=?2, expansion=?3, updated_at=?4, dirty=1 WHERE id=?1", params![s.id, s.trigger, s.expansion, now_ms()])?;
            Ok(s.id)
        } else {
            // Same phrase again (even one that was deleted) updates that row instead of duplicating.
            let existing: Option<i64> = conn.query_row("SELECT id FROM snippets WHERE trigger=?1", [&s.trigger], |r| r.get(0)).optional()?;
            if let Some(id) = existing {
                conn.execute("UPDATE snippets SET expansion=?2, deleted=0, dirty=1, updated_at=?3 WHERE id=?1", params![id, s.expansion, now_ms()])?;
                return Ok(id);
            }
            conn.execute(
                "INSERT INTO snippets (trigger, expansion, created_at, uid, updated_at) VALUES (?1,?2,?3,?4,?3)",
                params![s.trigger, s.expansion, now_ms(), new_uid()],
            )?;
            Ok(conn.last_insert_rowid())
        }
    }

    pub fn snippet_delete(&self, id: i64) -> DbResult<()> {
        self.c().execute("UPDATE snippets SET deleted=1, dirty=1, updated_at=?2 WHERE id=?1", params![id, now_ms()])?;
        Ok(())
    }

    pub fn snippet_bump(&self, id: i64) -> DbResult<()> {
        self.c().execute("UPDATE snippets SET uses=uses+1 WHERE id=?1", [id])?;
        Ok(())
    }

    // ---------- notes ----------
    pub fn notes(&self, search: Option<&str>) -> DbResult<Vec<Note>> {
        let conn = self.c();
        let like = format!("%{}%", search.unwrap_or("").trim());
        let mut stmt = conn.prepare(
            "SELECT id, title, body, pinned, created_at, updated_at FROM notes WHERE title LIKE ?1 OR body LIKE ?1 ORDER BY pinned DESC, updated_at DESC",
        )?;
        let rows = stmt.query_map([like], |r| {
            Ok(Note { id: r.get(0)?, title: r.get(1)?, body: r.get(2)?, pinned: r.get::<_, i64>(3)? != 0, created_at: r.get(4)?, updated_at: r.get(5)? })
        })?;
        rows.collect()
    }

    pub fn note_save(&self, n: &Note) -> DbResult<i64> {
        let conn = self.c();
        let t = now_ms();
        if n.id > 0 {
            conn.execute(
                "UPDATE notes SET title=?2, body=?3, pinned=?4, updated_at=?5 WHERE id=?1",
                params![n.id, n.title, n.body, n.pinned as i64, t],
            )?;
            Ok(n.id)
        } else {
            conn.execute(
                "INSERT INTO notes (title, body, pinned, created_at, updated_at) VALUES (?1,?2,?3,?4,?4)",
                params![n.title, n.body, n.pinned as i64, t],
            )?;
            Ok(conn.last_insert_rowid())
        }
    }

    pub fn note_delete(&self, id: i64) -> DbResult<()> {
        self.c().execute("DELETE FROM notes WHERE id=?1", [id])?;
        Ok(())
    }

    // ---------- per-app mode learning ----------
    pub fn record_mode_choice(&self, app_key: &str, mode_id: &str) -> DbResult<()> {
        self.c().execute(
            "INSERT INTO mode_choices (app_key, mode_id, count, last_at) VALUES (?1,?2,1,?3)
             ON CONFLICT(app_key, mode_id) DO UPDATE SET count=count+1, last_at=excluded.last_at",
            params![app_key, mode_id, now_ms()],
        )?;
        Ok(())
    }

    /// The mode chosen most for an app, if it was picked at least `min` times and is the
    /// clear majority of manual choices there.
    pub fn learned_mode(&self, app_key: &str, min: i64) -> DbResult<Option<String>> {
        let conn = self.c();
        let mut stmt = conn.prepare("SELECT mode_id, count FROM mode_choices WHERE app_key=?1 ORDER BY count DESC, last_at DESC")?;
        let rows: Vec<(String, i64)> = stmt.query_map([app_key], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<DbResult<_>>()?;
        let total: i64 = rows.iter().map(|r| r.1).sum();
        Ok(rows.first().filter(|(_, c)| *c >= min && *c * 3 >= total * 2).map(|(m, _)| m.clone()))
    }

    // ---------- stats ----------
    pub fn stats(&self, typing_wpm: u32, now: chrono::DateTime<Local>) -> DbResult<Stats> {
        self.stats_with(typing_wpm, now, false)
    }

    /// Stats for this computer, or (`include_remote`) combined with the numbers other computers
    /// uploaded. A dictation present both locally and in `remote_stats` is counted once.
    pub fn stats_with(&self, typing_wpm: u32, now: chrono::DateTime<Local>, include_remote: bool) -> DbResult<Stats> {
        let conn = self.c();
        let mut stmt = conn.prepare(
            "SELECT created_at, word_count, duration_ms, COALESCE(app_name,''), fixes, id FROM dictations WHERE status='done' AND kind='dictation'",
        )?;
        let local: Vec<(i64, i64, i64, String, i64, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
            .collect::<DbResult<_>>()?;
        drop(stmt);
        let mut rows: Vec<(i64, i64, i64, String, i64)> = Vec::with_capacity(local.len());
        let mut seen: HashSet<String> = HashSet::new();
        for (a, b, c2, d, e, id) in local {
            rows.push((a, b, c2, d, e));
            if include_remote {
                seen.insert(id);
            }
        }
        if include_remote {
            let mut stmt = conn.prepare("SELECT uid, created_at, words, duration_ms, COALESCE(app,''), fixes FROM remote_stats")?;
            let remote: Vec<(String, i64, i64, i64, String, i64)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
                .collect::<DbResult<_>>()?;
            for (uid, at, words, dur, app, fixes) in remote {
                if !seen.contains(&uid) {
                    rows.push((at, words, dur, app, fixes));
                }
            }
        }
        let mut stmt = conn.prepare(
            "SELECT timings FROM dictations WHERE status='done' AND kind='dictation' AND timings IS NOT NULL ORDER BY created_at DESC LIMIT ?1",
        )?;
        let timings: Vec<String> = stmt.query_map([SPEED_WINDOW], |r| r.get(0))?.collect::<DbResult<_>>()?;
        drop(stmt);
        drop(conn);
        let mut s = compute_stats(&rows, typing_wpm, now);
        s.speed = compute_speed(&timings);
        s.scope = if include_remote { "all-computers" } else { "this-computer" }.to_string();
        Ok(s)
    }

    /// Completed dictations newer than `since` (their `created_at`), as numbers-only rows for the
    /// account. The dictation id is the row's uid.
    pub fn stats_rows_since(&self, since: i64, limit: i64) -> DbResult<Vec<StatRow>> {
        let conn = self.c();
        let mut stmt = conn.prepare(
            "SELECT id, created_at, word_count, duration_ms, app_name, fixes FROM dictations
             WHERE status='done' AND kind='dictation' AND created_at > ?1 ORDER BY created_at LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![since, limit], |r| {
            Ok(StatRow { uid: r.get(0)?, created_at: r.get(1)?, words: r.get(2)?, duration_ms: r.get(3)?, app: r.get(4)?, fixes: r.get(5)?, device_id: None, server_at: None })
        })?;
        rows.collect()
    }

    /// Stores other computers' stat rows. Returns how many were new.
    pub fn remote_stats_add(&self, rows: &[StatRow]) -> DbResult<usize> {
        let mut conn = self.c();
        let tx = conn.transaction()?;
        let mut added = 0;
        for r in rows {
            added += tx.execute(
                "INSERT OR IGNORE INTO remote_stats (uid, created_at, words, duration_ms, app, fixes) VALUES (?1,?2,?3,?4,?5,?6)",
                params![r.uid, r.created_at, r.words, r.duration_ms, r.app, r.fixes],
            )?;
        }
        tx.commit()?;
        Ok(added)
    }

    pub fn remote_stats_clear(&self) -> DbResult<()> {
        self.c().execute("DELETE FROM remote_stats", [])?;
        Ok(())
    }

    // ---------- sync (dictionary words and snippets) ----------

    /// Dictionary words and snippets (including deleted ones) changed after `since`, in the API
    /// shape. Built-in starter words are never included unless the user has made them their own.
    pub fn items_changed_since(&self, since: i64) -> DbResult<Vec<SyncItem>> {
        self.items_query(since, false)
    }

    /// Words and snippets changed on this computer and not yet uploaded. Tracked with a flag
    /// per row rather than a clock cursor, so a clock that jumps backwards can't hide changes.
    pub fn items_dirty(&self) -> DbResult<Vec<SyncItem>> {
        self.items_query(i64::MIN, true)
    }

    /// Flags every shareable word and snippet as not yet uploaded. Used on a first merge with an
    /// account, so a computer's existing words reach a brand-new account.
    pub fn mark_all_items_dirty(&self) -> DbResult<()> {
        let conn = self.c();
        conn.execute("UPDATE dictionary SET dirty=1 WHERE source <> 'builtin'", [])?;
        conn.execute("UPDATE snippets SET dirty=1", [])?;
        Ok(())
    }

    /// Marks uploaded items as clean. A row edited again while the upload was running (its
    /// `updated_at` moved) stays dirty.
    pub fn mark_items_pushed(&self, items: &[SyncItem]) -> DbResult<()> {
        let mut conn = self.c();
        let tx = conn.transaction()?;
        for it in items {
            let table = if it.kind == "snippet" { "snippets" } else { "dictionary" };
            tx.execute(&format!("UPDATE {table} SET dirty=0 WHERE uid=?1 AND updated_at=?2"), params![it.uid, it.updated_at])?;
        }
        tx.commit()
    }

    fn items_query(&self, since: i64, only_dirty: bool) -> DbResult<Vec<SyncItem>> {
        let conn = self.c();
        let dirty = only_dirty as i64;
        let mut out = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT uid, word, replace_json, sounds_like_json, source, starred, keyword, updated_at, deleted
             FROM dictionary WHERE updated_at > ?1 AND source <> 'builtin' AND (?2=0 OR dirty=1) ORDER BY updated_at",
        )?;
        let rows = stmt.query_map(params![since, dirty], |r| {
            Ok(SyncItem {
                kind: "dictionary".into(),
                uid: r.get(0)?,
                data: serde_json::json!({
                    "word": r.get::<_, String>(1)?,
                    "replace": json_vec(&r.get::<_, String>(2)?),
                    "soundsLike": json_vec(&r.get::<_, String>(3)?),
                    "source": r.get::<_, String>(4)?,
                    "starred": r.get::<_, i64>(5)? != 0,
                    "keyword": r.get::<_, i64>(6)? != 0,
                }),
                updated_at: r.get(7)?,
                deleted: r.get::<_, i64>(8)? != 0,
            })
        })?;
        for r in rows {
            out.push(r?);
        }
        drop(stmt);
        let mut stmt = conn.prepare("SELECT uid, trigger, expansion, updated_at, deleted FROM snippets WHERE updated_at > ?1 AND (?2=0 OR dirty=1) ORDER BY updated_at")?;
        let rows = stmt.query_map(params![since, dirty], |r| {
            Ok(SyncItem {
                kind: "snippet".into(),
                uid: r.get(0)?,
                data: serde_json::json!({ "trigger": r.get::<_, String>(1)?, "expansion": r.get::<_, String>(2)? }),
                updated_at: r.get(3)?,
                deleted: r.get::<_, i64>(4)? != 0,
            })
        })?;
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Merges items received from the account. Newest `updated_at` wins. An item is matched by
    /// its uid first, then by word/trigger, so the same word added on two computers becomes one
    /// entry rather than a duplicate. Returns how many local rows changed.
    pub fn apply_remote_items(&self, items: &[SyncItem]) -> DbResult<usize> {
        let mut conn = self.c();
        let tx = conn.transaction()?;
        let mut changed = 0;
        for it in items {
            let did = match it.kind.as_str() {
                "dictionary" => apply_dict_item(&tx, it)?,
                "snippet" => apply_snippet_item(&tx, it)?,
                _ => false,
            };
            changed += did as usize;
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Writes a consistent copy of the whole database to `path` (replacing any file there).
    pub fn backup_to(&self, path: &Path) -> DbResult<()> {
        let _ = std::fs::remove_file(path);
        let p = path.to_string_lossy().replace('\'', "''");
        self.c().execute_batch(&format!("VACUUM INTO '{p}'"))
    }
}

/// rows: (created_at ms, words, duration ms, app name, fixes)
pub fn compute_stats(rows: &[(i64, i64, i64, String, i64)], typing_wpm: u32, now: chrono::DateTime<Local>) -> Stats {
    use std::collections::{BTreeMap, HashMap};
    let today = now.date_naive();
    let week_start = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let mut per_day: BTreeMap<NaiveDate, i64> = BTreeMap::new();
    let mut per_app: HashMap<String, i64> = HashMap::new();
    let mut s = Stats::default();
    let mut speaking_ms = 0i64;
    let mut speaking_ms_week = 0i64;
    for (at, words, dur, app, fixes) in rows {
        let Some(dt) = Local.timestamp_millis_opt(*at).single() else { continue };
        let day = dt.date_naive();
        *per_day.entry(day).or_default() += words;
        if !app.is_empty() {
            *per_app.entry(app.clone()).or_default() += words;
        }
        s.total_words += words;
        s.total_dictations += 1;
        s.fixes += fixes;
        speaking_ms += dur;
        if day >= week_start {
            s.words_this_week += words;
            speaking_ms_week += dur;
        }
        if day == today {
            s.words_today += words;
        }
    }
    let minutes = speaking_ms as f64 / 60_000.0;
    s.wpm = if minutes > 0.05 { (s.total_words as f64 / minutes * 10.0).round() / 10.0 } else { 0.0 };
    let typing = typing_wpm.max(1) as f64;
    s.minutes_saved = (s.total_words as f64 / typing - minutes).max(0.0);
    s.minutes_saved_this_week = (s.words_this_week as f64 / typing - speaking_ms_week as f64 / 60_000.0).max(0.0);
    // Streaks.
    let mut streak = 0;
    let mut d = today;
    if !per_day.contains_key(&d) {
        d -= chrono::Duration::days(1); // today not yet dictated doesn't break the streak
    }
    while per_day.contains_key(&d) {
        streak += 1;
        d -= chrono::Duration::days(1);
    }
    s.streak_days = streak;
    let mut longest = 0;
    let mut run = 0;
    let mut prev: Option<NaiveDate> = None;
    for day in per_day.keys() {
        run = match prev {
            Some(p) if *day - p == chrono::Duration::days(1) => run + 1,
            _ => 1,
        };
        longest = longest.max(run);
        prev = Some(*day);
    }
    s.longest_streak = longest;
    for i in (0..365).rev() {
        let day = today - chrono::Duration::days(i);
        s.daily.push(DayCount { date: day.format("%Y-%m-%d").to_string(), words: *per_day.get(&day).unwrap_or(&0) });
    }
    let mut apps: Vec<AppCount> = per_app.into_iter().map(|(name, words)| AppCount { name, words }).collect();
    apps.sort_by(|a, b| b.words.cmp(&a.words).then(a.name.cmp(&b.name)));
    apps.truncate(8);
    s.top_apps = apps;
    s.scope = "this-computer".to_string();
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::starter_entries;

    fn item(id: &str, at: i64, text: &str) -> HistoryItem {
        HistoryItem {
            id: id.into(),
            created_at: at,
            kind: "dictation".into(),
            status: "done".into(),
            final_text: Some(text.into()),
            word_count: crate::text::word_count(text) as i64,
            duration_ms: 6000,
            app_name: Some("Gmail".into()),
            ..Default::default()
        }
    }

    #[test]
    fn history_crud_and_search() {
        let db = Db::open_in_memory().unwrap();
        db.insert_dictation(&item("a", 1000, "Shalom and parnasa")).unwrap();
        db.insert_dictation(&item("b", 2000, "Meeting on Tuesday")).unwrap();
        assert_eq!(db.list(&HistoryQuery::default()).unwrap().len(), 2);
        let r = db.list(&HistoryQuery { text: Some("parnasa".into()), ..Default::default() }).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "a");
        let r = db.list(&HistoryQuery { from: Some(1500), ..Default::default() }).unwrap();
        assert_eq!(r[0].id, "b");
        // LIKE wildcards in the query are literal.
        assert!(db.list(&HistoryQuery { text: Some("%".into()), ..Default::default() }).unwrap().is_empty());
        assert_eq!(db.last_with_text().unwrap().unwrap().id, "b");
        db.set_inserted("b", true).unwrap();
        assert!(db.get("b").unwrap().unwrap().inserted);
        assert_eq!(db.delete("a").unwrap(), None);
        assert!(db.get("a").unwrap().is_none());
    }

    #[test]
    fn unfinished_and_errors() {
        let db = Db::open_in_memory().unwrap();
        let mut it = item("x", 1, "");
        it.status = "recording".into();
        it.final_text = None;
        db.insert_dictation(&it).unwrap();
        assert_eq!(db.unfinished().unwrap().len(), 1);
        db.set_error("x", Status::Queued, "network", "offline").unwrap();
        let x = db.get("x").unwrap().unwrap();
        assert_eq!(x.status, "queued");
        assert_eq!(x.attempts, 1);
        db.set_result("x", "raw", Some("clean"), "Final text", "auto", 2, &serde_json::json!({}), &serde_json::json!({})).unwrap();
        let x = db.get("x").unwrap().unwrap();
        assert_eq!(x.status, "done");
        assert_eq!(x.word_count, 2);
        assert!(x.error.is_none());
        assert!(db.unfinished().unwrap().is_empty());
    }

    #[test]
    fn segments_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        let mut s = SegmentRow { dictation_id: "d".into(), idx: 0, start_sample: 0, end_sample: 100, has_speech: true, status: "pending".into(), text: None, error: None };
        db.upsert_segment(&s).unwrap();
        s.status = "done".into();
        s.text = Some("hello".into());
        db.upsert_segment(&s).unwrap();
        let segs = db.segments("d").unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text.as_deref(), Some("hello"));
    }

    #[test]
    fn dictionary_seeding_and_hiding() {
        let db = Db::open_in_memory().unwrap();
        let n = db.seed_starter(1, &starter_entries()).unwrap();
        assert!(n > 100);
        assert_eq!(db.seed_starter(1, &starter_entries()).unwrap(), 0);
        let all = db.dict_all().unwrap();
        let z = all.iter().find(|e| e.word == "zechus").unwrap();
        assert_eq!(z.source, EntrySource::Builtin);
        db.dict_delete(z.id).unwrap();
        assert!(!db.dict_all().unwrap().iter().any(|e| e.word == "zechus"));
        // Re-seeding a newer version doesn't resurrect deleted words.
        db.seed_starter(2, &starter_entries()).unwrap();
        assert!(!db.dict_all().unwrap().iter().any(|e| e.word == "zechus"));
        // Adding it by hand brings it back.
        db.dict_add(&DictEntry::user("zechus")).unwrap();
        assert!(db.dict_all().unwrap().iter().any(|e| e.word == "zechus"));
        let id = db.dict_add(&DictEntry::user("Menachem")).unwrap();
        db.dict_bump_uses(&[(id, 3)]).unwrap();
        assert_eq!(db.dict_all().unwrap().iter().find(|e| e.id == id).unwrap().uses, 3);
        db.dict_delete(id).unwrap();
        assert!(!db.dict_all().unwrap().iter().any(|e| e.id == id));
    }

    #[test]
    fn suggestions_are_not_repeated() {
        let db = Db::open_in_memory().unwrap();
        let s = Suggestion { word: "Menachem".into(), variants: vec!["Menahem".into()], occurrences: 3 };
        assert!(db.suggestion_add(&s).unwrap());
        assert!(!db.suggestion_add(&s).unwrap());
        let pending = db.suggestions_pending().unwrap();
        assert_eq!(pending.len(), 1);
        db.suggestion_resolve(pending[0].0, false).unwrap();
        assert!(db.suggestions_pending().unwrap().is_empty());
        assert!(!db.suggestion_add(&s).unwrap());
    }

    #[test]
    fn snippets_and_notes() {
        let db = Db::open_in_memory().unwrap();
        let id = db.snippet_save(&Snippet { id: 0, trigger: "my email".into(), expansion: "a@b.c".into(), uses: 0 }).unwrap();
        // Same trigger, different case → update, not duplicate.
        db.snippet_save(&Snippet { id: 0, trigger: "My Email".into(), expansion: "x@y.z".into(), uses: 0 }).unwrap();
        let all = db.snippets_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].expansion, "x@y.z");
        db.snippet_delete(id).unwrap();
        assert!(db.snippets_all().unwrap().is_empty());
        let nid = db.note_save(&Note { title: "Ideas".into(), body: "zechus".into(), ..Default::default() }).unwrap();
        assert_eq!(db.notes(Some("zech")).unwrap().len(), 1);
        db.note_delete(nid).unwrap();
        assert!(db.notes(None).unwrap().is_empty());
    }

    #[test]
    fn learns_app_modes_by_majority() {
        let db = Db::open_in_memory().unwrap();
        db.record_mode_choice("slack.exe", "casual").unwrap();
        db.record_mode_choice("slack.exe", "casual").unwrap();
        assert_eq!(db.learned_mode("slack.exe", 3).unwrap(), None);
        db.record_mode_choice("slack.exe", "casual").unwrap();
        assert_eq!(db.learned_mode("slack.exe", 3).unwrap().as_deref(), Some("casual"));
        db.record_mode_choice("slack.exe", "professional").unwrap();
        db.record_mode_choice("slack.exe", "professional").unwrap();
        assert_eq!(db.learned_mode("slack.exe", 3).unwrap(), None);
    }

    #[test]
    fn stats_streaks_and_time_saved() {
        let now = Local.with_ymd_and_hms(2026, 11, 20, 15, 0, 0).unwrap();
        let day = |d: i64| (now - chrono::Duration::days(d)).timestamp_millis();
        let rows = vec![
            (day(0), 100, 60_000, "Gmail".to_string(), 2),
            (day(1), 50, 30_000, "Slack".to_string(), 0),
            (day(2), 50, 30_000, "Gmail".to_string(), 1),
            (day(10), 80, 60_000, "Word".to_string(), 0),
            (day(11), 80, 60_000, "Word".to_string(), 0),
        ];
        let s = compute_stats(&rows, 40, now);
        assert_eq!(s.total_words, 360);
        assert_eq!(s.streak_days, 3);
        assert_eq!(s.longest_streak, 3);
        assert_eq!(s.words_today, 100);
        assert_eq!(s.fixes, 3);
        // Friday 20 Nov: week started Monday 16 Nov → days 0..4 count.
        assert_eq!(s.words_this_week, 200);
        // 360 words at 40 wpm = 9 min typing; spoke 4 min → 5 min saved.
        assert!((s.minutes_saved - 5.0).abs() < 0.01);
        assert!((s.wpm - 90.0).abs() < 0.1);
        assert_eq!(s.daily.len(), 365);
        assert_eq!(s.daily.last().unwrap().words, 100);
        assert_eq!(s.top_apps[0].name, "Word");
    }

    // ---------- sync support ----------

    #[test]
    fn old_database_is_migrated_without_losing_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        {
            // The schema as it was before accounts existed.
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE dictionary (id INTEGER PRIMARY KEY AUTOINCREMENT, word TEXT NOT NULL UNIQUE COLLATE NOCASE,
                   replace_json TEXT NOT NULL DEFAULT '[]', sounds_like_json TEXT NOT NULL DEFAULT '[]', source TEXT NOT NULL,
                   starred INTEGER NOT NULL DEFAULT 0, keyword INTEGER NOT NULL DEFAULT 1, uses INTEGER NOT NULL DEFAULT 0,
                   hidden INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL);
                 CREATE TABLE snippets (id INTEGER PRIMARY KEY AUTOINCREMENT, trigger TEXT NOT NULL UNIQUE COLLATE NOCASE,
                   expansion TEXT NOT NULL, uses INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL);
                 INSERT INTO dictionary (word, source, created_at) VALUES ('Menachem','user',1000),('Shabbos','user',2000);
                 INSERT INTO snippets (trigger, expansion, created_at) VALUES ('my email','a@b.c',3000);",
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.dict_all().unwrap().len(), 2);
        assert_eq!(db.snippets_all().unwrap().len(), 1);
        let items = db.items_changed_since(0).unwrap();
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(|i| !i.uid.is_empty() && !i.deleted));
        let m = items.iter().find(|i| i.data["word"] == "Menachem").unwrap();
        assert_eq!(m.updated_at, 1000, "updated_at is backfilled from created_at");
        let uids: HashSet<_> = items.iter().map(|i| i.uid.clone()).collect();
        assert_eq!(uids.len(), 3, "uids are unique");
        drop(db);
        // Opening again keeps the same uids.
        let db = Db::open(&path).unwrap();
        let again: HashSet<_> = db.items_changed_since(0).unwrap().into_iter().map(|i| i.uid).collect();
        assert_eq!(uids, again);
    }

    #[test]
    fn deleting_is_soft_and_readding_works() {
        let db = Db::open_in_memory().unwrap();
        let id = db.dict_add(&DictEntry::user("Menachem")).unwrap();
        db.dict_delete(id).unwrap();
        assert!(db.dict_all().unwrap().is_empty());
        let deleted = db.items_changed_since(0).unwrap();
        assert_eq!(deleted.len(), 1);
        assert!(deleted[0].deleted);
        // Adding the same word again revives it (same uid, so the account sees an update).
        let id2 = db.dict_add(&DictEntry::user("menachem")).unwrap();
        assert_eq!(id, id2);
        assert_eq!(db.dict_all().unwrap().len(), 1);
        let it = db.items_changed_since(0).unwrap();
        assert_eq!(it[0].uid, deleted[0].uid);
        assert!(!it[0].deleted);

        let sid = db.snippet_save(&Snippet { id: 0, trigger: "my email".into(), expansion: "a@b.c".into(), uses: 0 }).unwrap();
        db.snippet_delete(sid).unwrap();
        assert!(db.snippets_all().unwrap().is_empty());
        let sid2 = db.snippet_save(&Snippet { id: 0, trigger: "my email".into(), expansion: "new".into(), uses: 0 }).unwrap();
        assert_eq!(sid, sid2);
        assert_eq!(db.snippets_all().unwrap()[0].expansion, "new");
        // Renaming another word onto a deleted word's text works too.
        let a = db.dict_add(&DictEntry::user("Alpha")).unwrap();
        let b = db.dict_add(&DictEntry::user("Beta")).unwrap();
        db.dict_delete(a).unwrap();
        let mut e = db.dict_all().unwrap().into_iter().find(|e| e.id == b).unwrap();
        e.word = "Alpha".into();
        db.dict_update(&e).unwrap();
        assert!(db.dict_all().unwrap().iter().any(|e| e.word == "Alpha"));
    }

    #[test]
    fn builtin_words_are_not_synced_until_the_user_edits_them() {
        let db = Db::open_in_memory().unwrap();
        db.seed_starter(1, &starter_entries()).unwrap();
        assert!(db.items_changed_since(0).unwrap().is_empty());
        let mut z = db.dict_all().unwrap().into_iter().find(|e| e.word == "zechus").unwrap();
        z.replace = vec!["zechuss".into()];
        db.dict_update(&z).unwrap();
        let items = db.items_changed_since(0).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].data["word"], "zechus");
        assert_eq!(items[0].data["source"], "user");
    }

    fn dict_item(uid: &str, word: &str, at: i64, deleted: bool) -> SyncItem {
        SyncItem { kind: "dictionary".into(), uid: uid.into(), data: serde_json::json!({"word": word, "replace": ["x"], "source": "user"}), updated_at: at, deleted }
    }

    #[test]
    fn remote_items_merge_newest_wins_and_match_by_word() {
        let db = Db::open_in_memory().unwrap();
        db.dict_add(&DictEntry::user("Menachem")).unwrap();
        let local = db.items_changed_since(0).unwrap().remove(0);
        // Same word, different uid, newer → one entry, taking the remote identity and content.
        let n = db.apply_remote_items(&[dict_item("remote-1", "menachem", local.updated_at + 10, false)]).unwrap();
        assert_eq!(n, 1);
        let all = db.dict_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].replace, vec!["x"]);
        assert_eq!(db.items_changed_since(0).unwrap()[0].uid, "remote-1");
        // An older version of the same uid is ignored.
        assert_eq!(db.apply_remote_items(&[dict_item("remote-1", "Menachem", 5, false)]).unwrap(), 0);
        // A remote deletion that is newer removes it; an older one does not.
        assert_eq!(db.apply_remote_items(&[dict_item("remote-1", "Menachem", local.updated_at + 5, true)]).unwrap(), 0);
        assert_eq!(db.dict_all().unwrap().len(), 1);
        assert_eq!(db.apply_remote_items(&[dict_item("remote-1", "Menachem", local.updated_at + 20, true)]).unwrap(), 1);
        assert!(db.dict_all().unwrap().is_empty());
        // Same word, different uid, but OLDER than ours → ours stays.
        db.dict_add(&DictEntry::user("Rivka")).unwrap();
        let mine = db.items_changed_since(0).unwrap().into_iter().find(|i| i.data["word"] == "Rivka").unwrap();
        assert_eq!(db.apply_remote_items(&[dict_item("other", "Rivka", 1, false)]).unwrap(), 0);
        assert_eq!(db.dict_all().unwrap()[0].replace, Vec::<String>::new());
        assert!(db.items_changed_since(0).unwrap().iter().any(|i| i.uid == mine.uid));
        // A word from the account that we never had is added.
        db.apply_remote_items(&[dict_item("new", "Chaya", 50, false)]).unwrap();
        assert!(db.dict_all().unwrap().iter().any(|e| e.word == "Chaya"));
        // Snippets work the same way.
        let sn = SyncItem { kind: "snippet".into(), uid: "s1".into(), data: serde_json::json!({"trigger": "sig", "expansion": "Best"}), updated_at: 10, deleted: false };
        assert_eq!(db.apply_remote_items(&[sn.clone()]).unwrap(), 1);
        assert_eq!(db.snippets_all().unwrap()[0].expansion, "Best");
        let del = SyncItem { deleted: true, updated_at: 20, data: serde_json::Value::Null, ..sn };
        assert_eq!(db.apply_remote_items(&[del]).unwrap(), 1);
        assert!(db.snippets_all().unwrap().is_empty());
    }

    #[test]
    fn remote_item_takes_over_a_builtin_word() {
        let db = Db::open_in_memory().unwrap();
        db.seed_starter(1, &starter_entries()).unwrap();
        let before = db.dict_all().unwrap().len();
        db.apply_remote_items(&[dict_item("u", "zechus", 1, false)]).unwrap();
        assert_eq!(db.dict_all().unwrap().len(), before);
        let z = db.dict_all().unwrap().into_iter().find(|e| e.word == "zechus").unwrap();
        assert_eq!(z.source, EntrySource::User);
        assert_eq!(z.replace, vec!["x"]);
    }

    #[test]
    fn sync_item_json_is_tolerant_about_shapes() {
        let it: SyncItem = serde_json::from_str(r#"{"kind":"snippet","uid":"a","data":"{\"trigger\":\"t\",\"expansion\":\"e\"}","updated_at":5,"deleted":1}"#).unwrap();
        assert!(it.deleted);
        assert_eq!(it.data["trigger"], "t");
        let out = serde_json::to_value(&it).unwrap();
        assert_eq!(out["deleted"], 1);
    }

    #[test]
    fn stats_rows_and_combined_scope() {
        let db = Db::open_in_memory().unwrap();
        db.insert_dictation(&item("a", 1000, "one two three")).unwrap();
        db.insert_dictation(&item("b", 2000, "four five")).unwrap();
        let rows = db.stats_rows_since(1000, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], StatRow { uid: "b".into(), created_at: 2000, words: 2, duration_ms: 6000, app: Some("Gmail".into()), fixes: 0, device_id: None, server_at: None });
        let now = Local.with_ymd_and_hms(2026, 11, 20, 15, 0, 0).unwrap();
        let remote = vec![
            StatRow { uid: "b".into(), created_at: 2000, words: 2, duration_ms: 6000, app: None, fixes: 0, device_id: Some("d1".into()), server_at: None },
            StatRow { uid: "r1".into(), created_at: 3000, words: 10, duration_ms: 6000, app: Some("Word".into()), fixes: 1, device_id: Some("d2".into()), server_at: None },
        ];
        assert_eq!(db.remote_stats_add(&remote).unwrap(), 2);
        assert_eq!(db.remote_stats_add(&remote).unwrap(), 0);
        let local_only = db.stats(40, now).unwrap();
        assert_eq!(local_only.total_words, 5);
        assert_eq!(local_only.scope, "this-computer");
        let all = db.stats_with(40, now, true).unwrap();
        assert_eq!(all.total_words, 15, "the duplicate of local dictation b is counted once");
        assert_eq!(all.total_dictations, 3);
        assert_eq!(all.scope, "all-computers");
        db.remote_stats_clear().unwrap();
        assert_eq!(db.stats_with(40, now, true).unwrap().total_words, 5);
    }

    #[test]
    fn backup_copy_opens_with_the_same_data() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("a.db")).unwrap();
        db.dict_add(&DictEntry::user("Menachem")).unwrap();
        let copy = dir.path().join("copy.db");
        db.backup_to(&copy).unwrap();
        db.backup_to(&copy).unwrap(); // replacing an existing backup works
        let b = Db::open(&copy).unwrap();
        assert_eq!(b.dict_all().unwrap().len(), 1);
    }

    #[test]
    fn unpushed_changes_are_tracked_by_flag_not_by_clock() {
        let db = Db::open_in_memory().unwrap();
        let id = db.dict_add(&DictEntry::user("Menachem")).unwrap();
        db.snippet_save(&Snippet { id: 0, trigger: "sig".into(), expansion: "Best".into(), uses: 0 }).unwrap();
        let dirty = db.items_dirty().unwrap();
        assert_eq!(dirty.len(), 2);
        db.mark_items_pushed(&dirty).unwrap();
        assert!(db.items_dirty().unwrap().is_empty());
        // The computer's clock jumps back: the new edit gets an OLD timestamp, but is still sent.
        let mut e = db.dict_all().unwrap().remove(0);
        e.replace = vec!["x".into()];
        db.dict_update(&e).unwrap();
        db.c().execute("UPDATE dictionary SET updated_at=1 WHERE id=?1", [id]).unwrap();
        assert!(db.items_changed_since(10).unwrap().iter().all(|i| i.kind != "dictionary"), "a time cursor would miss it");
        assert_eq!(db.items_dirty().unwrap().len(), 1);
        // An edit made while an upload was in flight stays dirty.
        let sent = db.items_dirty().unwrap();
        db.dict_update(&e).unwrap();
        db.mark_items_pushed(&sent).unwrap();
        assert_eq!(db.items_dirty().unwrap().len(), 1);
        // Items that arrived from the account are not dirty (nothing to send back).
        db.apply_remote_items(&[dict_item("r", "Chaya", 5, false)]).unwrap();
        assert!(!db.items_dirty().unwrap().iter().any(|i| i.uid == "r"));
        // Deleting marks dirty too.
        let rid = db.dict_all().unwrap().into_iter().find(|e| e.word == "Chaya").unwrap().id;
        db.dict_delete(rid).unwrap();
        assert!(db.items_dirty().unwrap().iter().any(|i| i.uid == "r" && i.deleted));
    }

    #[test]
    fn speed_uses_medians_and_ignores_unmeasured_rows() {
        let t = |tr: u64, cl: u64, tot: u64, fast: bool| {
            serde_json::json!({"transcribeWaitMs": tr, "cleanupMs": cl, "totalAfterStopMs": tot, "cleanupFast": fast}).to_string()
        };
        let rows = vec![
            t(600, 800, 1400, true),
            t(900, 1500, 2400, false),
            t(700, 900, 1600, true),
            t(0, 1200, 1200, false), // re-processed from saved text: not a real wait
            "not json".into(),
            r#"{"recordingMs":1000}"#.into(), // old row without the new fields
        ];
        let s = compute_speed(&rows);
        assert_eq!(s.samples, 3);
        assert_eq!(s.transcribe_wait_median_ms, 700);
        assert_eq!(s.cleanup_median_ms, 900);
        assert_eq!(s.total_wait_median_ms, 1600);
        assert!((s.fast_model_share - 2.0 / 3.0).abs() < 1e-9);
        let even = compute_speed(&[t(100, 100, 200, false), t(300, 300, 400, false)]);
        assert_eq!(even.total_wait_median_ms, 300);
        assert_eq!(compute_speed(&[]), Speed::default());
    }

    #[test]
    fn speed_covers_only_the_last_50_dictations_in_stats() {
        let db = Db::open_in_memory().unwrap();
        for i in 0..60i64 {
            let mut it = item(&format!("d{i}"), 1_000 + i, "hello there");
            let total = if i < 10 { 9000 } else { 1000 };
            it.timings = Some(serde_json::json!({"transcribeWaitMs": 500, "cleanupMs": 400, "totalAfterStopMs": total, "cleanupFast": false}));
            db.insert_dictation(&it).unwrap();
        }
        let s = db.stats(40, Local::now()).unwrap();
        assert_eq!(s.speed.samples, 50);
        assert_eq!(s.speed.total_wait_median_ms, 1000);
    }
}

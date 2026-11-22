//! Local SQLite database: dictation history (text + audio path), segments of long
//! recordings, dictionary, snippets, scratchpad notes, and usage statistics.

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
CREATE TABLE IF NOT EXISTS mode_choices (
  app_key TEXT NOT NULL, mode_id TEXT NOT NULL, count INTEGER NOT NULL DEFAULT 0,
  last_at INTEGER NOT NULL,
  PRIMARY KEY (app_key, mode_id)
);
"#;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
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
            "SELECT id, word, replace_json, sounds_like_json, source, starred, keyword, uses FROM dictionary WHERE hidden=0 ORDER BY starred DESC, source='builtin', word COLLATE NOCASE",
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
                    "INSERT OR IGNORE INTO dictionary (word, replace_json, sounds_like_json, source, keyword, created_at) VALUES (?1,?2,?3,'builtin',?4,?5)",
                    params![e.word, r, sl, e.keyword as i64, now_ms()],
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
        // Re-adding a hidden built-in word un-hides it.
        let existing: Option<(i64, i64)> = conn
            .query_row("SELECT id, hidden FROM dictionary WHERE word=?1", [&e.word], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        if let Some((id, _)) = existing {
            conn.execute(
                "UPDATE dictionary SET hidden=0, replace_json=?2, sounds_like_json=?3, starred=?4, keyword=?5 WHERE id=?1",
                params![id, serde_json::to_string(&e.replace).unwrap(), serde_json::to_string(&e.sounds_like).unwrap(), e.starred as i64, e.keyword as i64],
            )?;
            return Ok(id);
        }
        conn.execute(
            "INSERT INTO dictionary (word, replace_json, sounds_like_json, source, starred, keyword, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![e.word, serde_json::to_string(&e.replace).unwrap(), serde_json::to_string(&e.sounds_like).unwrap(), source_str(e.source), e.starred as i64, e.keyword as i64, now_ms()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Editing a built-in word makes it the user's own, so starter-list updates won't undo it.
    pub fn dict_update(&self, e: &DictEntry) -> DbResult<()> {
        self.c().execute(
            "UPDATE dictionary SET word=?2, replace_json=?3, sounds_like_json=?4, starred=?5, keyword=?6,
             source=CASE WHEN source='builtin' THEN 'user' ELSE source END WHERE id=?1",
            params![e.id, e.word, serde_json::to_string(&e.replace).unwrap(), serde_json::to_string(&e.sounds_like).unwrap(), e.starred as i64, e.keyword as i64],
        )?;
        Ok(())
    }

    pub fn dict_delete(&self, id: i64) -> DbResult<()> {
        let conn = self.c();
        let source: Option<String> = conn.query_row("SELECT source FROM dictionary WHERE id=?1", [id], |r| r.get(0)).optional()?;
        if source.as_deref() == Some("builtin") {
            conn.execute("UPDATE dictionary SET hidden=1 WHERE id=?1", [id])?;
        } else {
            conn.execute("DELETE FROM dictionary WHERE id=?1", [id])?;
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
        let mut stmt = conn.prepare("SELECT id, trigger, expansion, uses FROM snippets ORDER BY trigger COLLATE NOCASE")?;
        let rows = stmt.query_map([], |r| Ok(Snippet { id: r.get(0)?, trigger: r.get(1)?, expansion: r.get(2)?, uses: r.get(3)? }))?;
        rows.collect()
    }

    pub fn snippet_save(&self, s: &Snippet) -> DbResult<i64> {
        let conn = self.c();
        if s.id > 0 {
            conn.execute("UPDATE snippets SET trigger=?2, expansion=?3 WHERE id=?1", params![s.id, s.trigger, s.expansion])?;
            Ok(s.id)
        } else {
            conn.execute(
                "INSERT INTO snippets (trigger, expansion, created_at) VALUES (?1,?2,?3)
                 ON CONFLICT(trigger) DO UPDATE SET expansion=excluded.expansion",
                params![s.trigger, s.expansion, now_ms()],
            )?;
            conn.query_row("SELECT id FROM snippets WHERE trigger=?1", [&s.trigger], |r| r.get(0))
        }
    }

    pub fn snippet_delete(&self, id: i64) -> DbResult<()> {
        self.c().execute("DELETE FROM snippets WHERE id=?1", [id])?;
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
        let conn = self.c();
        let mut stmt = conn.prepare(
            "SELECT created_at, word_count, duration_ms, COALESCE(app_name,''), fixes FROM dictations WHERE status='done' AND kind='dictation'",
        )?;
        let rows: Vec<(i64, i64, i64, String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<DbResult<_>>()?;
        drop(stmt);
        drop(conn);
        Ok(compute_stats(&rows, typing_wpm, now))
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
}

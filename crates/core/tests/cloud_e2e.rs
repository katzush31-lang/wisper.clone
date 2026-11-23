//! End-to-end contract test: Wisper's Rust sync client against the REAL Cloudflare Worker
//! (`wrangler dev --local`, real D1), not a mock. See `cloud/scripts/e2e.sh`, which starts the
//! Worker, seeds sign-ins and runs this file.
//!
//! Skipped unless `WISPER_E2E_URL` is set, and `#[ignore]`d, so a plain `cargo test` never runs it.
//!
//! Environment (all set by the driver script):
//! - `WISPER_E2E_URL`    base URL of the local Worker
//! - `WISPER_E2E_TOKENS` `1a=<refresh token>,1b=<...>,2a=...` : slot number + computer (a|b)
//! - `WISPER_E2E_STATE`  the `--persist-to` directory of the local D1 (read directly to prove
//!                       what is, and is not, stored on the server)
//!
//! Every scenario uses its own slot(s), i.e. its own user, so scenarios cannot disturb each other.
//! Google sign-in can't run here, so each slot's user and two devices are seeded straight into
//! D1; a computer "signs in" by starting from its seeded refresh token (`resume_sign_in`).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use wisper_core::cloud::{CloudClient, CloudError, KeyStore, MemoryKeyStore, MemoryTokenStore, SyncEngine, SyncReport, TokenStore};
use wisper_core::db::{Db, HistoryItem};
use wisper_core::dictionary::DictEntry;
use wisper_core::settings::{KeyStorage, RemoteSection, Settings, Theme};
use wisper_core::snippets::Snippet;

// ---------------------------------------------------------------- environment

fn url() -> Option<String> {
    std::env::var("WISPER_E2E_URL").ok().filter(|u| !u.is_empty())
}

/// Every test starts with this: returns the Worker URL, or None (test passes trivially) when the
/// driver script isn't running.
macro_rules! e2e_url {
    () => {
        match url() {
            Some(u) => u,
            None => {
                eprintln!("WISPER_E2E_URL not set: skipping (run cloud/scripts/e2e.sh)");
                return;
            }
        }
    };
}

fn token(slot: u32, dev: char) -> String {
    let all = std::env::var("WISPER_E2E_TOKENS").expect("WISPER_E2E_TOKENS");
    let want = format!("{slot}{dev}");
    all.split(',')
        .find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == want).map(|(_, v)| v.to_string()))
        .unwrap_or_else(|| panic!("no seeded token for slot {want}"))
}

fn user_id(slot: u32) -> String {
    format!("user-{slot}")
}

fn device_id(slot: u32, dev: char) -> String {
    format!("dev-{slot}-{dev}")
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

async fn pause() {
    tokio::time::sleep(Duration::from_millis(25)).await;
}

/// The Worker allows 20 `/auth/*` calls per minute per IP. Wait if this run is about to exceed that.
async fn spend_auth_calls(n: usize) {
    static USED: OnceLock<Mutex<VecDeque<Instant>>> = OnceLock::new();
    const BUDGET: usize = 16;
    loop {
        let wait = {
            let mut q = USED.get_or_init(Default::default).lock().unwrap();
            while q.front().map(|t| t.elapsed() > Duration::from_secs(61)).unwrap_or(false) {
                q.pop_front();
            }
            if q.len() + n <= BUDGET {
                (0..n).for_each(|_| q.push_back(Instant::now()));
                None
            } else {
                Some(Duration::from_secs(61).saturating_sub(q.front().unwrap().elapsed()) + Duration::from_millis(200))
            }
        };
        match wait {
            None => return,
            Some(w) => {
                eprintln!("  (pacing: waiting {:.0}s for the Worker's sign-in rate limit)", w.as_secs_f32());
                tokio::time::sleep(w).await;
            }
        }
    }
}

fn sha256_b64url(s: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(s.as_bytes()))
}

// ---------------------------------------------------------------- reading the real D1

fn d1() -> Connection {
    let state = PathBuf::from(std::env::var("WISPER_E2E_STATE").expect("WISPER_E2E_STATE"));
    fn find(dir: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    find(&p, out);
                } else if p.extension().map(|x| x == "sqlite").unwrap_or(false) && p.file_name().unwrap() != "metadata.sqlite" {
                    out.push(p);
                }
            }
        }
    }
    let mut files = vec![];
    find(&state.join("v3").join("d1"), &mut files);
    assert_eq!(files.len(), 1, "expected exactly one local D1 database under {state:?}, found {files:?}");
    Connection::open_with_flags(&files[0], OpenFlags::SQLITE_OPEN_READ_ONLY).expect("open local D1")
}

fn q(sql: &str, params: &[&str]) -> Vec<Vec<Value>> {
    let c = d1();
    let mut st = c.prepare(sql).unwrap();
    let n = st.column_count();
    let rows = st
        .query_map(rusqlite::params_from_iter(params.iter()), |r| {
            (0..n)
                .map(|i| {
                    Ok(match r.get_ref(i)? {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(i) => json!(i),
                        rusqlite::types::ValueRef::Real(f) => json!(f),
                        rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                        rusqlite::types::ValueRef::Blob(b) => json!(format!("<{} bytes>", b.len())),
                    })
                })
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap();
    rows.collect::<rusqlite::Result<_>>().unwrap()
}

fn count(sql: &str, params: &[&str]) -> i64 {
    q(sql, params)[0][0].as_i64().unwrap()
}

/// Every table, every column, as one big string: for "this text appears nowhere on the server".
fn dump_all_d1() -> String {
    let c = d1();
    let tables: Vec<String> = c
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let mut out = String::new();
    for t in tables {
        out.push_str(&format!("[{t}]\n"));
        let rows = q(&format!("SELECT * FROM \"{t}\""), &[]);
        for r in rows {
            out.push_str(&serde_json::to_string(&r).unwrap());
            out.push('\n');
        }
    }
    // Plus the raw bytes of the database file, in case the text is somewhere a table scan misses.
    out
}

fn assert_nowhere_in_d1(needle: &str) {
    let dump = dump_all_d1();
    assert!(!dump.contains(needle), "found {needle:?} somewhere in D1");
    // The raw file too (including any not-yet-checkpointed write-ahead log).
    let state = PathBuf::from(std::env::var("WISPER_E2E_STATE").unwrap());
    let mut stack = vec![state];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(bytes) = std::fs::read(&p) {
                assert!(!bytes.windows(needle.len()).any(|w| w == needle.as_bytes()), "found {needle:?} in the file {p:?}");
            }
        }
    }
}

// ---------------------------------------------------------------- a computer

struct Computer {
    _dir: tempfile::TempDir,
    db: Arc<Db>,
    tokens: Arc<MemoryTokenStore>,
    keys: Arc<MemoryKeyStore>,
    engine: Arc<SyncEngine>,
    settings: Settings,
    slot: u32,
    dev: char,
}

impl Computer {
    /// A fresh computer (own data folder, database, key store) that starts from the refresh token
    /// seeded for `slot`/`dev`, then runs its first sync like the app does after sign-in.
    async fn start(base: &str, slot: u32, dev: char) -> Computer {
        let mut c = Computer::start_no_sync(base, slot, dev).await;
        c.sync().await;
        c
    }

    async fn start_no_sync(base: &str, slot: u32, dev: char) -> Computer {
        spend_auth_calls(1).await;
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(&dir.path().join("wisper.db")).unwrap());
        let tokens = Arc::new(MemoryTokenStore::new());
        tokens.save(&token(slot, dev)).unwrap();
        let keys = Arc::new(MemoryKeyStore::new());
        let client = CloudClient::new(base, tokens.clone());
        let engine = SyncEngine::new(client, db.clone(), keys.clone(), dir.path().to_path_buf());
        let info = engine.resume_sign_in().await.expect("resume sign-in against the real Worker");
        assert_eq!(info.id, user_id(slot));
        assert_eq!(info.device_id, device_id(slot, dev));
        Computer { _dir: dir, db, tokens, keys, engine, settings: Settings::default(), slot, dev }
    }

    /// Edits settings the way the app does: stamp the sections that changed.
    fn edit(&mut self, f: impl FnOnce(&mut Settings)) {
        let old = self.settings.clone();
        f(&mut self.settings);
        self.settings.stamp_changed_sections(&old, now_ms());
    }

    async fn sync(&mut self) -> SyncReport {
        let r = self.engine.sync(&self.settings).await.expect("sync against the real Worker");
        r.apply_to_settings(&mut self.settings);
        r
    }

    async fn choose(&mut self, storage: KeyStorage) {
        self.engine.set_key_storage(storage).await.expect("choose key storage");
        // The app mirrors the (per-computer, never synced) choice into settings for the UI.
        self.settings.account.key_storage = storage;
    }

    /// Like `set_api_key` in the app: save locally, then upload if keys live in the account.
    async fn set_key(&mut self, provider: &str, key: &str) {
        self.keys.set(provider, key);
        self.engine.upload_key(provider, key).await.expect("upload key");
    }

    fn words(&self) -> Vec<String> {
        let mut w: Vec<_> = self.db.dict_all().unwrap().into_iter().map(|e| e.word).collect();
        w.sort();
        w
    }

    fn entry(&self, word: &str) -> Option<DictEntry> {
        self.db.dict_all().unwrap().into_iter().find(|e| e.word == word)
    }

    fn client(&self) -> &Arc<CloudClient> {
        self.engine.client()
    }

    fn me(&self) -> (String, String) {
        (user_id(self.slot), device_id(self.slot, self.dev))
    }
}

// ---------------------------------------------------------------- raw HTTP (for what the Rust
// client deliberately can't do: other users' ids, forged tokens, replayed refresh tokens)

struct Raw {
    http: reqwest::Client,
    base: String,
    access: String,
    refresh: String,
}

async fn raw_refresh(base: &str, refresh: &str) -> (u16, Value) {
    spend_auth_calls(1).await;
    let r = reqwest::Client::new().post(format!("{base}/auth/refresh")).json(&json!({"refresh_token": refresh})).send().await.unwrap();
    let code = r.status().as_u16();
    (code, r.json().await.unwrap_or(Value::Null))
}

impl Raw {
    async fn login(base: &str, refresh: &str) -> Raw {
        let (code, body) = raw_refresh(base, refresh).await;
        assert_eq!(code, 200, "refresh failed: {body}");
        Raw {
            http: reqwest::Client::new(),
            base: base.to_string(),
            access: body["access_token"].as_str().unwrap().to_string(),
            refresh: body["refresh_token"].as_str().unwrap().to_string(),
        }
    }

    async fn req(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.req_with(method, path, body, &self.access).await
    }

    async fn req_with(&self, method: &str, path: &str, body: Option<Value>, access: &str) -> (u16, Value) {
        let mut rb = self.http.request(method.parse().unwrap(), format!("{}{}", self.base, path)).bearer_auth(access);
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let r = rb.send().await.unwrap();
        let code = r.status().as_u16();
        let text = r.text().await.unwrap_or_default();
        (code, serde_json::from_str(&text).unwrap_or(Value::Null))
    }
}

const KEY_ANTHROPIC: &str = "sk-ant-api03-Zk3mQ9vT7pLx2WnYhR5cB8dF1gJ4sUaE6oIqNtV0yXwCjMrHbKlPzDfSgAe-9QQ-wisper-e2e";
const KEY_OPENAI: &str = "sk-proj-Hq7TnV2xLmB9rWcYdE4fGaJ1kPsUoZ6iNbX3tR8yCvMwQhKeAlDgFjSzOp5uI0-wisper-e2e";

// ================================================================ scenarios

#[tokio::test]
#[ignore]
async fn s01_settings_section_change_reaches_the_other_computer() {
    let base = e2e_url!();
    let mut a = Computer::start(&base, 1, 'a').await;
    // Hebrew name, a theme, a nested value (sounds.volume) and a list, all in different sections.
    a.edit(|s| {
        s.user_name = "מנחם מענדל שניאורסאהן".into();
        s.theme = Theme::Dark;
        s.sounds.volume = 0.25;
        s.languages = vec!["en".into(), "he".into(), "yi".into()];
        s.microphone = Some("Mic only on A".into()); // never synced
    });
    a.sync().await;

    let mut b = Computer::start(&base, 1, 'b').await;
    assert_eq!(b.settings.user_name, "מנחם מענדל שניאורסאהן");
    assert_eq!(b.settings.theme, Theme::Dark);
    assert!((b.settings.sounds.volume - 0.25).abs() < 1e-6);
    assert_eq!(b.settings.languages, vec!["en", "he", "yi"]);
    assert_eq!(b.settings.microphone, None, "never-synced fields stay on their computer");

    // A later single-section change goes A -> B and leaves the other sections alone.
    pause().await;
    a.edit(|s| s.theme = Theme::Light);
    a.sync().await;
    b.sync().await;
    assert_eq!(b.settings.theme, Theme::Light);
    assert_eq!(b.settings.user_name, "מנחם מענדל שניאורסאהן");

    // The server really holds the sections (as JSON), tied to this user only.
    assert!(count("SELECT COUNT(*) FROM settings WHERE user_id = ? AND section = 'theme'", &[&user_id(1)]) == 1);
    assert_eq!(count("SELECT COUNT(*) FROM settings WHERE user_id = ? AND section = 'microphone'", &[&user_id(1)]), 0);
}

#[tokio::test]
#[ignore]
async fn s02_dictionary_add_edit_delete_reach_the_other_computer() {
    let base = e2e_url!();
    let mut a = Computer::start(&base, 2, 'a').await;
    let mut b = Computer::start(&base, 2, 'b').await;

    // Add: Ashkenazi spellings, Hebrew and Yiddish script, punctuation, replace + sounds-like lists.
    a.db.dict_add(&DictEntry { replace: vec!["shabbat".into(), "shabbes".into()], sounds_like: vec!["shah bus".into()], ..DictEntry::user("Shabbos") }).unwrap();
    a.db.dict_add(&DictEntry::user("זכות")).unwrap();
    a.db.dict_add(&DictEntry { starred: true, ..DictEntry::user("פֿאַרבריינגען") }).unwrap();
    a.db.dict_add(&DictEntry::user("be'H")).unwrap();
    a.sync().await;
    b.sync().await;
    assert_eq!(b.words(), vec!["Shabbos", "be'H", "זכות", "פֿאַרבריינגען"].into_iter().map(String::from).collect::<Vec<_>>().tap_sorted());
    let sh = b.entry("Shabbos").unwrap();
    assert_eq!(sh.replace, vec!["shabbat", "shabbes"]);
    assert_eq!(sh.sounds_like, vec!["shah bus"]);
    assert!(b.entry("פֿאַרבריינגען").unwrap().starred);

    // Edit.
    pause().await;
    let mut e = a.entry("Shabbos").unwrap();
    e.replace = vec!["shabbat".into()];
    e.starred = true;
    e.keyword = true;
    a.db.dict_update(&e).unwrap();
    a.sync().await;
    b.sync().await;
    let sh = b.entry("Shabbos").unwrap();
    assert_eq!(sh.replace, vec!["shabbat"]);
    assert!(sh.starred && sh.keyword);

    // Delete (soft delete travels as a tombstone).
    pause().await;
    let z = a.entry("זכות").unwrap();
    a.db.dict_delete(z.id).unwrap();
    a.sync().await;
    b.sync().await;
    assert!(b.entry("זכות").is_none());
    assert_eq!(b.words().len(), 3);
    assert_eq!(count("SELECT COUNT(*) FROM items WHERE user_id = ? AND kind = 'dictionary' AND deleted = 1", &[&user_id(2)]), 1);
    assert_eq!(count("SELECT COUNT(*) FROM items WHERE user_id = ? AND kind = 'dictionary' AND deleted = 0", &[&user_id(2)]), 3);

    // Re-adding the deleted word on B comes back to A.
    pause().await;
    b.db.dict_add(&DictEntry::user("זכות")).unwrap();
    b.sync().await;
    a.sync().await;
    assert!(a.entry("זכות").is_some());
}

/// Tiny helper so the expected list above reads naturally.
trait TapSorted {
    fn tap_sorted(self) -> Self;
}
impl TapSorted for Vec<String> {
    fn tap_sorted(mut self) -> Self {
        self.sort();
        self
    }
}

#[tokio::test]
#[ignore]
async fn s03_snippets_reach_the_other_computer() {
    let base = e2e_url!();
    let mut a = Computer::start(&base, 3, 'a').await;
    let mut b = Computer::start(&base, 3, 'b').await;

    // Multi-line Hebrew + English text near the 8,000 character app limit.
    let long: String = "ברוך אתה השם — Baruch atah Hashem. ".repeat(220); // about 7,700 chars
    assert!(long.chars().count() < 8000 && long.len() > 8000);
    a.db.snippet_save(&Snippet { id: 0, trigger: "my address".into(), expansion: "12 Main St\nBrooklyn, NY\n11213".into(), uses: 0 }).unwrap();
    a.db.snippet_save(&Snippet { id: 0, trigger: "ברכה".into(), expansion: long.clone(), uses: 0 }).unwrap();
    a.sync().await;
    b.sync().await;
    let got = b.db.snippets_all().unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got.iter().find(|s| s.trigger == "my address").unwrap().expansion, "12 Main St\nBrooklyn, NY\n11213");
    assert_eq!(got.iter().find(|s| s.trigger == "ברכה").unwrap().expansion, long, "long unicode text round-trips exactly");

    // Edit and delete.
    pause().await;
    let mut s = got.iter().find(|s| s.trigger == "my address").unwrap().clone();
    s.expansion = "99 Other Rd".into();
    a.db.snippet_save(&Snippet { id: a.db.snippets_all().unwrap().iter().find(|x| x.trigger == "my address").unwrap().id, ..s }).unwrap();
    let blessing = a.db.snippets_all().unwrap().into_iter().find(|x| x.trigger == "ברכה").unwrap();
    a.db.snippet_delete(blessing.id).unwrap();
    a.sync().await;
    b.sync().await;
    let got = b.db.snippets_all().unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].expansion, "99 Other Rd");
}

#[tokio::test]
#[ignore]
async fn s04_cloud_api_keys_sync_and_removal_reach_the_other_computer() {
    let base = e2e_url!();
    let mut a = Computer::start(&base, 4, 'a').await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_ANTHROPIC).await;
    a.set_key("openai", KEY_OPENAI).await;
    a.sync().await;

    let mut b = Computer::start(&base, 4, 'b').await;
    b.choose(KeyStorage::Cloud).await;
    assert_eq!(b.keys.get("anthropic").as_deref(), Some(KEY_ANTHROPIC));
    assert_eq!(b.keys.get("openai").as_deref(), Some(KEY_OPENAI));

    // The server stores them encrypted: ciphertext + iv + version, never the plain key.
    let rows = q("SELECT provider, ciphertext, iv, key_version FROM api_keys WHERE user_id = ? ORDER BY provider", &[&user_id(4)]);
    assert_eq!(rows.len(), 2);
    for (row, plain) in rows.iter().zip([KEY_ANTHROPIC, KEY_OPENAI]) {
        let ct = row[1].as_str().unwrap();
        assert!(!ct.is_empty() && ct != plain && !ct.contains(plain) && !plain.contains(ct), "ciphertext must differ from the key");
        assert!(!row[2].as_str().unwrap().is_empty(), "an IV is stored");
        assert_eq!(row[3], json!(1));
    }
    assert_ne!(rows[0][1], rows[1][1]);
    assert_nowhere_in_d1(KEY_ANTHROPIC);
    assert_nowhere_in_d1(KEY_OPENAI);

    // A changes a key: B gets the new one.
    pause().await;
    let rotated = "sk-ant-api03-ROTATED-key-0123456789";
    a.set_key("anthropic", rotated).await;
    b.sync().await;
    assert_eq!(b.keys.get("anthropic").as_deref(), Some(rotated));

    // Removal on A reaches B (and the account).
    a.set_key("openai", "").await;
    assert!(a.keys.get("openai").is_none());
    b.sync().await;
    assert!(b.keys.get("openai").is_none(), "removed on A, removed on B");
    assert_eq!(b.keys.get("anthropic").as_deref(), Some(rotated));
    let remote = a.client().get_keys().await.unwrap();
    assert!(remote.openai.is_none());
    assert_eq!(count("SELECT COUNT(*) FROM api_keys WHERE user_id = ? AND provider = 'openai'", &[&user_id(4)]), 0);
    assert_nowhere_in_d1(rotated);
}

#[tokio::test]
#[ignore]
async fn s05_local_key_storage_never_uploads() {
    let base = e2e_url!();
    let mut a = Computer::start_no_sync(&base, 5, 'a').await;
    a.keys.set("anthropic", KEY_ANTHROPIC); // a key that already existed before signing in
    a.sync().await;
    // Before choosing, nothing is uploaded.
    assert_eq!(a.client().get_keys().await.unwrap().anthropic, None);

    a.choose(KeyStorage::Local).await;
    a.set_key("openai", KEY_OPENAI).await;
    a.sync().await;
    a.sync().await;

    let remote = a.client().get_keys().await.unwrap(); // GET /v1/keys
    assert!(remote.openai.is_none() && remote.anthropic.is_none(), "GET /v1/keys must be empty");
    assert_eq!(count("SELECT COUNT(*) FROM api_keys WHERE user_id = ?", &[&user_id(5)]), 0, "no api_keys row in D1");
    assert_nowhere_in_d1(KEY_ANTHROPIC);
    assert_nowhere_in_d1(KEY_OPENAI);
    assert_eq!(a.keys.get("openai").as_deref(), Some(KEY_OPENAI), "local copy kept");

    // A second computer on Local never receives keys either, and its own keys never leave it.
    let mut b = Computer::start(&base, 5, 'b').await;
    b.choose(KeyStorage::Local).await;
    b.set_key("anthropic", "sk-ant-only-on-b-1234567890").await;
    b.sync().await;
    assert!(b.keys.get("openai").is_none());
    assert_eq!(count("SELECT COUNT(*) FROM api_keys WHERE user_id = ?", &[&user_id(5)]), 0);
    assert_nowhere_in_d1("sk-ant-only-on-b-1234567890");

    // Switching to Cloud later uploads (encrypted); switching back removes them from the account
    // but keeps this computer's copies.
    a.choose(KeyStorage::Cloud).await;
    assert_eq!(count("SELECT COUNT(*) FROM api_keys WHERE user_id = ?", &[&user_id(5)]), 2);
    let ct = q("SELECT ciphertext FROM api_keys WHERE user_id = ? AND provider = 'openai'", &[&user_id(5)])[0][0].as_str().unwrap().to_string();
    assert_ne!(ct, KEY_OPENAI);
    assert_nowhere_in_d1(KEY_OPENAI);
    a.choose(KeyStorage::Local).await;
    assert_eq!(count("SELECT COUNT(*) FROM api_keys WHERE user_id = ?", &[&user_id(5)]), 0);
    assert_eq!(a.keys.get("openai").as_deref(), Some(KEY_OPENAI));
}

#[tokio::test]
#[ignore]
async fn s06_stats_combine_across_computers_and_no_dictated_text_reaches_d1() {
    let base = e2e_url!();
    // Marker phrases that must never appear anywhere on the server, in any table or file.
    let marker_a = "ZQXMARKER-alpha-7f3a9c קדיש פֿאַרבריינגען";
    let marker_b = "ZQXMARKER-beta-51d2e8 Shabbos zechus";
    let item = |id: &str, words: i64, text: &str, app: &str| HistoryItem {
        id: id.into(),
        created_at: now_ms(),
        kind: "dictation".into(),
        status: "done".into(),
        word_count: words,
        duration_ms: 3000,
        app_name: Some(app.into()),
        raw_text: Some(format!("raw {text}")),
        clean_text: Some(format!("clean {text}")),
        final_text: Some(text.into()),
        selected_text: Some(format!("selected {text}")),
        ..Default::default()
    };
    let mut a = Computer::start_no_sync(&base, 6, 'a').await;
    let mut b = Computer::start_no_sync(&base, 6, 'b').await;
    a.db.insert_dictation(&item("e2e-6-a1", 10, marker_a, "Gmail")).unwrap();
    a.db.insert_dictation(&item("e2e-6-a2", 7, marker_a, "Word")).unwrap();
    b.db.insert_dictation(&item("e2e-6-b1", 5, marker_b, "Gmail")).unwrap();
    a.sync().await;
    b.sync().await;
    a.sync().await;

    let now = chrono::Local::now();
    assert_eq!(b.db.stats_with(40, now, true).unwrap().total_words, 22, "B sees A's 17 words + its own 5");
    assert_eq!(a.db.stats_with(40, now, true).unwrap().total_words, 22);
    assert_eq!(b.db.stats_with(40, now, true).unwrap().scope, "all-computers");
    assert_eq!(b.db.stats(40, now).unwrap().total_words, 5, "this-computer scope is unchanged");
    // Syncing again must not double count.
    b.sync().await;
    a.sync().await;
    assert_eq!(b.db.stats_with(40, now, true).unwrap().total_words, 22);

    // On the server: three numbers-only rows, attributed to the right computers.
    assert_eq!(count("SELECT COUNT(*) FROM stats WHERE user_id = ?", &[&user_id(6)]), 3);
    assert_eq!(count("SELECT COUNT(*) FROM stats WHERE user_id = ? AND device_id = ?", &[&user_id(6), &device_id(6, 'a')]), 2);
    assert_eq!(count("SELECT COALESCE(SUM(words),0) FROM stats WHERE user_id = ?", &[&user_id(6)]), 22);
    // No dictated text anywhere: not the markers, not even the prefixes the app stores locally.
    assert_nowhere_in_d1("ZQXMARKER");
    assert_nowhere_in_d1(marker_a);
    assert_nowhere_in_d1(marker_b);
    assert_nowhere_in_d1("קדיש");
}

#[tokio::test]
#[ignore]
async fn s07_newest_change_wins() {
    let base = e2e_url!();
    let mut a = Computer::start(&base, 7, 'a').await;
    let mut b = Computer::start(&base, 7, 'b').await;

    a.edit(|s| s.user_name = "From A".into());
    a.db.dict_add(&DictEntry { replace: vec!["a-version".into()], ..DictEntry::user("Chaya") }).unwrap();
    a.db.snippet_save(&Snippet { id: 0, trigger: "sig".into(), expansion: "A text".into(), uses: 0 }).unwrap();
    a.sync().await;
    b.sync().await;
    pause().await;

    // B changes the same things later; B's changes are newer, so they win everywhere.
    b.edit(|s| s.user_name = "From B".into());
    let mut e = b.entry("Chaya").unwrap();
    e.replace = vec!["b-version".into()];
    b.db.dict_update(&e).unwrap();
    let sid = b.db.snippets_all().unwrap()[0].id;
    b.db.snippet_save(&Snippet { id: sid, trigger: "sig".into(), expansion: "B text".into(), uses: 0 }).unwrap();
    b.sync().await;
    a.sync().await;
    b.sync().await;
    assert_eq!(a.settings.user_name, "From B");
    assert_eq!(b.settings.user_name, "From B");
    assert_eq!(a.entry("Chaya").unwrap().replace, vec!["b-version"]);
    assert_eq!(a.db.snippets_all().unwrap()[0].expansion, "B text");

    // A stale write straight at the API (older timestamps) is ignored by the server.
    let stale_section = RemoteSection { value: json!("STALE"), updated_at: 1000 };
    a.client().put_settings(&[("userName".to_string(), stale_section)].into_iter().collect()).await.unwrap();
    let on_server = a.client().get_settings().await.unwrap();
    assert_eq!(on_server["userName"].value, json!("From B"), "the server keeps the newer value");
    let mut item = a.client().get_items(0).await.unwrap().items.into_iter().find(|i| i.kind == "snippet").unwrap();
    item.data = json!({"trigger": "sig", "expansion": "STALE"});
    item.updated_at = 1000;
    a.client().post_items(&[item]).await.unwrap();
    a.sync().await;
    assert_eq!(a.db.snippets_all().unwrap()[0].expansion, "B text");
    b.sync().await;
    assert_eq!(b.db.snippets_all().unwrap()[0].expansion, "B text");

    // And the other way round: when A edits after B, A's value wins.
    pause().await;
    a.edit(|s| s.user_name = "From A again".into());
    a.sync().await;
    b.sync().await;
    assert_eq!(b.settings.user_name, "From A again");
}

#[tokio::test]
#[ignore]
async fn s08_refresh_tokens_rotate_and_replay_revokes_the_device() {
    let base = e2e_url!();
    let slot = 8;

    // Through the real client: a refresh rotates the stored token, and D1 only keeps hashes.
    let seeded = token(slot, 'b');
    let mut b = Computer::start(&base, slot, 'b').await;
    let rotated = b.tokens.load().unwrap();
    assert_ne!(rotated, seeded, "the refresh token changes on every refresh");
    let row = &q("SELECT refresh_hash, prev_refresh_hash FROM devices WHERE id = ?", &[&device_id(slot, 'b')])[0];
    assert_eq!(row[0], json!(sha256_b64url(&rotated)));
    assert_eq!(row[1], json!(sha256_b64url(&seeded)));
    assert_nowhere_in_d1(&rotated);
    assert_nowhere_in_d1(&seeded);
    b.sync().await; // still fine

    // Raw: use A's token once, then replay the old one.
    let t0 = token(slot, 'a');
    let (code, body) = raw_refresh(&base, &t0).await;
    assert_eq!(code, 200);
    let (t1, access1) = (body["refresh_token"].as_str().unwrap().to_string(), body["access_token"].as_str().unwrap().to_string());
    assert_ne!(t1, t0);
    // The new access token works.
    let http = reqwest::Client::new();
    let me = http.get(format!("{base}/v1/me")).bearer_auth(&access1).send().await.unwrap();
    assert_eq!(me.status().as_u16(), 200);

    // Replaying the already-used token is rejected as `revoked` ...
    let (code, body) = raw_refresh(&base, &t0).await;
    assert_eq!((code, body["error"].as_str()), (401, Some("revoked")));
    // ... and it revoked the whole device: the newest token and the access token die too.
    let (code, body) = raw_refresh(&base, &t1).await;
    assert_eq!((code, body["error"].as_str()), (401, Some("revoked")));
    let me = http.get(format!("{base}/v1/me")).bearer_auth(&access1).send().await.unwrap();
    assert_eq!(me.status().as_u16(), 401);
    assert_eq!(count("SELECT revoked FROM devices WHERE id = ?", &[&device_id(slot, 'a')]), 1);
    // The user's other computer is untouched.
    assert_eq!(count("SELECT revoked FROM devices WHERE id = ?", &[&device_id(slot, 'b')]), 0);
    b.sync().await;

    // Garbage tokens are just "invalid", not "revoked", and revoke nobody.
    let (code, body) = raw_refresh(&base, "not-a-real-refresh-token").await;
    assert_eq!((code, body["error"].as_str()), (401, Some("invalid")));

    // Through the real client: a stolen copy of B's pre-rotation token is a signed-out computer,
    // and tells the app to forget the sign-in; then the genuine B is revoked as well.
    let thief_store = Arc::new(MemoryTokenStore::new());
    thief_store.save(&rotated).unwrap(); // B's current token: the thief presents it first and wins
    let thief = CloudClient::new(&base, thief_store.clone());
    spend_auth_calls(1).await;
    thief.me().await.expect("the thief presents the current token first");
    assert_ne!(thief_store.load().unwrap(), rotated);
    // Now the genuine B presents the token it still holds, which has been rotated away.
    let stale_store = Arc::new(MemoryTokenStore::new());
    stale_store.save(&rotated).unwrap();
    let stale = CloudClient::new(&base, stale_store.clone());
    spend_auth_calls(1).await;
    assert!(matches!(stale.get_keys().await.unwrap_err(), CloudError::Revoked | CloudError::RevokedKeepKeys));
    assert!(stale_store.load().is_none(), "a revoked sign-in is forgotten on that computer");
    assert_eq!(count("SELECT revoked FROM devices WHERE id = ?", &[&device_id(slot, 'b')]), 1);
    assert!(matches!(thief.get_keys().await.unwrap_err(), CloudError::Revoked | CloudError::RevokedKeepKeys), "the thief's token is dead too");
}

#[tokio::test]
#[ignore]
async fn s09_another_user_cannot_see_or_touch_user_as_data() {
    let base = e2e_url!();
    // User A (slot 9): data in every area.
    let mut a = Computer::start(&base, 9, 'a').await;
    a.choose(KeyStorage::Cloud).await;
    a.edit(|s| s.user_name = "Private A".into());
    a.db.dict_add(&DictEntry::user("PrivateWordA")).unwrap();
    a.db.snippet_save(&Snippet { id: 0, trigger: "private trigger".into(), expansion: "private expansion".into(), uses: 0 }).unwrap();
    a.set_key("anthropic", KEY_ANTHROPIC).await;
    a.db
        .insert_dictation(&HistoryItem { id: "e2e-9-a1".into(), created_at: now_ms(), kind: "dictation".into(), status: "done".into(), word_count: 42, duration_ms: 1000, ..Default::default() })
        .unwrap();
    a.sync().await;
    let (uid_a, did_a) = a.me();
    let items_a = serde_json::to_value(a.client().get_items(0).await.unwrap().items).unwrap().as_array().unwrap().clone();
    assert_eq!(items_a.len(), 2);

    // User C (slot 10): a different person, signed in and valid.
    let c = Raw::login(&base, &token(10, 'a')).await;
    let (code, body) = c.req("GET", "/v1/me", None).await;
    assert_eq!(code, 200);
    assert_eq!(body["user"]["id"], json!(user_id(10)));

    // Nothing of A's is visible, even when C tries to name A (query, header, body).
    for spoof in ["", &format!("&user_id={uid_a}"), &format!("&user={uid_a}")] {
        let (_, v) = c.req("GET", &format!("/v1/items?since=0{spoof}"), None).await;
        assert_eq!(v["items"], json!([]), "items leaked with {spoof:?}");
        let (_, v) = c.req("GET", &format!("/v1/stats?since=0{spoof}"), None).await;
        assert_eq!(v["rows"], json!([]), "stats leaked with {spoof:?}");
        let (_, v) = c.req("GET", &format!("/v1/settings?x=1{spoof}"), None).await;
        assert_eq!(v["sections"], json!({}), "settings leaked with {spoof:?}");
    }
    let (_, v) = c.req("GET", "/v1/keys", None).await;
    assert_eq!(v, json!({"openai": null, "anthropic": null}));
    let (_, v) = c.req("GET", "/v1/devices", None).await;
    let ids: Vec<&str> = v.as_array().unwrap().iter().map(|d| d["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 2, "C sees only its own two computers");
    assert!(ids.iter().all(|i| i.starts_with("dev-10-")), "{ids:?}");

    // C cannot modify or revoke A's things, including by guessing ids.
    let (code, _) = c.req("DELETE", &format!("/v1/devices/{did_a}"), None).await;
    assert!(code == 204 || code == 404, "got {code}");
    assert_eq!(count("SELECT revoked FROM devices WHERE id = ?", &[&did_a]), 0, "A's device must not be revoked by C");
    // C writes the very same keys/ids: lands in C's own space only.
    let uid_item = items_a[0]["uid"].clone();
    let (code, _) = c
        .req("POST", "/v1/items", Some(json!({"items": [{"kind": items_a[0]["kind"], "uid": uid_item, "data": {"word": "HIJACK", "trigger": "HIJACK", "expansion": "HIJACK"}, "updated_at": now_ms() + 5000, "deleted": 0}]})))
        .await;
    assert_eq!(code, 200);
    let (code, _) = c.req("PUT", "/v1/settings", Some(json!({"sections": {"userName": {"value": "HIJACK", "updated_at": now_ms() + 5000}}}))).await;
    assert_eq!(code, 200);
    let (code, _) = c.req("PUT", "/v1/keys/anthropic", Some(json!({"key": "sk-ant-HIJACK-0000000000"}))).await;
    assert_eq!(code, 200);
    let (code, _) = c.req("POST", "/v1/stats", Some(json!({"rows": [{"uid": "e2e-9-a1", "created_at": now_ms(), "words": 999999, "duration_ms": 1, "app": "x", "fixes": 0}]}))).await;
    assert_eq!(code, 200);
    // Wiping C's keys and deleting C's whole account must not touch A.
    assert_eq!(c.req("DELETE", "/v1/keys", None).await.0, 204);
    assert_eq!(c.req("DELETE", "/v1/account", None).await.0, 204);

    // A forged token (C's token with the user id swapped to A's) is rejected.
    let parts: Vec<&str> = c.access.split('.').collect();
    let mut claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
    claims["sub"] = json!(uid_a);
    let forged = format!("{}.{}.{}", parts[0], URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap()), parts[2]);
    assert_eq!(c.req_with("GET", "/v1/keys", None, &forged).await.0, 401);
    // No / malformed credentials.
    assert_eq!(c.req_with("GET", "/v1/keys", None, "").await.0, 401);
    assert_eq!(reqwest::get(format!("{base}/v1/keys")).await.unwrap().status().as_u16(), 401);

    // A's data is exactly as it was, still readable by A, and a brand-new computer B gets it all.
    let on_server = a.client().get_settings().await.unwrap();
    assert_eq!(on_server["userName"].value, json!("Private A"));
    let mut b = Computer::start(&base, 9, 'b').await;
    assert_eq!(b.settings.user_name, "Private A");
    assert_eq!(b.words(), vec!["PrivateWordA"]);
    assert_eq!(b.db.snippets_all().unwrap()[0].expansion, "private expansion");
    b.choose(KeyStorage::Cloud).await;
    assert_eq!(b.keys.get("anthropic").as_deref(), Some(KEY_ANTHROPIC), "A's key was not replaced by C's");
    assert_eq!(b.db.stats_with(40, chrono::Local::now(), true).unwrap().total_words, 42, "C's fake stats row didn't land in A's account");
    assert_eq!(count("SELECT COUNT(*) FROM items WHERE user_id = ?", &[&uid_a]), 2);
    assert_eq!(count("SELECT COUNT(*) FROM api_keys WHERE user_id = ?", &[&uid_a]), 1);
    // C's writes only ever landed in C's own rows (C later deleted its account), never A's.
    for t in ["items", "settings", "stats"] {
        let rows = q(&format!("SELECT * FROM {t} WHERE user_id = ?"), &[&uid_a]);
        assert!(!serde_json::to_string(&rows).unwrap().contains("HIJACK"), "{t}");
        assert!(!serde_json::to_string(&rows).unwrap().contains("999999"), "{t}");
    }
    assert_eq!(count("SELECT COUNT(*) FROM users WHERE id = ?", &[&user_id(10)]), 0, "C's own account is gone");
    assert_eq!(count("SELECT COUNT(*) FROM users WHERE id = ?", &[&uid_a]), 1, "A's account is untouched");
    assert_eq!(count("SELECT COUNT(*) FROM settings WHERE user_id = ?", &[&user_id(10)]), 0);
}

#[tokio::test]
#[ignore]
async fn s10_sign_out_revokes_the_device() {
    let base = e2e_url!();
    let mut a = Computer::start(&base, 11, 'a').await;
    let mut b = Computer::start(&base, 11, 'b').await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_ANTHROPIC).await;
    a.db.dict_add(&DictEntry::user("Menachem")).unwrap();
    a.sync().await;
    let last_refresh = a.tokens.load().unwrap();
    spend_auth_calls(1).await; // logout is under /auth/*
    a.engine.sign_out().await;

    // Locally: signed out, cached cloud keys gone, dictionary stays.
    assert!(a.engine.account().is_none());
    assert!(a.tokens.load().is_none());
    assert!(a.keys.get("anthropic").is_none());
    assert_eq!(a.words(), vec!["Menachem"]);
    // On the server: the device is revoked; its refresh token no longer works.
    assert_eq!(count("SELECT revoked FROM devices WHERE id = ?", &[&device_id(11, 'a')]), 1);
    let (code, body) = raw_refresh(&base, &last_refresh).await;
    assert_eq!((code, body["error"].as_str()), (401, Some("revoked")));
    // The other computer is unaffected, no longer lists A, and keeps working.
    assert_eq!(count("SELECT revoked FROM devices WHERE id = ?", &[&device_id(11, 'b')]), 0);
    let devs = b.client().devices().await.unwrap();
    assert_eq!(devs.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), vec![device_id(11, 'b')]);
    b.sync().await;

    // Removing a computer from another computer revokes it too; it finds out on its next sync.
    let mut c = Computer::start(&base, 12, 'a').await;
    let d = Computer::start(&base, 12, 'b').await;
    c.client().remove_device(&device_id(12, 'b')).await.unwrap();
    assert_eq!(count("SELECT revoked FROM devices WHERE id = ?", &[&device_id(12, 'b')]), 1);
    let err = d.engine.client().get_keys().await.unwrap_err();
    assert_eq!(err, CloudError::Revoked);
    c.sync().await;

    // Deleting the account removes every row for that user and signs out.
    c.choose(KeyStorage::Cloud).await;
    c.set_key("openai", KEY_OPENAI).await;
    c.db.dict_add(&DictEntry::user("Gone")).unwrap();
    c.sync().await;
    c.engine.delete_account().await.unwrap();
    for t in ["settings", "items", "stats", "api_keys"] {
        assert_eq!(count(&format!("SELECT COUNT(*) FROM {t} WHERE user_id = ?"), &[&user_id(12)]), 0, "{t} rows remain after account deletion");
    }
    assert!(c.engine.account().is_none());
    assert_nowhere_in_d1(KEY_OPENAI);
}

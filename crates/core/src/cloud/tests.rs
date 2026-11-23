//! End-to-end tests of the client and sync engine against a fake account server (wiremock),
//! with two simulated computers (separate data folders, databases and key stores).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::*;
use crate::db::{Db, HistoryItem};
use crate::dictionary::DictEntry;
use crate::settings::{KeyStorage, Settings, Theme};
use crate::snippets::Snippet;

// ---------------------------------------------------------------- fake server

#[derive(Default)]
struct Dev {
    user: String,
    refresh: String,
    prev_refresh: Option<String>,
    revoked: bool,
}

#[derive(Default)]
struct St {
    n: u64,
    devices: HashMap<String, Dev>,
    access: HashMap<String, String>,
    keys: HashMap<(String, String), (String, i64)>,
    settings: HashMap<(String, String), (Value, i64)>,
    items: HashMap<(String, String, String), (Value, i64, bool, i64)>,
    stats: Vec<(String, String, Value, i64)>, // user, device, row, server_at
    log: Vec<String>,
    /// Reason sent with `revoked` replies (None = like an older server that sends none).
    revoke_reason: Option<String>,
    keys_removed_at: HashMap<String, i64>,
    settings_delay_ms: u64,
}

struct Fake {
    server: MockServer,
    st: Arc<Mutex<St>>,
}

fn json_resp(code: u16, v: Value) -> ResponseTemplate {
    ResponseTemplate::new(code).set_body_json(v)
}

fn err(code: u16, e: &str) -> ResponseTemplate {
    json_resp(code, json!({"error": e, "message": format!("fake {e}")}))
}

fn revoked(st: &St) -> ResponseTemplate {
    let mut body = json!({"error": "revoked", "message": "fake revoked"});
    if let Some(r) = &st.revoke_reason {
        body["reason"] = json!(r);
    }
    json_resp(401, body)
}

fn clock() -> i64 {
    now_ms()
}

fn handle(st: &Mutex<St>, req: &Request) -> ResponseTemplate {
    let mut st = st.lock().unwrap();
    let method = req.method.as_str().to_string();
    let path = req.url.path().to_string();
    st.log.push(format!("{method} {path}"));
    let q: HashMap<String, String> = req.url.query_pairs().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);

    fn auth_resp(st: &mut St, device: &str, user: &str) -> Value {
        st.n += 1;
        let (access, refresh) = (format!("acc{}", st.n), format!("ref{}", st.n));
        st.access.insert(access.clone(), device.to_string());
        let d = st.devices.get_mut(device).unwrap();
        d.prev_refresh = Some(std::mem::replace(&mut d.refresh, refresh.clone()));
        json!({"access_token": access, "refresh_token": refresh, "expires_in": 900,
               "user": {"id": user, "email": format!("{user}@example.com"), "name": user}, "device_id": device})
    }

    match (method.as_str(), path.as_str()) {
        ("POST", "/auth/token") => {
            let code = body["code"].as_str().unwrap_or("");
            let Some(user) = code.strip_prefix("google:") else { return err(400, "invalid_code") };
            st.n += 1;
            let dev = format!("dev{}", st.n);
            st.devices.insert(dev.clone(), Dev { user: user.to_string(), ..Default::default() });
            return json_resp(200, auth_resp(&mut st, &dev, user));
        }
        ("POST", "/auth/refresh") => {
            let tok = body["refresh_token"].as_str().unwrap_or("").to_string();
            let found = st.devices.iter().find(|(_, d)| d.refresh == tok || d.prev_refresh.as_deref() == Some(tok.as_str())).map(|(k, _)| k.clone());
            let Some(id) = found else { return err(401, "invalid") };
            let d = st.devices.get_mut(&id).unwrap();
            if d.revoked || d.prev_refresh.as_deref() == Some(tok.as_str()) {
                d.revoked = true;
                return revoked(&st);
            }
            let user = d.user.clone();
            return json_resp(200, auth_resp(&mut st, &id, &user));
        }
        _ => {}
    }

    // Everything else needs a valid access token for a live device.
    let bearer = req.headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("").to_string();
    let Some(dev) = st.access.get(&bearer).cloned() else { return err(401, "invalid") };
    if st.devices[&dev].revoked {
        return revoked(&st);
    }
    let user = st.devices[&dev].user.clone();
    let now = clock();
    let since: i64 = q.get("since").and_then(|s| s.parse().ok()).unwrap_or(0);

    match (method.as_str(), path.as_str()) {
        ("POST", "/auth/logout") => {
            st.devices.get_mut(&dev).unwrap().revoked = true;
            ResponseTemplate::new(204)
        }
        ("GET", "/v1/me") => json_resp(200, json!({"user": {"id": user, "email": format!("{user}@example.com"), "name": user}, "device_id": dev})),
        ("GET", "/v1/devices") => {
            let list: Vec<Value> = st.devices.iter().filter(|(_, d)| d.user == user && !d.revoked).map(|(id, _)| json!({"id": id, "name": "PC", "created_at": 1, "last_seen": 2, "current": *id == dev})).collect();
            json_resp(200, json!(list))
        }
        ("GET", "/v1/keys") => {
            let get = |p: &str| st.keys.get(&(user.clone(), p.to_string())).map(|(k, t)| json!({"key": k, "updated_at": t})).unwrap_or(Value::Null);
            let removed = st.keys_removed_at.get(&user).map(|t| json!(t)).unwrap_or(Value::Null);
            json_resp(200, json!({"openai": get("openai"), "anthropic": get("anthropic"), "removed_at": removed}))
        }
        ("PUT", p) if p.starts_with("/v1/keys/") => {
            let prov = p.trim_start_matches("/v1/keys/").to_string();
            let key = body["key"].as_str().unwrap_or("").to_string();
            if key.is_empty() {
                st.keys.remove(&(user, prov));
            } else {
                st.keys_removed_at.remove(&user);
                st.keys.insert((user, prov), (key, now));
            }
            json_resp(200, json!({"updated_at": now}))
        }
        ("DELETE", "/v1/keys") => {
            st.keys.retain(|(u, _), _| *u != user);
            st.keys_removed_at.insert(user.clone(), now);
            ResponseTemplate::new(204)
        }
        ("GET", "/v1/settings") => {
            let delay = std::time::Duration::from_millis(st.settings_delay_ms);
            let sections: serde_json::Map<String, Value> = st.settings.iter().filter(|((u, _), _)| *u == user).map(|((_, s), (v, t))| (s.clone(), json!({"value": v, "updated_at": t}))).collect();
            json_resp(200, json!({"sections": sections})).set_delay(delay)
        }
        ("PUT", "/v1/settings") => {
            for (name, sec) in body["sections"].as_object().cloned().unwrap_or_default() {
                let t = sec["updated_at"].as_i64().unwrap_or(0);
                let k = (user.clone(), name);
                if st.settings.get(&k).map(|(_, old)| t > *old).unwrap_or(true) {
                    st.settings.insert(k, (sec["value"].clone(), t));
                }
            }
            let sections: serde_json::Map<String, Value> = st.settings.iter().filter(|((u, _), _)| *u == user).map(|((_, s), (v, t))| (s.clone(), json!({"value": v, "updated_at": t}))).collect();
            json_resp(200, json!({"sections": sections}))
        }
        ("GET", "/v1/items") => {
            let items: Vec<Value> = st
                .items
                .iter()
                .filter(|((u, _, _), (_, _, _, at))| *u == user && *at >= since)
                .map(|((_, kind, uid), (data, t, del, at))| json!({"kind": kind, "uid": uid, "data": data, "updated_at": t, "deleted": *del as i64, "server_at": at}))
                .collect();
            json_resp(200, json!({"items": items, "now": now}))
        }
        ("POST", "/v1/items") => {
            let items = body["items"].as_array().cloned().unwrap_or_default();
            for it in &items {
                let k = (user.clone(), it["kind"].as_str().unwrap().to_string(), it["uid"].as_str().unwrap().to_string());
                let t = it["updated_at"].as_i64().unwrap();
                if st.items.get(&k).map(|(_, old, _, _)| t > *old).unwrap_or(true) {
                    let del = it["deleted"].as_i64().map(|d| d != 0).or(it["deleted"].as_bool()).unwrap_or(false);
                    st.items.insert(k, (it["data"].clone(), t, del, now));
                }
            }
            json_resp(200, json!({"received": items.len(), "now": now}))
        }
        ("GET", "/v1/stats") => {
            let rows: Vec<Value> = st.stats.iter().filter(|(u, _, _, at)| *u == user && *at >= since).map(|(_, d, r, at)| {
                let mut r = r.clone();
                r["device_id"] = json!(d);
                r["server_at"] = json!(at);
                r
            }).collect();
            json_resp(200, json!({"rows": rows, "now": now}))
        }
        ("POST", "/v1/stats") => {
            let mut inserted = 0;
            for r in body["rows"].as_array().cloned().unwrap_or_default() {
                if !st.stats.iter().any(|(u, _, x, _)| *u == user && x["uid"] == r["uid"]) {
                    st.stats.push((user.clone(), dev.clone(), r, now));
                    inserted += 1;
                }
            }
            json_resp(200, json!({"received": 0, "inserted": inserted}))
        }
        ("DELETE", p) if p.starts_with("/v1/devices/") => {
            let id = p.trim_start_matches("/v1/devices/");
            if st.devices.get(id).map(|d| d.user == user).unwrap_or(false) {
                st.devices.get_mut(id).unwrap().revoked = true;
            }
            ResponseTemplate::new(204)
        }
        ("DELETE", "/v1/account") => {
            st.keys.retain(|(u, _), _| *u != user);
            st.settings.retain(|(u, _), _| *u != user);
            st.items.retain(|(u, _, _), _| *u != user);
            st.stats.retain(|(u, _, _, _)| *u != user);
            for d in st.devices.values_mut().filter(|d| d.user == user) {
                d.revoked = true;
            }
            ResponseTemplate::new(204)
        }
        _ => err(404, "not_found"),
    }
}

impl Fake {
    async fn start() -> Fake {
        let server = MockServer::start().await;
        let st = Arc::new(Mutex::new(St::default()));
        let st2 = st.clone();
        Mock::given(any()).respond_with(move |req: &Request| handle(&st2, req)).mount(&server).await;
        Fake { server, st }
    }
    fn url(&self) -> String {
        self.server.uri()
    }
    fn calls(&self, what: &str) -> usize {
        self.st.lock().unwrap().log.iter().filter(|l| l.as_str() == what).count()
    }
    fn expire_access_tokens(&self) {
        self.st.lock().unwrap().access.clear();
    }
    fn revoke_all_devices(&self, reason: Option<&str>) {
        self.st.lock().unwrap().revoke_reason = reason.map(String::from);
        for d in self.st.lock().unwrap().devices.values_mut() {
            d.revoked = true;
        }
    }
    fn server_keys(&self) -> HashMap<String, String> {
        self.st.lock().unwrap().keys.iter().map(|((_, p), (k, _))| (p.clone(), k.clone())).collect()
    }
}

// ---------------------------------------------------------------- simulated computer

struct Computer {
    dir: tempfile::TempDir,
    db: Arc<Db>,
    tokens: Arc<MemoryTokenStore>,
    keys: Arc<MemoryKeyStore>,
    engine: Arc<SyncEngine>,
    settings: Settings,
}

async fn pause() {
    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
}

impl Computer {
    fn new(url: &str) -> Computer {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(&dir.path().join("wisper.db")).unwrap());
        let tokens = Arc::new(MemoryTokenStore::new());
        let keys = Arc::new(MemoryKeyStore::new());
        let client = CloudClient::new(url, tokens.clone());
        let engine = SyncEngine::new(client, db.clone(), keys.clone(), dir.path().to_path_buf());
        Computer { dir, db, tokens, keys, engine, settings: Settings::default() }
    }

    /// Edits settings the way the app does: stamp the sections that changed.
    fn edit(&mut self, f: impl FnOnce(&mut Settings)) {
        let old = self.settings.clone();
        f(&mut self.settings);
        self.settings.stamp_changed_sections(&old, now_ms());
    }

    async fn sync(&mut self) -> SyncReport {
        let r = self.engine.sync(&self.settings).await.expect("sync");
        r.apply_to_settings(&mut self.settings);
        r
    }

    /// Signs in and runs the first sync, like the app does.
    async fn sign_in(&mut self, user: &str) {
        self.engine.complete_sign_in(&format!("google:{user}"), "verifier", "My PC").await.unwrap();
        self.sync().await;
    }

    /// Sign-in (code exchange) and the first sync, returning that sync's report.
    async fn sync_after_sign_in(&mut self, user: &str) -> SyncReport {
        self.engine.complete_sign_in(&format!("google:{user}"), "verifier", "My PC").await.unwrap();
        self.sync().await
    }

    async fn choose(&mut self, storage: KeyStorage) {
        self.engine.set_key_storage(storage).await.unwrap();
        // The app mirrors the choice into settings for the UI (it is per computer, never synced).
        self.settings.account.key_storage = storage;
    }

    /// Like `set_api_key` in the app: save locally, then upload if keys live in the account.
    async fn set_key(&mut self, provider: &str, key: &str) {
        self.keys.set(provider, key);
        self.engine.upload_key(provider, key).await.unwrap();
    }

    fn words(&self) -> Vec<String> {
        let mut w: Vec<_> = self.db.dict_all().unwrap().into_iter().map(|e| e.word).collect();
        w.sort();
        w
    }
}

const KEY_A: &str = "sk-ant-aaaaaaaaaaaaaaaaaaaaaaaa";
const KEY_B: &str = "sk-openai-bbbbbbbbbbbbbbbbbbbbbbbb";

// ---------------------------------------------------------------- tests

#[tokio::test]
async fn setting_word_snippet_and_key_changed_on_a_appear_on_b() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.edit(|s| {
        s.user_name = "Sarah".into();
        s.theme = Theme::Dark;
        s.sounds.volume = 0.2;
    });
    a.db.dict_add(&DictEntry::user("Menachem")).unwrap();
    a.db.snippet_save(&Snippet { id: 0, trigger: "my email".into(), expansion: "a@b.c".into(), uses: 0 }).unwrap();
    a.set_key("anthropic", KEY_A).await;
    a.sync().await;

    b.sign_in("u1").await;
    b.choose(KeyStorage::Cloud).await;
    let r = b.sync().await;
    assert_eq!(b.settings.user_name, "Sarah");
    assert_eq!(b.settings.theme, Theme::Dark);
    assert!((b.settings.sounds.volume - 0.2).abs() < 1e-6);
    assert_eq!(b.words(), vec!["Menachem"]);
    assert_eq!(b.db.snippets_all().unwrap()[0].expansion, "a@b.c");
    assert_eq!(b.keys.get("anthropic").as_deref(), Some(KEY_A));
    assert!(!r.first_merge);

    // A later key change on B reaches A.
    b.set_key("openai", KEY_B).await;
    a.sync().await;
    assert_eq!(a.keys.get("openai").as_deref(), Some(KEY_B));
}

#[tokio::test]
async fn deletes_propagate_and_words_can_be_readded() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    a.sign_in("u1").await;
    let wid = a.db.dict_add(&DictEntry::user("Menachem")).unwrap();
    let sid = a.db.snippet_save(&Snippet { id: 0, trigger: "sig".into(), expansion: "Best".into(), uses: 0 }).unwrap();
    a.sync().await;
    b.sign_in("u1").await;
    assert_eq!(b.words(), vec!["Menachem"]);
    assert_eq!(b.db.snippets_all().unwrap().len(), 1);

    pause().await;
    a.db.dict_delete(wid).unwrap();
    a.db.snippet_delete(sid).unwrap();
    a.sync().await;
    b.sync().await;
    assert!(b.words().is_empty());
    assert!(b.db.snippets_all().unwrap().is_empty());

    // Adding the word again on B works despite the old row, and comes back to A.
    pause().await;
    b.db.dict_add(&DictEntry::user("Menachem")).unwrap();
    b.sync().await;
    a.sync().await;
    assert_eq!(a.words(), vec!["Menachem"]);
}

#[tokio::test]
async fn the_newer_change_wins() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    a.sign_in("u1").await;
    b.sign_in("u1").await;
    a.edit(|s| s.user_name = "From A".into());
    a.db.dict_add(&DictEntry { replace: vec!["a-version".into()], ..DictEntry::user("Chaya") }).unwrap();
    a.sync().await;
    b.sync().await;
    pause().await;
    // B changes the same things later.
    b.edit(|s| s.user_name = "From B".into());
    let mut e = b.db.dict_all().unwrap().remove(0);
    e.replace = vec!["b-version".into()];
    b.db.dict_update(&e).unwrap();
    b.sync().await;
    // A syncs again and takes B's newer values; A does not push its older ones over B's.
    a.sync().await;
    b.sync().await;
    assert_eq!(a.settings.user_name, "From B");
    assert_eq!(b.settings.user_name, "From B");
    assert_eq!(a.db.dict_all().unwrap()[0].replace, vec!["b-version"]);
    assert_eq!(b.db.dict_all().unwrap()[0].replace, vec!["b-version"]);
}

#[tokio::test]
async fn local_key_storage_never_uploads() {
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.keys.set("anthropic", KEY_A); // a key that exists before signing in
    a.sign_in("u1").await;
    // Before choosing, nothing is uploaded.
    assert!(fake.server_keys().is_empty());
    a.choose(KeyStorage::Local).await;
    let gets_before = fake.calls("GET /v1/keys"); // the sign-in sync looked once (to decide)
    a.set_key("openai", KEY_B).await;
    a.sync().await;
    a.sync().await;
    assert!(fake.server_keys().is_empty());
    assert_eq!(fake.calls("PUT /v1/keys/openai") + fake.calls("PUT /v1/keys/anthropic"), 0);
    assert_eq!(fake.calls("GET /v1/keys"), gets_before, "local storage doesn't even download keys");
    assert_eq!(a.keys.get("anthropic").as_deref(), Some(KEY_A));
}

#[tokio::test]
async fn switching_storage_moves_keys_as_documented() {
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    assert_eq!(fake.server_keys().get("anthropic").map(String::as_str), Some(KEY_A));
    // Cloud -> Local: this computer stops syncing keys; its copy and the account's both stay.
    a.choose(KeyStorage::Local).await;
    assert_eq!(fake.calls("DELETE /v1/keys"), 0);
    assert_eq!(fake.server_keys().get("anthropic").map(String::as_str), Some(KEY_A));
    assert_eq!(a.keys.get("anthropic").as_deref(), Some(KEY_A));
    // Removing the account's keys is a separate, explicit action; local copies are kept.
    a.engine.remove_cloud_keys().await.unwrap();
    assert!(fake.server_keys().is_empty());
    assert_eq!(a.keys.get("anthropic").as_deref(), Some(KEY_A));
    // Local -> Cloud: this computer's keys are uploaded.
    a.choose(KeyStorage::Cloud).await;
    assert_eq!(fake.server_keys().get("anthropic").map(String::as_str), Some(KEY_A));
}

#[tokio::test]
async fn first_choice_never_wipes_keys_uploaded_by_another_computer() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    b.keys.set("anthropic", "sk-ant-own-on-b-dddddddddddddd"); // different keys: B is asked, not auto-chosen
    b.sign_in("u1").await; // takes the account's "cloud" setting
    assert!(!b.engine.key_storage_chosen());
    b.choose(KeyStorage::Local).await; // first choice on B: only this computer is affected
    assert_eq!(fake.server_keys().get("anthropic").map(String::as_str), Some(KEY_A));
    assert_eq!(b.keys.get("anthropic").as_deref(), Some("sk-ant-own-on-b-dddddddddddddd"));
}

#[tokio::test]
async fn removing_a_key_on_one_computer_removes_it_on_the_other() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    b.sign_in("u1").await;
    b.choose(KeyStorage::Cloud).await;
    assert_eq!(b.keys.get("anthropic").as_deref(), Some(KEY_A));
    a.set_key("anthropic", "").await;
    b.sync().await;
    assert!(b.keys.get("anthropic").is_none());
}

#[tokio::test]
async fn sign_out_clears_cached_keys_only_for_cloud_storage() {
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    a.db.dict_add(&DictEntry::user("Menachem")).unwrap();
    a.engine.sign_out().await;
    assert!(a.keys.get("anthropic").is_none(), "a key the account provably holds is removed");
    assert!(a.tokens.load().is_none());
    assert!(a.engine.account().is_none());
    assert_eq!(a.words(), vec!["Menachem"], "dictionary stays");
    assert_eq!(fake.calls("POST /auth/logout"), 1);

    let mut l = Computer::new(&fake.url());
    l.sign_in("u2").await;
    l.choose(KeyStorage::Local).await;
    l.keys.set("openai", KEY_B);
    l.engine.sign_out().await;
    assert_eq!(l.keys.get("openai").as_deref(), Some(KEY_B));
}

#[tokio::test]
async fn a_revoked_sign_in_becomes_signed_out_and_clears_cloud_keys() {
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    fake.revoke_all_devices(None); // an older server sends no reason: treated as "removed"
    fake.expire_access_tokens();
    let e = a.engine.sync(&a.settings).await.unwrap_err();
    assert_eq!(e, CloudError::Revoked);
    assert!(e.needs_sign_in());
    assert!(a.tokens.load().is_none());
    assert!(a.engine.account().is_none());
    assert!(a.keys.get("anthropic").is_none(), "cloud-stored keys are removed after a revocation");
}

#[tokio::test]
async fn expired_access_token_refreshes_once_and_rotates_the_refresh_token() {
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.sign_in("u1").await;
    let before = a.tokens.load().unwrap();
    fake.expire_access_tokens();
    a.sync().await;
    assert_eq!(fake.calls("POST /auth/refresh"), 1);
    assert_ne!(a.tokens.load().unwrap(), before);
    // Still working with the new tokens (no further refresh needed).
    a.sync().await;
    assert_eq!(fake.calls("POST /auth/refresh"), 1);
}

#[tokio::test]
async fn offline_is_reported_plainly_and_keeps_the_sign_in() {
    let tokens = Arc::new(MemoryTokenStore::new());
    tokens.save("some-refresh-token").unwrap();
    let client = CloudClient::new("http://127.0.0.1:1", tokens.clone());
    let e = client.get_keys().await.unwrap_err();
    assert_eq!(e, CloudError::Offline);
    assert!(e.message().contains("internet"));
    assert!(!e.needs_sign_in());
    assert!(tokens.load().is_some());
}

#[tokio::test]
async fn server_errors_use_the_servers_plain_message() {
    let server = MockServer::start().await;
    Mock::given(any()).respond_with(json_resp(400, json!({"error": "invalid_code", "message": "That sign-in link has expired."}))).mount(&server).await;
    let client = CloudClient::new(&server.uri(), Arc::new(MemoryTokenStore::new()));
    let e = client.exchange_code("x", "y", "pc").await.unwrap_err();
    assert_eq!(e, CloudError::Server("That sign-in link has expired.".into()));
}

#[tokio::test]
async fn first_sign_in_uploads_local_when_account_is_empty_and_backs_up() {
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.edit(|s| s.user_name = "Local Name".into());
    a.db.dict_add(&DictEntry::user("Menachem")).unwrap();
    a.sign_in("u1").await;
    assert!(a.dir.path().join("settings.before-signin.json").exists());
    assert!(a.dir.path().join("wisper.before-signin.db").exists());
    let backup = Db::open(&a.dir.path().join("wisper.before-signin.db")).unwrap();
    assert_eq!(backup.dict_all().unwrap().len(), 1);
    let saved: Settings = serde_json::from_slice(&std::fs::read(a.dir.path().join("settings.before-signin.json")).unwrap()).unwrap();
    assert_eq!(saved.user_name, "Local Name");
    {
        let st = fake.st.lock().unwrap();
        assert_eq!(st.settings[&("u1".to_string(), "userName".to_string())].0, "Local Name");
        assert_eq!(st.items.len(), 1);
    }
    assert!(a.engine.last_sync().is_some());
}

#[tokio::test]
async fn first_sign_in_takes_the_accounts_settings_and_keeps_never_synced_fields() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    a.sign_in("u1").await;
    a.edit(|s| {
        s.user_name = "Account Name".into();
        s.theme = Theme::Dark;
        s.microphone = Some("mic-A".into());
    });
    a.sync().await;

    b.edit(|s| {
        s.user_name = "Local Name".into();
        s.microphone = Some("mic-B".into());
        s.launch_at_login = false;
        s.onboarded = true;
    });
    b.sign_in("u1").await;
    assert_eq!(b.settings.user_name, "Account Name");
    assert_eq!(b.settings.theme, Theme::Dark);
    assert_eq!(b.settings.microphone.as_deref(), Some("mic-B"));
    assert!(!b.settings.launch_at_login && b.settings.onboarded);
    // The pre-sign-in copy still has what B had before.
    let saved: Settings = serde_json::from_slice(&std::fs::read(b.dir.path().join("settings.before-signin.json")).unwrap()).unwrap();
    assert_eq!(saved.user_name, "Local Name");
}

#[tokio::test]
async fn first_sign_in_merges_dictionary_without_duplicates() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    a.db.dict_add(&DictEntry::user("Menachem")).unwrap();
    a.db.dict_add(&DictEntry::user("OnlyA")).unwrap();
    a.db.snippet_save(&Snippet { id: 0, trigger: "sig".into(), expansion: "A text".into(), uses: 0 }).unwrap();
    pause().await;
    b.db.dict_add(&DictEntry::user("menachem")).unwrap();
    b.db.dict_add(&DictEntry::user("OnlyB")).unwrap();
    b.db.snippet_save(&Snippet { id: 0, trigger: "SIG".into(), expansion: "B text".into(), uses: 0 }).unwrap();
    a.sign_in("u1").await;
    b.sign_in("u1").await;
    a.sync().await;
    b.sync().await;
    assert_eq!(a.words().len(), 3);
    assert_eq!(b.words().len(), 3);
    assert_eq!(a.db.snippets_all().unwrap().len(), 1);
    assert_eq!(b.db.snippets_all().unwrap().len(), 1);
    // B's entries were newer, so B's snippet text wins everywhere.
    assert_eq!(a.db.snippets_all().unwrap()[0].expansion, "B text");
    assert_eq!(b.db.snippets_all().unwrap()[0].expansion, "B text");
}

#[tokio::test]
async fn usage_numbers_combine_across_computers_without_double_counting() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    let item = |id: &str, words: i64| HistoryItem {
        id: id.into(),
        created_at: now_ms(),
        kind: "dictation".into(),
        status: "done".into(),
        word_count: words,
        duration_ms: 3000,
        app_name: Some("Gmail".into()),
        final_text: Some("secret text that must never be uploaded".into()),
        ..Default::default()
    };
    a.db.insert_dictation(&item("a1", 10)).unwrap();
    b.db.insert_dictation(&item("b1", 5)).unwrap();
    a.sign_in("u1").await;
    b.sign_in("u1").await;
    a.sync().await;
    b.sync().await;
    let now = chrono::Local::now();
    assert_eq!(a.db.stats_with(40, now, true).unwrap().total_words, 15);
    assert_eq!(b.db.stats_with(40, now, true).unwrap().total_words, 15);
    assert_eq!(a.db.stats(40, now).unwrap().total_words, 10, "this-computer scope is unchanged");
    // Syncing again changes nothing, and no dictated text ever reaches the server.
    a.sync().await;
    assert_eq!(a.db.stats_with(40, now, true).unwrap().total_words, 15);
    let st = fake.st.lock().unwrap();
    assert_eq!(st.stats.len(), 2);
    assert!(!format!("{:?}", st.stats).contains("secret"));
}

#[tokio::test]
async fn account_deletion_clears_cloud_data_and_signs_out() {
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    a.engine.delete_account().await.unwrap();
    assert!(fake.server_keys().is_empty());
    assert!(fake.st.lock().unwrap().settings.is_empty());
    assert!(a.engine.account().is_none());
    // Only the cloud copy is destroyed: this computer keeps its keys.
    assert_eq!(a.keys.get("anthropic").as_deref(), Some(KEY_A));
}

#[tokio::test]
async fn devices_can_be_listed_and_removed() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    a.sign_in("u1").await;
    b.sign_in("u1").await;
    let devices = a.engine.client().devices().await.unwrap();
    assert_eq!(devices.len(), 2);
    let other = devices.iter().find(|d| !d.current).unwrap().id.clone();
    a.engine.client().remove_device(&other).await.unwrap();
    assert_eq!(a.engine.client().devices().await.unwrap().len(), 1);
    // The removed computer finds out on its next sync.
    fake.expire_access_tokens();
    assert_eq!(b.engine.sync(&b.settings).await.unwrap_err(), CloudError::Revoked);
}

// ---------------------------------------------------------------- review fixes

#[tokio::test]
async fn sign_out_before_choosing_keeps_the_only_copy_of_the_keys() {
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.keys.set("anthropic", KEY_A);
    a.sign_in("u1").await; // storage was never chosen (default "cloud" must not count)
    a.engine.sign_out().await;
    assert_eq!(a.keys.get("anthropic").as_deref(), Some(KEY_A));
}

#[tokio::test]
async fn revocation_before_choosing_keeps_keys() {
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.keys.set("anthropic", KEY_A);
    a.sign_in("u1").await;
    fake.revoke_all_devices(Some("removed"));
    fake.expire_access_tokens();
    assert_eq!(a.engine.sync(&a.settings).await.unwrap_err(), CloudError::Revoked);
    assert_eq!(a.keys.get("anthropic").as_deref(), Some(KEY_A));
}

#[tokio::test]
async fn keys_that_never_reached_the_account_are_kept_on_sign_out() {
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await; // uploaded and recorded
    // A key typed while offline: saved here, upload failed, so the account doesn't have it.
    a.keys.set("openai", KEY_B);
    a.engine.sign_out().await;
    assert!(a.keys.get("anthropic").is_none());
    assert_eq!(a.keys.get("openai").as_deref(), Some(KEY_B));
    // A synced key that was edited afterwards (and not uploaded) is kept too.
    let mut b = Computer::new(&fake.url());
    b.sign_in("u2").await;
    b.choose(KeyStorage::Cloud).await;
    b.set_key("anthropic", KEY_A).await;
    b.keys.set("anthropic", "sk-ant-edited-and-not-uploaded-yet");
    b.engine.sign_out().await;
    assert_eq!(b.keys.get("anthropic").as_deref(), Some("sk-ant-edited-and-not-uploaded-yet"));
}

#[tokio::test]
async fn replayed_token_or_deleted_account_signs_out_but_keeps_keys() {
    for reason in ["reuse", "account_deleted"] {
        let fake = Fake::start().await;
        let mut a = Computer::new(&fake.url());
        a.sign_in("u1").await;
        a.choose(KeyStorage::Cloud).await;
        a.set_key("anthropic", KEY_A).await;
        fake.revoke_all_devices(Some(reason));
        fake.expire_access_tokens();
        let e = a.engine.sync(&a.settings).await.unwrap_err();
        assert_eq!(e, CloudError::RevokedKeepKeys, "{reason}");
        assert!(e.needs_sign_in());
        assert_eq!(e.message(), "Please sign in again.");
        assert!(a.engine.account().is_none());
        assert_eq!(a.keys.get("anthropic").as_deref(), Some(KEY_A), "{reason} keeps keys");
    }
    // "removed" (explicitly) clears the provably synced key.
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    fake.revoke_all_devices(Some("removed"));
    fake.expire_access_tokens();
    assert_eq!(a.engine.sync(&a.settings).await.unwrap_err(), CloudError::Revoked);
    assert!(a.keys.get("anthropic").is_none());
}

#[tokio::test]
async fn a_stale_cloud_snapshot_cannot_reupload_keys_after_switching_to_local() {
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    let stale = a.settings.clone(); // a sync that read settings before the switch
    assert_eq!(stale.account.key_storage, KeyStorage::Cloud);
    // The switch and a key save race; they are serialized by the engine lock.
    let (switched, uploaded) = tokio::join!(a.engine.set_key_storage(KeyStorage::Local), a.engine.upload_key("openai", KEY_B));
    switched.unwrap();
    uploaded.unwrap();
    // Once switched, nothing more is uploaded even from a stale Cloud snapshot.
    let puts = fake.calls("PUT /v1/keys/anthropic") + fake.calls("PUT /v1/keys/openai");
    a.engine.sync(&stale).await.unwrap();
    a.engine.upload_key("anthropic", "sk-ant-changed-after-switching-0000").await.unwrap();
    assert_eq!(fake.server_keys().get("anthropic").map(String::as_str), Some(KEY_A));
    assert_eq!(fake.calls("PUT /v1/keys/anthropic") + fake.calls("PUT /v1/keys/openai"), puts);
}

#[tokio::test]
async fn key_storage_is_per_computer_and_never_synced() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    a.sign_in("u1").await;
    a.choose(KeyStorage::Local).await;
    a.edit(|s| s.user_name = "Sarah".into());
    a.sync().await;
    b.sign_in("u1").await;
    b.choose(KeyStorage::Cloud).await;
    b.sync().await;
    assert_eq!(b.settings.user_name, "Sarah");
    assert_eq!(b.settings.account.key_storage, KeyStorage::Cloud);
    assert_eq!(b.engine.key_storage(), Some(KeyStorage::Cloud));
    assert_eq!(a.engine.key_storage(), Some(KeyStorage::Local));
    assert!(!fake.st.lock().unwrap().settings.keys().any(|(_, name)| name.contains("keyStorage")));
}

#[tokio::test]
async fn sign_out_waits_for_a_sync_in_progress() {
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.sign_in("u1").await;
    fake.st.lock().unwrap().settings_delay_ms = 400;
    let (engine, settings) = (a.engine.clone(), a.settings.clone());
    let running = tokio::spawn(async move { engine.sync(&settings).await });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    a.engine.sign_out().await;
    // The sync had to finish first, so it can't write its bookkeeping back afterwards.
    assert!(running.is_finished());
    assert!(a.engine.last_sync().is_none());
    assert!(a.engine.account().is_none());
}

#[tokio::test]
async fn a_different_account_gets_a_fresh_timestamped_backup() {
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.edit(|s| s.user_name = "Before first account".into());
    a.sign_in("u1").await;
    a.engine.sign_out().await;
    a.edit(|s| s.user_name = "Before second account".into());
    a.sign_in("u2").await;
    let names: Vec<String> = std::fs::read_dir(a.dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    let settings_backups: Vec<_> = names.iter().filter(|n| n.starts_with("settings.before-signin") && n.ends_with(".json")).collect();
    assert_eq!(settings_backups.len(), 2, "{names:?}");
    assert_eq!(names.iter().filter(|n| n.starts_with("wisper.before-signin") && n.ends_with(".db")).count(), 2);
    let first: Settings = serde_json::from_slice(&std::fs::read(a.dir.path().join("settings.before-signin.json")).unwrap()).unwrap();
    assert_eq!(first.user_name, "Before first account");
    let newer = settings_backups.iter().find(|n| **n != "settings.before-signin.json").unwrap();
    let second: Settings = serde_json::from_slice(&std::fs::read(a.dir.path().join(newer)).unwrap()).unwrap();
    assert_eq!(second.user_name, "Before second account");
    // Signing the same account in again later does not make yet another copy.
    a.engine.sign_out().await;
    a.sign_in("u2").await;
    assert_eq!(std::fs::read_dir(a.dir.path()).unwrap().count(), names.len());
}

#[tokio::test]
async fn first_choice_keeps_the_replaced_local_key_as_previous() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    b.keys.set("anthropic", KEY_B); // a different key already on this computer
    b.sign_in("u1").await;
    b.choose(KeyStorage::Cloud).await;
    assert_eq!(b.keys.get("anthropic").as_deref(), Some(KEY_A), "the account's key is used");
    assert_eq!(b.keys.get("previous-anthropic").as_deref(), Some(KEY_B), "the replaced key is not lost");
}

#[tokio::test]
async fn uploaded_items_are_marked_clean() {
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.sign_in("u1").await;
    a.db.dict_add(&DictEntry::user("Menachem")).unwrap();
    assert_eq!(a.db.items_dirty().unwrap().len(), 1);
    a.sync().await;
    assert!(a.db.items_dirty().unwrap().is_empty());
    assert_eq!(fake.st.lock().unwrap().items.len(), 1);
}

#[tokio::test]
async fn a_new_account_receives_this_computers_words_and_snippets() {
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.db.dict_add(&DictEntry::user("Menachem")).unwrap();
    a.db.snippet_save(&Snippet { id: 0, trigger: "sig".into(), expansion: "Best".into(), uses: 0 }).unwrap();
    a.sign_in("u1").await;
    a.engine.sign_out().await;
    a.sign_in("u2").await;
    let st = fake.st.lock().unwrap();
    for user in ["u1", "u2"] {
        assert_eq!(st.items.keys().filter(|(u, _, _)| u == user).count(), 2, "{user} has the word and the snippet");
    }
}

#[tokio::test]
async fn removed_or_replaced_keys_are_stashed_once_so_they_can_be_recovered() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    b.sign_in("u1").await;
    b.choose(KeyStorage::Cloud).await;
    // Another computer removes the key; B blanks its synced copy but stashes it first.
    a.set_key("anthropic", "").await;
    b.sync().await;
    assert!(b.keys.get("anthropic").is_none());
    assert_eq!(b.keys.get("previous-anthropic").as_deref(), Some(KEY_A));
    // A later rotation (key synced before) does not overwrite the stashed original.
    a.set_key("anthropic", KEY_B).await;
    b.sync().await;
    a.set_key("anthropic", "sk-ant-rotated-again-0000000000").await;
    b.keys.set("anthropic", KEY_B);
    b.sync().await;
    assert_eq!(b.keys.get("anthropic").as_deref(), Some("sk-ant-rotated-again-0000000000"));
    assert_eq!(b.keys.get("previous-anthropic").as_deref(), Some(KEY_A));
}

#[tokio::test]
async fn removing_cloud_keys_switches_this_computer_to_local_and_others_keep_their_keys() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    b.sign_in("u1").await;
    b.choose(KeyStorage::Cloud).await;
    assert_eq!(b.keys.get("anthropic").as_deref(), Some(KEY_A));
    pause().await;

    a.engine.remove_cloud_keys().await.unwrap();
    assert!(fake.server_keys().is_empty());
    assert_eq!(a.engine.key_storage(), Some(KeyStorage::Local));
    assert_eq!(a.keys.get("anthropic").as_deref(), Some(KEY_A));

    // Several later syncs on both computers: the account stays empty and nobody loses a key.
    let first = b.sync().await;
    assert!(first.switched_to_local);
    assert_eq!(first.notice.as_deref(), Some(sync::KEYS_REMOVED_NOTICE));
    for _ in 0..3 {
        a.sync().await;
        let r = b.sync().await;
        assert!(r.notice.is_none());
    }
    assert!(fake.server_keys().is_empty(), "nothing re-uploaded the keys");
    assert_eq!(b.keys.get("anthropic").as_deref(), Some(KEY_A), "B keeps its working key");
    assert!(b.keys.get("previous-anthropic").is_none(), "nothing was blanked or stashed");
    assert_eq!(b.engine.key_storage(), Some(KeyStorage::Local));

    // Switching A back to Cloud re-uploads; B stays Local until its user changes it.
    a.choose(KeyStorage::Cloud).await;
    assert_eq!(fake.server_keys().get("anthropic").map(String::as_str), Some(KEY_A));
    a.sync().await;
    b.sync().await;
    assert_eq!(b.engine.key_storage(), Some(KeyStorage::Local));
    assert_eq!(b.keys.get("anthropic").as_deref(), Some(KEY_A));
    // A later removal is news again, even after the account had keys.
    b.choose(KeyStorage::Cloud).await;
    a.engine.remove_cloud_keys().await.unwrap();
    pause().await;
    assert!(b.sync().await.switched_to_local);
}

#[tokio::test]
async fn removing_one_provider_still_propagates_but_removing_all_does_not_blank_keys() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    a.set_key("openai", KEY_B).await;
    b.sign_in("u1").await;
    b.choose(KeyStorage::Cloud).await;
    a.set_key("openai", "").await; // one provider removed: propagates (previous stashed)
    let r = b.sync().await;
    assert!(!r.switched_to_local);
    assert!(b.keys.get("openai").is_none());
    assert_eq!(b.keys.get("previous-openai").as_deref(), Some(KEY_B));
    assert_eq!(b.keys.get("anthropic").as_deref(), Some(KEY_A));
    assert_eq!(b.engine.key_storage(), Some(KeyStorage::Cloud));
}

#[tokio::test]
async fn choosing_cloud_again_after_own_removal_sticks_even_with_no_keys() {
    // Reviewer's case: remove keys from the account, clear this computer's key too, then pick
    // "Your account" again with nothing to upload. The next sync must not flip back to Local.
    let fake = Fake::start().await;
    let mut a = Computer::new(&fake.url());
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    pause().await;
    a.engine.remove_cloud_keys().await.unwrap();
    a.keys.set("anthropic", "");
    a.choose(KeyStorage::Cloud).await;
    let r = a.sync().await;
    assert!(!r.switched_to_local);
    assert!(r.notice.is_none());
    assert_eq!(a.engine.key_storage(), Some(KeyStorage::Cloud));
}

// ---------------------------------------------------------------- automatic key choice

#[tokio::test]
async fn second_computer_with_no_keys_gets_the_accounts_keys_at_sign_in_without_any_click() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    a.set_key("openai", KEY_B).await;

    b.engine.complete_sign_in("google:u1", "verifier", "Second PC").await.unwrap();
    let r = b.sync().await; // the sync that follows sign-in
    assert_eq!(b.keys.get("anthropic").as_deref(), Some(KEY_A));
    assert_eq!(b.keys.get("openai").as_deref(), Some(KEY_B));
    assert!(r.keys_changed && r.auto_chose_cloud && !r.switched_to_local);
    assert_eq!(r.notice.as_deref(), Some(sync::KEYS_AUTO_SYNCED_NOTICE));
    assert!(b.engine.key_storage_chosen());
    assert_eq!(b.engine.key_storage(), Some(KeyStorage::Cloud));
    assert_eq!(b.engine.key_choice_situation(), None);
    // Later changes keep flowing without any user action.
    a.set_key("anthropic", "sk-ant-rotated-cccccccccccccccc").await;
    b.sync().await;
    assert_eq!(b.keys.get("anthropic").as_deref(), Some("sk-ant-rotated-cccccccccccccccc"));
}

#[tokio::test]
async fn the_choice_is_retried_on_later_syncs_while_not_chosen() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    b.sign_in("u1").await; // account has no keys yet: nothing to decide
    assert!(!b.engine.key_storage_chosen());
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    let r = b.sync().await;
    assert!(r.auto_chose_cloud);
    assert_eq!(b.keys.get("anthropic").as_deref(), Some(KEY_A));
    assert_eq!(b.engine.key_storage(), Some(KeyStorage::Cloud));
}

#[tokio::test]
async fn second_computer_with_its_own_different_keys_is_asked_and_nothing_is_overwritten() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    b.keys.set("anthropic", "sk-ant-own-on-b-dddddddddddddd");
    let r = b.sync_after_sign_in("u1").await;
    assert!(!r.auto_chose_cloud && !r.keys_changed);
    assert!(!b.engine.key_storage_chosen());
    assert_eq!(b.engine.key_storage(), None);
    assert_eq!(b.engine.key_choice_situation().as_deref(), Some("differ"));
    assert_eq!(b.keys.get("anthropic").as_deref(), Some("sk-ant-own-on-b-dddddddddddddd"));
    assert_eq!(fake.server_keys().get("anthropic").map(String::as_str), Some(KEY_A));
    b.sync().await; // more syncs don't decide for the user either
    assert!(!b.engine.key_storage_chosen());
    assert_eq!(b.keys.get("anthropic").as_deref(), Some("sk-ant-own-on-b-dddddddddddddd"));
}

#[tokio::test]
async fn own_keys_and_an_empty_account_still_asks() {
    let fake = Fake::start().await;
    let mut b = Computer::new(&fake.url());
    b.keys.set("openai", KEY_B);
    let r = b.sync_after_sign_in("u1").await;
    assert!(!r.auto_chose_cloud);
    assert!(!b.engine.key_storage_chosen());
    assert_eq!(b.engine.key_choice_situation().as_deref(), Some("own_only"));
    assert!(fake.server_keys().is_empty(), "nothing is uploaded before the user chooses");
}

#[tokio::test]
async fn the_same_keys_on_both_sides_choose_the_account_automatically() {
    let fake = Fake::start().await;
    let (mut a, mut b) = (Computer::new(&fake.url()), Computer::new(&fake.url()));
    a.sign_in("u1").await;
    a.choose(KeyStorage::Cloud).await;
    a.set_key("anthropic", KEY_A).await;
    b.keys.set("anthropic", KEY_A);
    let r = b.sync_after_sign_in("u1").await;
    assert!(r.auto_chose_cloud && !r.keys_changed);
    assert_eq!(b.engine.key_storage(), Some(KeyStorage::Cloud));
    assert_eq!(b.keys.get("anthropic").as_deref(), Some(KEY_A));
}

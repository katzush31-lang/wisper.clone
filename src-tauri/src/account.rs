//! Optional Wisper account (Google sign-in, synced keys/settings/dictionary/snippets/stats).
//! The logic lives in `wisper_core::cloud`; this file adds the desktop parts: opening the
//! browser, the one-shot loopback listener, Credential Manager storage, background sync
//! triggers and the Tauri commands the UI calls. Sync runs in the background and never blocks
//! dictation; errors only update the status.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use wisper_core::cloud::{self, CloudClient, KeyStore, SyncEngine, SyncReport, TokenStore};
use wisper_core::db::Db;
use wisper_core::settings::KeyStorage;

use crate::commands::{self, Paths};
use crate::controller::ControllerState;
use crate::secrets;

type Res<T> = Result<T, String>;

const NOT_AVAILABLE: &str = "Accounts aren't available in this build of Wisper.";
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------- storage

/// The refresh token lives in Windows Credential Manager (service `Wisper`, account
/// `cloud-refresh-token`). If the OS store is unavailable (development on Linux) it is kept in
/// memory for the session only.
struct CredentialTokenStore;

fn token_memory() -> &'static Mutex<Option<String>> {
    static M: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(None))
}

fn token_entry() -> Option<keyring::Entry> {
    keyring::Entry::new("Wisper", "cloud-refresh-token").ok()
}

impl TokenStore for CredentialTokenStore {
    fn load(&self) -> Option<String> {
        if let Some(t) = token_memory().lock().clone() {
            return Some(t);
        }
        token_entry()?.get_password().ok().filter(|t| !t.is_empty())
    }

    fn save(&self, token: &str) -> Result<(), String> {
        match token_entry().map(|e| e.set_password(token)) {
            Some(Ok(())) => {
                *token_memory().lock() = None;
            }
            _ => {
                tracing::warn!("credential store unavailable; keeping the sign-in in memory only");
                *token_memory().lock() = Some(token.to_string());
            }
        }
        Ok(())
    }

    fn clear(&self) {
        *token_memory().lock() = None;
        if let Some(e) = token_entry() {
            let _ = e.delete_credential();
        }
    }
}

/// The same key entries dictation reads; changing one also refreshes the running engine.
struct AppKeyStore {
    app: AppHandle,
}

impl KeyStore for AppKeyStore {
    fn get(&self, provider: &str) -> Option<String> {
        secrets::get(provider)
    }

    fn set(&self, provider: &str, key: &str) {
        let _ = secrets::set(provider, key);
        ControllerState::get(&self.app).engine.set_keys(commands::load_keys());
    }

    /// When the account's key replaces a different local one, the local one is kept here so
    /// nothing is irrecoverably lost.
    fn keep_previous(&self, provider: &str, key: &str) {
        secrets::set_named(&format!("previous-{provider}-api-key"), key);
    }
}

// ---------------------------------------------------------------- state

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    available: bool,
    signed_in: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_id: Option<String>,
    key_storage: &'static str,
    /// False until the user has picked where keys live after signing in (keys aren't synced before).
    key_storage_chosen: bool,
    /// While not chosen and a question is needed: "own_only" (this computer's keys would be
    /// uploaded) or "differ" (the account's different keys would be used here).
    #[serde(skip_serializing_if = "Option::is_none")]
    key_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_sync: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sync_error: Option<String>,
    /// A friendly heads-up (not an error), e.g. keys were removed from the account elsewhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    notice: Option<String>,
}

pub struct Account {
    app: AppHandle,
    /// None when this build has no account service URL.
    engine: Option<Arc<SyncEngine>>,
    last_error: Mutex<Option<String>>,
    notice: Mutex<Option<String>>,
    /// Bumped on every change; a debounced sync only runs if nothing newer came in.
    generation: AtomicU64,
    signing_in: AtomicBool,
    /// Lets `account_cancel_sign_in` stop the waiting listener at once.
    cancel_sign_in: Mutex<Option<Arc<tokio::sync::Notify>>>,
    last_focus_sync: Mutex<Option<Instant>>,
}

/// Called once at startup (after the controller and paths are managed).
pub fn init(app: &AppHandle, db: Arc<Db>, data_dir: std::path::PathBuf) {
    let url = option_env!("WISPER_CLOUD_URL").map(str::trim).filter(|u| !u.is_empty());
    let engine = url.map(|u| {
        let client = CloudClient::new(u, Arc::new(CredentialTokenStore));
        SyncEngine::new(client, db, Arc::new(AppKeyStore { app: app.clone() }), data_dir)
    });
    app.manage(Arc::new(Account {
        app: app.clone(),
        engine,
        last_error: Mutex::new(None),
        notice: Mutex::new(None),
        generation: AtomicU64::new(0),
        signing_in: AtomicBool::new(false),
        cancel_sign_in: Mutex::new(None),
        last_focus_sync: Mutex::new(None),
    }));
}

fn account(app: &AppHandle) -> Arc<Account> {
    app.state::<Arc<Account>>().inner().clone()
}

/// Something that syncs changed locally (settings, dictionary, snippets, keys, a finished
/// dictation): sync soon, in the background.
pub fn notify_changed(app: &AppHandle) {
    if let Some(a) = app.try_state::<Arc<Account>>() {
        a.trigger_soon();
    }
}

/// The hub window gained focus.
pub fn on_focus(app: &AppHandle) {
    if let Some(a) = app.try_state::<Arc<Account>>() {
        let mut last = a.last_focus_sync.lock();
        if last.map(|t| t.elapsed() > Duration::from_secs(30)).unwrap_or(true) {
            *last = Some(Instant::now());
            drop(last);
            a.trigger_soon();
        }
    }
}

/// Syncs on start, then every 5 minutes.
pub fn start(app: &AppHandle) {
    let a = account(app);
    if a.engine.is_none() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            a.run_sync().await;
            tokio::time::sleep(Duration::from_secs(300)).await;
        }
    });
}

/// After a key was set on this computer: upload it when keys live in the account.
pub fn key_changed(app: &AppHandle, provider: &str, key: &str) {
    let Some(a) = app.try_state::<Arc<Account>>() else { return };
    let a = a.inner().clone();
    let (provider, key) = (provider.to_string(), key.to_string());
    tauri::async_runtime::spawn(async move {
        if let Some(engine) = &a.engine {
            let settings = ControllerState::get(&a.app).engine.settings();
            if engine.account().is_some() && settings.account.sync_enabled {
                // A failure here is fine: the next sync notices the changed key and uploads it.
                let _ = engine.upload_key(&provider, &key).await;
            }
        }
    });
}

fn device_name() -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .map(|v| v.trim().to_string())
        .find(|v| !v.is_empty())
        .unwrap_or_else(|| "This computer".to_string())
}

impl Account {
    pub fn status(&self) -> AccountStatus {
        let settings = ControllerState::get(&self.app).engine.settings();
        let info = self.engine.as_ref().and_then(|e| e.account());
        AccountStatus {
            available: self.engine.is_some(),
            signed_in: info.is_some(),
            email: info.as_ref().map(|i| i.email.clone()),
            name: info.as_ref().and_then(|i| i.name.clone()),
            device_id: info.as_ref().map(|i| i.device_id.clone()),
            // Per computer: the engine's choice once made, else whatever the settings mirror says.
            key_storage: self.engine.as_ref().and_then(|e| e.key_storage()).unwrap_or(settings.account.key_storage).as_str(),
            key_storage_chosen: self.engine.as_ref().map(|e| e.key_storage_chosen()).unwrap_or(false),
            key_choice: self.engine.as_ref().and_then(|e| e.key_choice_situation()),
            last_sync: self.engine.as_ref().and_then(|e| e.last_sync()),
            sync_error: self.last_error.lock().clone(),
            notice: self.notice.lock().clone(),
        }
    }

    fn emit(&self) -> AccountStatus {
        let s = self.status();
        let _ = self.app.emit("account-changed", &s);
        s
    }

    fn engine(&self) -> Res<&Arc<SyncEngine>> {
        self.engine.as_ref().ok_or_else(|| NOT_AVAILABLE.to_string())
    }

    fn signed_in_engine(&self) -> Res<&Arc<SyncEngine>> {
        let e = self.engine()?;
        if e.account().is_none() {
            return Err("You're not signed in.".into());
        }
        Ok(e)
    }

    /// Sync ~2 seconds after the last change.
    pub fn trigger_soon(self: &Arc<Self>) {
        if self.engine.is_none() {
            return;
        }
        let g = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if me.generation.load(Ordering::SeqCst) == g {
                me.run_sync().await;
            }
        });
    }

    /// One sync, if signed in with syncing on. Errors only update the status.
    async fn run_sync(self: &Arc<Self>) {
        let Some(engine) = self.engine.clone() else { return };
        let settings = ControllerState::get(&self.app).engine.settings();
        if !settings.account.sync_enabled || engine.account().is_none() {
            return;
        }
        match engine.sync(&settings).await {
            Ok(report) => {
                *self.last_error.lock() = None;
                // Keys already changed on this computer (and are in use by dictation): tell the
                // Providers page, Home and the key status straight away, without waiting for idle.
                if report.keys_changed || report.auto_chose_cloud {
                    let c = ControllerState::get(&self.app);
                    if c.engine.requeue_auth_failures() > 0 {
                        let _ = self.app.emit("history-changed", "");
                    }
                    let _ = self.app.emit("keys-changed", "");
                }
                // Never change settings in the middle of a recording or while a dictation is
                // processing: wait until Wisper is idle.
                let c = ControllerState::get(&self.app);
                while !c.is_idle() {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                self.apply_report(&report);
            }
            Err(e) => {
                tracing::info!("sync failed: {e}");
                *self.last_error.lock() = Some(e.message());
            }
        }
        self.emit();
    }

    fn apply_report(&self, r: &SyncReport) {
        let c = ControllerState::get(&self.app);
        // Apply to the settings as they are now (they may have been edited during the sync).
        let mut cur = c.engine.settings();
        if r.apply_to_settings(&mut cur) {
            let file = self.app.state::<Paths>().settings_file.clone();
            if let Err(e) = cur.save(&file) {
                tracing::warn!("couldn't save synced settings: {e}");
            }
            commands::apply_settings(&self.app, &cur);
        }
        if r.switched_to_local {
            // This computer now keeps its own keys: mirror that into settings for the UI.
            let _ = commands::update_settings(&self.app, |s| s.account.key_storage = KeyStorage::Local);
            *self.notice.lock() = r.notice.clone();
        }
        if r.auto_chose_cloud {
            // This computer picked "Your account (synced)" by itself: mirror it for the UI.
            let _ = commands::update_settings(&self.app, |s| s.account.key_storage = KeyStorage::Cloud);
            *self.notice.lock() = r.notice.clone();
        }
        if r.items_changed {
            let _ = self.app.emit("dictionary-changed", "");
            let _ = self.app.emit("snippets-changed", "");
        }
        if r.stats_changed {
            let _ = self.app.emit("history-changed", "");
        }
    }

    async fn sign_in(self: &Arc<Self>) -> Res<AccountStatus> {
        let engine = self.engine()?.clone();
        if self.signing_in.swap(true, Ordering::SeqCst) {
            return Err("Sign-in is already open in your browser.".into());
        }
        let result = self.sign_in_inner(&engine).await;
        self.signing_in.store(false, Ordering::SeqCst);
        result?;
        *self.last_error.lock() = None;
        self.emit();
        // First sync (the merge with whatever the account already holds).
        self.run_sync().await;
        Ok(self.status())
    }

    async fn sign_in_inner(&self, engine: &Arc<SyncEngine>) -> Res<()> {
        use tauri_plugin_opener::OpenerExt;
        let verifier = cloud::new_verifier();
        let challenge = cloud::pkce_challenge(&verifier);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(|_| "Wisper couldn't start the sign-in. Please try again.".to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let url = engine.client().login_url(&challenge, port);
        self.app.opener().open_url(url, None::<&str>).map_err(|_| "Wisper couldn't open your web browser to sign in.".to_string())?;
        let cancel = Arc::new(tokio::sync::Notify::new());
        *self.cancel_sign_in.lock() = Some(cancel.clone());
        let code = wait_for_code(listener, &cancel).await;
        *self.cancel_sign_in.lock() = None;
        let code = code?;
        engine.complete_sign_in(&code, &verifier, &device_name()).await.map_err(|e| e.message())?;
        Ok(())
    }
}

// ---------------------------------------------------------------- loopback listener

const PAGE_OK: &str = "<!doctype html><meta charset=utf-8><title>Wisper</title><body style=\"font-family:system-ui,sans-serif;text-align:center;margin-top:20vh\"><h2>You're signed in</h2><p>You can close this tab and go back to Wisper.</p></body>";
const PAGE_CANCELLED: &str = "<!doctype html><meta charset=utf-8><title>Wisper</title><body style=\"font-family:system-ui,sans-serif;text-align:center;margin-top:20vh\"><h2>Sign-in cancelled</h2><p>You can close this tab. Nothing was changed.</p></body>";

async fn respond(sock: &mut tokio::net::TcpStream, status: &str, body: &str) {
    let msg = format!("HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = sock.write_all(msg.as_bytes()).await;
    let _ = sock.shutdown().await;
}

/// Reads the first request line of one connection. Returns None to keep waiting.
async fn read_callback(sock: &mut tokio::net::TcpStream) -> Option<Result<String, String>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(2).any(|w| w == b"\r\n") && buf.len() < 8192 {
        match sock.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let line = text.lines().next().unwrap_or("");
    match cloud::parse_callback(line) {
        cloud::Callback::Code(code) => {
            respond(sock, "200 OK", PAGE_OK).await;
            Some(Ok(code))
        }
        cloud::Callback::Error(e) => {
            respond(sock, "200 OK", PAGE_CANCELLED).await;
            Some(Err(cloud::callback_error_message(&e)))
        }
        cloud::Callback::Other => {
            respond(sock, "404 Not Found", "Not found").await;
            None
        }
    }
}

/// Waits (up to 5 minutes) for the browser to come back to `http://127.0.0.1:{port}/callback`.
/// Only `GET /callback` is accepted; the listener is dropped (closed) as soon as it returns.
async fn wait_for_code(listener: tokio::net::TcpListener, cancel: &tokio::sync::Notify) -> Res<String> {
    let wait = async {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { continue };
            // A connection that says nothing must not block the real one.
            if let Ok(Some(result)) = tokio::time::timeout(Duration::from_secs(5), read_callback(&mut sock)).await {
                return result;
            }
        }
    };
    tokio::select! {
        r = tokio::time::timeout(SIGN_IN_TIMEOUT, wait) => r.unwrap_or_else(|_| Err("Sign-in took too long and was stopped. Please try again.".to_string())),
        // Dropping the listener here closes the port straight away.
        _ = cancel.notified() => Err("Sign-in was cancelled.".to_string()),
    }
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub fn account_status(app: AppHandle) -> AccountStatus {
    account(&app).status()
}

#[tauri::command]
pub async fn account_sign_in(app: AppHandle) -> Res<AccountStatus> {
    account(&app).sign_in().await
}

#[tauri::command]
pub async fn account_sign_out(app: AppHandle) -> Res<AccountStatus> {
    let a = account(&app);
    let engine = a.engine()?.clone();
    engine.sign_out().await;
    *a.last_error.lock() = None;
    *a.notice.lock() = None;
    Ok(a.emit())
}

/// Stops a sign-in that is waiting for the browser (the listener closes immediately).
#[tauri::command]
pub fn account_cancel_sign_in(app: AppHandle) -> AccountStatus {
    let a = account(&app);
    if let Some(n) = a.cancel_sign_in.lock().take() {
        n.notify_one();
    }
    a.status()
}

#[tauri::command]
pub async fn account_sync_now(app: AppHandle) -> Res<AccountStatus> {
    let a = account(&app);
    a.signed_in_engine()?;
    a.run_sync().await;
    Ok(a.status())
}

#[tauri::command]
pub async fn account_set_key_storage(app: AppHandle, storage: String) -> Res<AccountStatus> {
    let a = account(&app);
    let engine = a.signed_in_engine()?.clone();
    let to = KeyStorage::parse(&storage).ok_or("Choose either \"local\" or \"cloud\".")?;
    // The engine records the choice itself (under its lock); settings only mirror it for the UI.
    let keys_changed = engine.set_key_storage(to).await.map_err(|e| e.message())?;
    commands::update_settings(&app, |s| s.account.key_storage = to)?;
    *a.notice.lock() = None;
    if keys_changed {
        let _ = app.emit("keys-changed", "");
    }
    a.trigger_soon();
    Ok(a.emit())
}

/// Deletes the keys kept in the account (this computer's own copies stay).
#[tauri::command]
pub async fn account_remove_cloud_keys(app: AppHandle) -> Res<AccountStatus> {
    let a = account(&app);
    a.signed_in_engine()?.remove_cloud_keys().await.map_err(|e| e.message())?;
    // This computer now keeps its own keys (the engine recorded it); mirror that for the UI.
    commands::update_settings(&app, |s| s.account.key_storage = KeyStorage::Local)?;
    *a.notice.lock() = None;
    a.trigger_soon();
    Ok(a.emit())
}

#[tauri::command]
pub async fn account_devices(app: AppHandle) -> Res<Vec<cloud::Device>> {
    let a = account(&app);
    a.signed_in_engine()?.client().devices().await.map_err(|e| e.message())
}

#[tauri::command]
pub async fn account_remove_device(app: AppHandle, id: String) -> Res<()> {
    let a = account(&app);
    let engine = a.signed_in_engine()?.clone();
    if engine.account().map(|i| i.device_id == id).unwrap_or(false) {
        // Removing this very computer is the same as signing out.
        engine.sign_out().await;
        a.emit();
        return Ok(());
    }
    engine.client().remove_device(&id).await.map_err(|e| e.message())
}

#[tauri::command]
pub async fn account_delete(app: AppHandle) -> Res<AccountStatus> {
    let a = account(&app);
    let engine = a.signed_in_engine()?.clone();
    engine.delete_account().await.map_err(|e| e.message())?;
    *a.last_error.lock() = None;
    Ok(a.emit())
}

/// For `get_stats`: true when numbers from all computers should be combined.
pub fn combined_stats(app: &AppHandle) -> bool {
    app.try_state::<Arc<Account>>().map(|a| a.engine.as_ref().map(|e| e.account().is_some()).unwrap_or(false)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpStream;

    /// Sends one raw request to the listener and returns the reply text.
    async fn request(port: u16, line: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(format!("{line}\r\nHost: x\r\n\r\n").as_bytes()).await.unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out).await;
        out
    }

    async fn listener() -> (tokio::net::TcpListener, u16) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = l.local_addr().unwrap().port();
        (l, p)
    }

    #[tokio::test]
    async fn only_get_callback_with_a_code_finishes_sign_in() {
        let (l, port) = listener().await;
        let cancel = Arc::new(tokio::sync::Notify::new());
        let c2 = cancel.clone();
        let waiting = tokio::spawn(async move { wait_for_code(l, &c2).await });
        // Noise is answered 404 and ignored.
        assert!(request(port, "GET /favicon.ico HTTP/1.1").await.starts_with("HTTP/1.1 404"));
        assert!(request(port, "POST /callback?code=evil HTTP/1.1").await.starts_with("HTTP/1.1 404"));
        let page = request(port, "GET /callback?code=abc123 HTTP/1.1").await;
        assert!(page.starts_with("HTTP/1.1 200") && page.contains("signed in"));
        assert_eq!(waiting.await.unwrap(), Ok("abc123".to_string()));
    }

    #[tokio::test]
    async fn google_failure_and_cancelled_callbacks_stop_waiting_with_plain_messages() {
        for (query, message) in [("error=google_failed", "Google sign-in didn't work. Please try again."), ("error=access_denied", "Sign-in was cancelled.")] {
            let (l, port) = listener().await;
            let cancel = Arc::new(tokio::sync::Notify::new());
            let c2 = cancel.clone();
            let waiting = tokio::spawn(async move { wait_for_code(l, &c2).await });
            request(port, &format!("GET /callback?{query} HTTP/1.1")).await;
            assert_eq!(waiting.await.unwrap(), Err(message.to_string()));
        }
    }

    #[tokio::test]
    async fn cancelling_stops_the_listener_immediately() {
        let (l, port) = listener().await;
        let cancel = Arc::new(tokio::sync::Notify::new());
        let c2 = cancel.clone();
        let waiting = tokio::spawn(async move { wait_for_code(l, &c2).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.notify_one();
        let r = tokio::time::timeout(Duration::from_secs(2), waiting).await.expect("returns at once").unwrap();
        assert_eq!(r, Err("Sign-in was cancelled.".to_string()));
        // The port is closed again.
        assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
    }
}

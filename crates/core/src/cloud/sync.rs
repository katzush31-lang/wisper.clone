//! The sync engine: one `sync()` call pushes what changed on this computer, pulls what changed
//! elsewhere, merges, and keeps API keys in step according to the chosen key storage.
//!
//! Safe to run again after any failure: every step is idempotent, and progress markers
//! ("cursors") live in the database's `meta` table and only move forward after a step succeeds.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::client::{CloudClient, UserInfo};
use super::merge::merge_sections;
use super::{hash_hex, now_ms, CloudError, CloudResult, KeyStore, PROVIDERS};
use crate::db::Db;
use crate::settings::{KeyStorage, RemoteSection, Settings, KEY_STORAGE_SECTION};

/// A dictation that finishes late (it was queued offline) has an old `created_at`, so the upload
/// looks back this far. The server ignores rows it already has; this run also remembers what it sent.
const STATS_OVERLAP_MS: i64 = 3 * 24 * 3600 * 1000;

const META_ACCOUNT: &str = "cloud_account";
const META_USER: &str = "cloud_user";
const META_MERGED: &str = "cloud_merge_done";
const META_CHOSEN: &str = "key_storage_chosen";
const META_LAST_OK: &str = "sync_last_ok";
const CURSORS: [&str; 4] = ["sync_items_pull", "sync_items_push", "sync_stats_push", "sync_stats_pull"];

fn key_hash_meta(provider: &str) -> String {
    format!("key_hash_{provider}")
}

/// Who is signed in on this computer (kept so the app can show it while offline).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountInfo {
    pub id: String,
    pub email: String,
    pub name: Option<String>,
    pub device_id: String,
}

/// What a successful sync changed on this computer. The app applies the settings parts to its
/// *current* settings (they may have been edited while the sync ran) and refreshes the UI.
#[derive(Debug, Default)]
pub struct SyncReport {
    /// True when this was the first merge after signing in on this computer.
    pub first_merge: bool,
    /// Settings sections taken from the account.
    pub remote_sections: BTreeMap<String, RemoteSection>,
    /// Change times to record for sections that had none.
    pub stamped: BTreeMap<String, i64>,
    pub items_changed: bool,
    pub keys_changed: bool,
    pub stats_changed: bool,
}

impl SyncReport {
    /// Applies the settings part of the report. Returns true if the settings changed and must be saved.
    pub fn apply_to_settings(&self, s: &mut Settings) -> bool {
        let before = s.clone();
        s.apply_remote_sections(&self.remote_sections);
        for (name, t) in &self.stamped {
            let e = s.section_times.entry(name.clone()).or_insert(0);
            *e = (*e).max(*t);
        }
        *s != before
    }
}

pub struct SyncEngine {
    client: Arc<CloudClient>,
    db: Arc<Db>,
    keys: Arc<dyn KeyStore>,
    data_dir: PathBuf,
    /// Only one sync (or key change) runs at a time.
    lock: tokio::sync::Mutex<()>,
    /// Stats rows already uploaded during this run (the look-back window would resend them).
    pushed_stats: std::sync::Mutex<HashSet<String>>,
}

fn dberr(e: rusqlite::Error) -> CloudError {
    CloudError::Server(format!("Couldn't read or save local data: {e}"))
}

impl SyncEngine {
    pub fn new(client: Arc<CloudClient>, db: Arc<Db>, keys: Arc<dyn KeyStore>, data_dir: PathBuf) -> Arc<SyncEngine> {
        Arc::new(SyncEngine { client, db, keys, data_dir, lock: Default::default(), pushed_stats: Default::default() })
    }

    pub fn client(&self) -> &Arc<CloudClient> {
        &self.client
    }

    fn meta(&self, key: &str) -> Option<String> {
        self.db.get_meta(key).ok().flatten()
    }

    fn meta_i64(&self, key: &str) -> i64 {
        self.meta(key).and_then(|v| v.parse().ok()).unwrap_or(0)
    }

    fn set_meta(&self, key: &str, value: &str) -> CloudResult<()> {
        self.db.set_meta(key, value).map_err(dberr)
    }

    /// The signed-in account, if any.
    pub fn account(&self) -> Option<AccountInfo> {
        if !self.client.is_signed_in() {
            return None;
        }
        serde_json::from_str(&self.meta(META_ACCOUNT)?).ok()
    }

    /// Whether the user has picked where keys live since signing in. Until then keys are neither
    /// uploaded nor downloaded.
    pub fn key_storage_chosen(&self) -> bool {
        self.meta(META_CHOSEN).as_deref() == Some("1")
    }

    /// When the last fully successful sync finished (unix ms).
    pub fn last_sync(&self) -> Option<i64> {
        Some(self.meta_i64(META_LAST_OK)).filter(|t| *t > 0)
    }

    /// True if API keys should be synced with the account right now.
    pub fn keys_sync_active(&self, settings: &Settings) -> bool {
        settings.account.key_storage == KeyStorage::Cloud && self.key_storage_chosen()
    }

    // ------------------------------------------------------------ sign in / out

    /// Finishes sign-in with the one-time code from the browser. The caller then runs `sync`.
    pub async fn complete_sign_in(&self, code: &str, verifier: &str, device_name: &str) -> CloudResult<AccountInfo> {
        let auth = self.client.exchange_code(code, verifier, device_name).await?;
        let UserInfo { id, email, name } = auth.user;
        self.record_sign_in(AccountInfo { id, email, name, device_id: auth.device_id })
    }

    /// Test seam (used by the end-to-end contract test against the real Worker): treats a refresh
    /// token already in the `TokenStore` as a finished sign-in. Asks the service who the token
    /// belongs to (`/v1/me`), then records it exactly as `complete_sign_in` does.
    pub async fn resume_sign_in(&self) -> CloudResult<AccountInfo> {
        let (user, device_id) = self.client.me().await?;
        self.record_sign_in(AccountInfo { id: user.id, email: user.email, name: user.name, device_id })
    }

    fn record_sign_in(&self, info: AccountInfo) -> CloudResult<AccountInfo> {
        // A different person than last time gets a fresh "first sign-in" merge (with backups).
        if self.meta(META_USER).as_deref() != Some(info.id.as_str()) {
            self.set_meta(META_MERGED, "0")?;
        }
        self.set_meta(META_USER, &info.id)?;
        self.set_meta(META_ACCOUNT, &serde_json::to_string(&info).unwrap())?;
        self.set_meta(META_CHOSEN, "0")?;
        for c in CURSORS {
            let _ = self.db.delete_meta(c);
        }
        self.pushed_stats.lock().unwrap().clear();
        Ok(info)
    }

    /// Everything this computer remembers about being signed in. Settings, dictionary and
    /// snippets stay. Cached API keys are removed only when `clear_keys` is true.
    fn local_sign_out(&self, clear_keys: bool) {
        if clear_keys {
            for p in PROVIDERS {
                self.keys.set(p, "");
            }
        }
        for key in [META_ACCOUNT, META_LAST_OK, "key_hash_openai", "key_hash_anthropic"].into_iter().chain(CURSORS) {
            let _ = self.db.delete_meta(key);
        }
        let _ = self.db.set_meta(META_CHOSEN, "0");
        let _ = self.db.remote_stats_clear();
        self.pushed_stats.lock().unwrap().clear();
    }

    /// Signs this computer out. Works offline (the account just keeps listing the device until
    /// it is removed). With cloud key storage the cached keys are deleted from this computer.
    pub async fn sign_out(&self, storage: KeyStorage) {
        let _ = self.client.logout().await;
        self.client.forget_tokens().await;
        self.local_sign_out(storage == KeyStorage::Cloud);
    }

    /// Deletes the account's cloud data, then signs out here.
    pub async fn delete_account(&self, storage: KeyStorage) -> CloudResult<()> {
        self.client.delete_account().await?;
        self.client.forget_tokens().await;
        self.local_sign_out(storage == KeyStorage::Cloud);
        Ok(())
    }

    // ------------------------------------------------------------ sync

    /// Runs one full sync using a snapshot of the current settings.
    pub async fn sync(&self, settings: &Settings) -> CloudResult<SyncReport> {
        let _g = self.lock.lock().await;
        let r = self.run(settings).await;
        if let Err(e) = &r {
            self.after_error(e, settings.account.key_storage);
        }
        r
    }

    /// If the service says this sign-in is over, become signed out here.
    fn after_error(&self, e: &CloudError, storage: KeyStorage) {
        if e.needs_sign_in() && !self.client.is_signed_in() {
            self.local_sign_out(*e == CloudError::Revoked && storage == KeyStorage::Cloud);
        }
    }

    async fn run(&self, settings: &Settings) -> CloudResult<SyncReport> {
        let start = now_ms();
        let first = self.meta(META_MERGED).as_deref() != Some("1");
        let mut report = SyncReport { first_merge: first, ..Default::default() };
        if first {
            self.backup_before_first_sync(settings)?;
        }

        // 1. Settings sections.
        let remote = self.client.get_settings().await?;
        let plan = merge_sections(settings, &remote, first, start);
        if !plan.push.is_empty() {
            self.client.put_settings(&plan.push).await?;
        }
        report.remote_sections = plan.apply;
        report.stamped = plan.stamp;

        // 2. Dictionary and snippets: pull and merge, then push what changed here.
        // Cursors are the server's own clock (`now` from its last reply), never ours.
        let pull_from = if first { 0 } else { self.meta_i64("sync_items_pull") };
        let pulled = self.client.get_items(pull_from).await?;
        report.items_changed = self.db.apply_remote_items(&pulled.items).map_err(dberr)? > 0;
        let changed = self.db.items_changed_since(self.meta_i64("sync_items_push")).map_err(dberr)?;
        for chunk in changed.chunks(500) {
            self.client.post_items(chunk).await?;
        }
        if pulled.now > 0 {
            self.set_meta("sync_items_pull", &pulled.now.to_string())?;
        }
        self.set_meta("sync_items_push", &(start - 1).to_string())?;

        // 3. Usage numbers.
        report.stats_changed = self.sync_stats().await?;

        // 4. API keys.
        let storage = report
            .remote_sections
            .get(KEY_STORAGE_SECTION)
            .and_then(|s| s.value.as_str())
            .and_then(KeyStorage::parse)
            .unwrap_or(settings.account.key_storage);
        if storage == KeyStorage::Cloud && self.key_storage_chosen() {
            report.keys_changed = self.sync_keys().await?;
        }

        self.set_meta(META_MERGED, "1")?;
        self.set_meta(META_LAST_OK, &now_ms().to_string())?;
        Ok(report)
    }

    /// Before anything is exchanged with the account for the first time: keep a copy of this
    /// computer's settings and database. Never overwrites an existing copy.
    fn backup_before_first_sync(&self, settings: &Settings) -> CloudResult<()> {
        let fail = |e: String| CloudError::Server(format!("Wisper couldn't make a safety copy of your data before syncing, so it didn't sync: {e}"));
        std::fs::create_dir_all(&self.data_dir).map_err(|e| fail(e.to_string()))?;
        let sp = self.data_dir.join("settings.before-signin.json");
        if !sp.exists() {
            settings.save(&sp).map_err(|e| fail(e.to_string()))?;
        }
        let dp = self.data_dir.join("wisper.before-signin.db");
        if !dp.exists() {
            self.db.backup_to(&dp).map_err(|e| fail(e.to_string()))?;
        }
        Ok(())
    }

    async fn sync_stats(&self) -> CloudResult<bool> {
        // Upload completed dictations (numbers only) we haven't uploaded yet.
        let push_cursor = self.meta_i64("sync_stats_push");
        let from = if push_cursor == 0 { 0 } else { (push_cursor - STATS_OVERLAP_MS).max(0) };
        let rows = self.db.stats_rows_since(from, 100_000).map_err(dberr)?;
        let fresh: Vec<_> = {
            let done = self.pushed_stats.lock().unwrap();
            rows.iter().filter(|r| !done.contains(&r.uid)).cloned().collect()
        };
        for chunk in fresh.chunks(1000) {
            self.client.post_stats(chunk).await?;
            let mut done = self.pushed_stats.lock().unwrap();
            done.extend(chunk.iter().map(|r| r.uid.clone()));
        }
        if let Some(max) = rows.iter().map(|r| r.created_at).max() {
            self.set_meta("sync_stats_push", &max.max(push_cursor).to_string())?;
        }
        // Download the other computers' numbers.
        let own = self.account().map(|a| a.device_id);
        let mut from = self.meta_i64("sync_stats_pull");
        let mut added = 0;
        loop {
            let page = self.client.get_stats(from).await?;
            let others: Vec<_> = page.rows.iter().filter(|r| r.device_id.is_none() || r.device_id != own).cloned().collect();
            added += self.db.remote_stats_add(&others).map_err(dberr)?;
            let last = page.rows.last().and_then(|r| r.server_at);
            match (page.more, last) {
                // Cut short: continue from the last row we saw (guard against not moving forward).
                (true, Some(t)) if t > from => {
                    from = t;
                    self.set_meta("sync_stats_pull", &from.to_string())?;
                }
                _ => {
                    if page.now > 0 {
                        self.set_meta("sync_stats_pull", &page.now.to_string())?;
                    }
                    break;
                }
            }
        }
        Ok(added > 0)
    }

    /// Reconciles this computer's keys with the account's (cloud storage only). Per provider:
    /// a key only the account has is copied here; a key only this computer has is uploaded; if
    /// both differ, whichever side changed since the last sync wins (the account on a tie).
    /// Returns true if a key on this computer changed.
    async fn sync_keys(&self) -> CloudResult<bool> {
        let remote = self.client.get_keys().await?;
        let mut changed = false;
        for p in PROVIDERS {
            let local = self.keys.get(p).filter(|k| !k.is_empty());
            let last = self.meta(&key_hash_meta(p));
            match (remote.get(p), local) {
                (Some(r), None) => {
                    self.keys.set(p, &r.key);
                    self.set_meta(&key_hash_meta(p), &hash_hex(&r.key))?;
                    changed = true;
                }
                (Some(r), Some(l)) if l == r.key => {
                    self.set_meta(&key_hash_meta(p), &hash_hex(&l))?;
                }
                (Some(r), Some(l)) => {
                    let edited_here = last.map(|h| h != hash_hex(&l)).unwrap_or(false);
                    if edited_here {
                        self.client.put_key(p, &l).await?;
                        self.set_meta(&key_hash_meta(p), &hash_hex(&l))?;
                    } else {
                        self.keys.set(p, &r.key);
                        self.set_meta(&key_hash_meta(p), &hash_hex(&r.key))?;
                        changed = true;
                    }
                }
                (None, Some(l)) => {
                    if last.as_deref() == Some(hash_hex(&l).as_str()) {
                        // We synced this exact key before and the account no longer has it: it
                        // was removed on another computer, so remove it here too.
                        self.keys.set(p, "");
                        let _ = self.db.delete_meta(&key_hash_meta(p));
                        changed = true;
                    } else {
                        self.client.put_key(p, &l).await?;
                        self.set_meta(&key_hash_meta(p), &hash_hex(&l))?;
                    }
                }
                (None, None) => {}
            }
        }
        Ok(changed)
    }

    // ------------------------------------------------------------ keys

    /// A key was just set (or cleared with an empty string) on this computer. Uploads it when
    /// keys are kept in the account; does nothing for local-only storage.
    pub async fn upload_key(&self, settings: &Settings, provider: &str, key: &str) -> CloudResult<()> {
        let _g = self.lock.lock().await;
        if !self.keys_sync_active(settings) || !PROVIDERS.contains(&provider) {
            return Ok(());
        }
        let r = self.client.put_key(provider, key).await;
        match &r {
            Ok(()) if key.is_empty() => {
                let _ = self.db.delete_meta(&key_hash_meta(provider));
            }
            Ok(()) => self.set_meta(&key_hash_meta(provider), &hash_hex(key))?,
            Err(e) => self.after_error(e, settings.account.key_storage),
        }
        r
    }

    /// The user chose where keys live (`from` is the current setting). Moving to `Local` after
    /// using the account removes the keys from the account (this computer keeps its copies);
    /// moving to `Cloud` after `Local` uploads this computer's keys. The very first choice after
    /// signing in only reconciles, so it can never wipe keys another computer uploaded.
    /// Returns true if a key on this computer changed.
    pub async fn set_key_storage(&self, from: KeyStorage, to: KeyStorage) -> CloudResult<bool> {
        let _g = self.lock.lock().await;
        let chosen_before = self.key_storage_chosen();
        let mut changed = false;
        let r: CloudResult<()> = async {
            match to {
                KeyStorage::Local => {
                    if chosen_before && from == KeyStorage::Cloud {
                        self.client.delete_keys().await?;
                        for p in PROVIDERS {
                            let _ = self.db.delete_meta(&key_hash_meta(p));
                        }
                    }
                }
                KeyStorage::Cloud if chosen_before && from == KeyStorage::Local => {
                    for p in PROVIDERS {
                        if let Some(k) = self.keys.get(p).filter(|k| !k.is_empty()) {
                            self.client.put_key(p, &k).await?;
                            self.set_meta(&key_hash_meta(p), &hash_hex(&k))?;
                        }
                    }
                }
                KeyStorage::Cloud => {
                    changed = self.sync_keys().await?;
                }
            }
            Ok(())
        }
        .await;
        match r {
            Ok(()) => {
                self.set_meta(META_CHOSEN, "1")?;
                Ok(changed)
            }
            Err(e) => {
                self.after_error(&e, from);
                Err(e)
            }
        }
    }
}

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
use crate::settings::{KeyStorage, RemoteSection, Settings};

/// A dictation that finishes late (it was queued offline) has an old `created_at`, so the upload
/// looks back this far. The server ignores rows it already has; this run also remembers what it sent.
const STATS_OVERLAP_MS: i64 = 3 * 24 * 3600 * 1000;

const META_ACCOUNT: &str = "cloud_account";
const META_USER: &str = "cloud_user";
const META_MERGED: &str = "cloud_merge_done";
const META_CHOSEN: &str = "key_storage_chosen";
const META_LAST_OK: &str = "sync_last_ok";
const META_REMOVED_SEEN: &str = "key_removed_seen";
/// Shown when the account's keys were removed from another computer.
pub const KEYS_REMOVED_NOTICE: &str = "Your API keys were removed from your account, so this computer now keeps its own copy.";
const META_MODE: &str = "key_storage_mode";
/// Why this computer is still asking where keys should live (`own_only` / `differ`); only
/// meaningful while the choice is not made.
const META_SITUATION: &str = "key_choice_situation";
/// Shown when this computer picked "Your account (synced)" by itself after signing in.
pub const KEYS_AUTO_SYNCED_NOTICE: &str = "Your API keys are now synced from your account.";
const META_BACKUP_FOR: &str = "backup_done_for";
const CURSORS: [&str; 3] = ["sync_items_pull", "sync_stats_push", "sync_stats_pull"];

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
    /// Set when this computer switched itself to local key storage (and why).
    pub notice: Option<String>,
    pub switched_to_local: bool,
    /// Set when this computer chose "Your account (synced)" by itself (nothing could be
    /// overwritten); the keys were pulled in the same sync.
    pub auto_chose_cloud: bool,
}

impl SyncReport {
    /// Applies the settings part of the report. Returns true if the settings changed and must be saved.
    pub fn apply_to_settings(&self, s: &mut Settings) -> bool {
        let before = s.clone();
        if self.first_merge {
            // First sign-in: the account's settings win outright, whatever this computer's change times say.
            for name in self.remote_sections.keys() {
                s.section_times.remove(name);
            }
        }
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

    /// While the choice is not made and this computer must ask: `own_only` (this computer has
    /// keys, the account has none: they would be uploaded) or `differ` (the account's keys would
    /// be used here, and this computer's different ones kept as a backup). None otherwise.
    pub fn key_choice_situation(&self) -> Option<String> {
        if self.key_storage_chosen() {
            return None;
        }
        self.meta(META_SITUATION)
    }

    /// When the last fully successful sync finished (unix ms).
    pub fn last_sync(&self) -> Option<i64> {
        Some(self.meta_i64(META_LAST_OK)).filter(|t| *t > 0)
    }

    /// Where this computer keeps API keys, once the user has chosen. This is the one place the
    /// engine reads the choice (always under its lock); the settings file only mirrors it for
    /// the UI. It is per computer and never synced.
    pub fn key_storage(&self) -> Option<KeyStorage> {
        if !self.key_storage_chosen() {
            return None;
        }
        // A recorded choice with no stored mode is treated as local: the safer default.
        Some(if self.meta(META_MODE).as_deref() == Some("cloud") { KeyStorage::Cloud } else { KeyStorage::Local })
    }

    /// True if API keys should be synced with the account right now.
    pub fn keys_sync_active(&self) -> bool {
        self.key_storage() == Some(KeyStorage::Cloud)
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
        let _ = self.db.delete_meta(META_SITUATION);
        for c in CURSORS {
            let _ = self.db.delete_meta(c);
        }
        self.pushed_stats.lock().unwrap().clear();
        Ok(info)
    }

    /// Removes this computer's copy of a key ONLY when it is certain the account holds exactly
    /// that key: key syncing is active here and the key matches what we last synced. Anything
    /// else (before the choice was made, an upload that never finished, a key edited since)
    /// might be the only copy, so it stays.
    fn clear_synced_keys(&self) {
        if !self.keys_sync_active() {
            return;
        }
        for p in PROVIDERS {
            if let Some(local) = self.keys.get(p).filter(|k| !k.is_empty()) {
                if self.meta(&key_hash_meta(p)).as_deref() == Some(hash_hex(&local).as_str()) {
                    self.keys.set(p, "");
                }
            }
        }
    }

    /// Everything this computer remembers about being signed in. Settings, dictionary,
    /// snippets and API keys stay (keys are handled separately by `clear_synced_keys`).
    fn local_sign_out(&self) {
        for key in [META_ACCOUNT, META_LAST_OK, META_REMOVED_SEEN, "key_hash_openai", "key_hash_anthropic"].into_iter().chain(CURSORS) {
            let _ = self.db.delete_meta(key);
        }
        let _ = self.db.set_meta(META_CHOSEN, "0");
        let _ = self.db.delete_meta(META_SITUATION);
        let _ = self.db.remote_stats_clear();
        self.pushed_stats.lock().unwrap().clear();
    }

    /// Signs this computer out. Works offline (the account just keeps listing the device until
    /// it is removed). Waits for any sync in progress so it can't restore anything afterwards.
    pub async fn sign_out(&self) {
        let _g = self.lock.lock().await;
        let _ = self.client.logout().await;
        self.client.forget_tokens().await;
        self.clear_synced_keys();
        self.local_sign_out();
    }

    /// Deletes the account's cloud data, then signs out here. This computer's own keys are kept:
    /// only the cloud copy is destroyed.
    pub async fn delete_account(&self) -> CloudResult<()> {
        let _g = self.lock.lock().await;
        self.client.delete_account().await?;
        self.client.forget_tokens().await;
        self.local_sign_out();
        Ok(())
    }

    // ------------------------------------------------------------ sync

    /// Runs one full sync using a snapshot of the current settings.
    pub async fn sync(&self, settings: &Settings) -> CloudResult<SyncReport> {
        let _g = self.lock.lock().await;
        let r = self.run(settings).await;
        if let Err(e) = &r {
            self.after_error(e);
        }
        r
    }

    /// If the service says this sign-in is over, become signed out here.
    fn after_error(&self, e: &CloudError) {
        if e.needs_sign_in() && !self.client.is_signed_in() {
            // Only "removed from the account" clears keys (and then only provably synced ones).
            // A replayed token or a deleted account keeps them.
            if *e == CloudError::Revoked {
                self.clear_synced_keys();
            }
            self.local_sign_out();
        }
    }

    async fn run(&self, settings: &Settings) -> CloudResult<SyncReport> {
        let start = now_ms();
        let first = self.meta(META_MERGED).as_deref() != Some("1");
        let mut report = SyncReport { first_merge: first, ..Default::default() };
        if first {
            self.backup_before_first_sync(settings)?;
            // A new account has none of this computer's words and snippets yet: send them all.
            self.db.mark_all_items_dirty().map_err(dberr)?;
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
        // Everything flagged as changed here and not yet uploaded (no clock involved).
        let dirty = self.db.items_dirty().map_err(dberr)?;
        for chunk in dirty.chunks(500) {
            self.client.post_items(chunk).await?;
            self.db.mark_items_pushed(chunk).map_err(dberr)?;
        }
        if pulled.now > 0 {
            self.set_meta("sync_items_pull", &pulled.now.to_string())?;
        }

        // 3. Usage numbers.
        report.stats_changed = self.sync_stats().await?;

        // 4. API keys (only when this computer keeps them in the account).
        if self.keys_sync_active() {
            report.keys_changed = self.sync_keys(&mut report).await?;
        } else if !self.key_storage_chosen() {
            self.decide_key_storage(&mut report).await?;
        }

        self.set_meta(META_MERGED, "1")?;
        self.set_meta(META_LAST_OK, &now_ms().to_string())?;
        Ok(report)
    }

    /// Before anything is exchanged with the account for the first time (and again whenever a
    /// different account signs in here): keep a copy of this computer's settings and database.
    /// The very first copy has the plain name; later ones are timestamped so nothing is overwritten.
    fn backup_before_first_sync(&self, settings: &Settings) -> CloudResult<()> {
        let fail = |e: String| CloudError::Server(format!("Wisper couldn't make a safety copy of your data before syncing, so it didn't sync: {e}"));
        let user = self.meta(META_USER).unwrap_or_default();
        if !user.is_empty() && self.meta(META_BACKUP_FOR).as_deref() == Some(user.as_str()) {
            return Ok(()); // already backed up for this account (this is a retry)
        }
        std::fs::create_dir_all(&self.data_dir).map_err(|e| fail(e.to_string()))?;
        let plain_settings = self.data_dir.join("settings.before-signin.json");
        let plain_db = self.data_dir.join("wisper.before-signin.db");
        let stem = if plain_settings.exists() || plain_db.exists() { format!("-{}", chrono::Local::now().format("%Y%m%d-%H%M%S%3f")) } else { String::new() };
        settings.save(&self.data_dir.join(format!("settings.before-signin{stem}.json"))).map_err(|e| fail(e.to_string()))?;
        self.db.backup_to(&self.data_dir.join(format!("wisper.before-signin{stem}.db"))).map_err(|e| fail(e.to_string()))?;
        self.set_meta(META_BACKUP_FOR, &user)?;
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

    /// Runs inside a sync (so under the lock) while the user hasn't chosen where keys live.
    /// When nothing on this computer can be overwritten or lost, picks "Your account (synced)"
    /// and pulls the keys right away:
    ///  - this computer has no keys of its own and the account has some, or
    ///  - every key this computer has is already the account's key.
    /// Otherwise the user is asked: `own_only` (own keys, account has none) or `differ`.
    /// Neither side having keys is left to the user too (nothing to sync yet).
    async fn decide_key_storage(&self, report: &mut SyncReport) -> CloudResult<()> {
        let remote = self.client.get_keys().await?;
        let account_has = PROVIDERS.iter().any(|p| remote.get(p).is_some());
        let local: Vec<(&str, String)> = PROVIDERS.iter().filter_map(|p| self.keys.get(p).filter(|k| !k.is_empty()).map(|k| (*p, k))).collect();
        let all_in_account = local.iter().all(|(p, k)| remote.get(p).map(|r| r.key == *k).unwrap_or(false));
        if account_has && (local.is_empty() || all_in_account) {
            let mut sub = SyncReport::default();
            let changed = self.sync_keys(&mut sub).await?;
            report.keys_changed = changed;
            if sub.switched_to_local {
                // The account's keys were removed on purpose: stay local and keep everything.
                report.notice = sub.notice;
                report.switched_to_local = true;
                self.set_meta(META_MODE, "local")?;
            } else {
                self.set_meta(META_MODE, "cloud")?;
                report.auto_chose_cloud = true;
                report.notice = Some(KEYS_AUTO_SYNCED_NOTICE.to_string());
            }
            self.set_meta(META_CHOSEN, "1")?;
            let _ = self.db.delete_meta(META_SITUATION);
        } else if !local.is_empty() {
            let s = if account_has { "differ" } else { "own_only" };
            self.set_meta(META_SITUATION, s)?;
        } else {
            let _ = self.db.delete_meta(META_SITUATION);
        }
        Ok(())
    }

    /// Reconciles this computer's keys with the account's (cloud storage only). Per provider:
    /// a key only the account has is copied here; a key only this computer has is uploaded; if
    /// both differ, whichever side changed since the last sync wins (the account on a tie).
    /// Returns true if a key on this computer changed.
    async fn sync_keys(&self, report: &mut SyncReport) -> CloudResult<bool> {
        let remote = self.client.get_keys().await?;
        if self.removal_is_new(&remote) {
            // The keys were removed from the account on purpose (not one provider): keep this
            // computer's keys exactly as they are and stop syncing them here.
            self.stop_syncing_keys();
            report.notice = Some(KEYS_REMOVED_NOTICE.to_string());
            report.switched_to_local = true;
            return Ok(false);
        }
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
                    let edited_here = last.as_ref().map(|h| *h != hash_hex(&l)).unwrap_or(false);
                    if edited_here {
                        self.client.put_key(p, &l).await?;
                        self.set_meta(&key_hash_meta(p), &hash_hex(&l))?;
                    } else {
                        // The account's key wins, but the replaced key is kept in the credential
                        // store as "previous-<provider>-api-key" so it is never irrecoverably lost.
                        // (Only the first time: a later rotation must not overwrite the stashed original.)
                        if last.is_none() {
                            self.keys.keep_previous(p, &l);
                        }
                        self.keys.set(p, &r.key);
                        self.set_meta(&key_hash_meta(p), &hash_hex(&r.key))?;
                        changed = true;
                    }
                }
                (None, Some(l)) => {
                    if last.as_deref() == Some(hash_hex(&l).as_str()) {
                        // We synced this exact key before and the account no longer has it: it
                        // was removed on another computer, so remove it here too.
                        self.keys.keep_previous(p, &l); // recoverable if that removal was a mistake
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
    /// keys are kept in the account; does nothing for local-only storage. The choice is read
    /// here, under the lock, so a switch to local that is in progress always wins.
    pub async fn upload_key(&self, provider: &str, key: &str) -> CloudResult<()> {
        let _g = self.lock.lock().await;
        if !self.keys_sync_active() || !PROVIDERS.contains(&provider) {
            return Ok(());
        }
        let r = self.client.put_key(provider, key).await;
        match &r {
            Ok(()) if key.is_empty() => {
                let _ = self.db.delete_meta(&key_hash_meta(provider));
            }
            Ok(()) => self.set_meta(&key_hash_meta(provider), &hash_hex(key))?,
            Err(e) => self.after_error(e),
        }
        r
    }

    /// Remembers the account's latest "all keys removed" time and says whether it is news to this
    /// computer. The first time we look (no record yet) is never news: the user just chose
    /// Cloud here, and uploading keys clears the mark.
    fn removal_is_new(&self, remote: &super::client::RemoteKeys) -> bool {
        let seen: Option<i64> = self.meta(META_REMOVED_SEEN).and_then(|v| v.parse().ok());
        let news = matches!((seen, remote.removed_at), (Some(s), Some(t)) if t > s);
        if !news {
            let now_seen = seen.unwrap_or(0).max(remote.removed_at.unwrap_or(0));
            let _ = self.db.set_meta(META_REMOVED_SEEN, &now_seen.to_string());
        }
        news
    }

    /// Records any removal marker as seen, even a newer one (the user just chose Cloud knowing
    /// the account's keys were removed), so the next sync doesn't switch back to local.
    fn acknowledge_removal(&self, remote: &super::client::RemoteKeys) {
        let seen: Option<i64> = self.meta(META_REMOVED_SEEN).and_then(|v| v.parse().ok());
        let now_seen = seen.unwrap_or(0).max(remote.removed_at.unwrap_or(0));
        let _ = self.db.set_meta(META_REMOVED_SEEN, &now_seen.to_string());
    }

    /// This computer keeps its own keys and no longer syncs them.
    fn stop_syncing_keys(&self) {
        let _ = self.db.set_meta(META_MODE, "local");
        for p in PROVIDERS {
            let _ = self.db.delete_meta(&key_hash_meta(p));
        }
    }

    /// Deletes every key the account holds, then switches THIS computer to local key storage
    /// (so it doesn't upload them again). Every computer keeps the keys it already has; other
    /// Cloud computers notice and switch to local too.
    pub async fn remove_cloud_keys(&self) -> CloudResult<()> {
        let _g = self.lock.lock().await;
        let r = self.client.delete_keys().await;
        match &r {
            Ok(()) => self.stop_syncing_keys(),
            Err(e) => self.after_error(e),
        }
        r
    }

    /// The user chose where keys live (per computer). Moving to `Local` just stops this computer
    /// syncing keys; moving to `Cloud` after `Local`
    /// uploads this computer's keys. The very first choice after signing in only reconciles, so
    /// it can never wipe keys another computer uploaded. The new choice is recorded here, under
    /// the lock and only once the account has been updated; the app mirrors it into settings.
    /// Returns true if a key on this computer changed.
    pub async fn set_key_storage(&self, to: KeyStorage) -> CloudResult<bool> {
        let _g = self.lock.lock().await;
        let from = self.key_storage();
        let mut changed = false;
        let mut sub = SyncReport::default();
        let r: CloudResult<()> = async {
            match to {
                KeyStorage::Local => {
                    // Only this computer stops syncing keys. Its copies stay, and so do the
                    // account's (other computers may use them); see `remove_cloud_keys`.
                    for p in PROVIDERS {
                        let _ = self.db.delete_meta(&key_hash_meta(p));
                    }
                }
                KeyStorage::Cloud if from == Some(KeyStorage::Local) => {
                    // Choosing Cloud again acknowledges any earlier removal; uploading clears it.
                    let remote = self.client.get_keys().await?;
                    self.acknowledge_removal(&remote);
                    for p in PROVIDERS {
                        if let Some(k) = self.keys.get(p).filter(|k| !k.is_empty()) {
                            self.client.put_key(p, &k).await?;
                            self.set_meta(&key_hash_meta(p), &hash_hex(&k))?;
                        }
                    }
                }
                KeyStorage::Cloud => {
                    changed = self.sync_keys(&mut sub).await?;
                }
            }
            Ok(())
        }
        .await;
        match r {
            Ok(()) => {
                // (If the account's keys turned out to have been removed, we stayed local.)
                let mode = if sub.switched_to_local { KeyStorage::Local } else { to };
                self.set_meta(META_MODE, mode.as_str())?;
                self.set_meta(META_CHOSEN, "1")?;
                Ok(changed)
            }
            Err(e) => {
                self.after_error(&e);
                Err(e)
            }
        }
    }
}

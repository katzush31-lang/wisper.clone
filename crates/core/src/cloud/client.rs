//! HTTP client for the Wisper account service (the Cloudflare Worker). Handles the sign-in
//! tokens: the short-lived access token lives in memory only; the long-lived refresh token is
//! kept through a `TokenStore`. A 401 triggers exactly one refresh and one retry.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{CloudError, CloudResult, TokenStore};
use crate::db::{StatRow, SyncItem};
use crate::settings::RemoteSection;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: String,
    pub email: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthResponse {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub expires_in: i64,
    pub user: UserInfo,
    pub device_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    #[serde(alias = "created_at", default)]
    pub created_at: Option<i64>,
    #[serde(alias = "last_seen", default)]
    pub last_seen: Option<i64>,
    #[serde(default)]
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RemoteKey {
    pub key: String,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RemoteKeys {
    #[serde(default)]
    pub openai: Option<RemoteKey>,
    #[serde(default)]
    pub anthropic: Option<RemoteKey>,
}

impl RemoteKeys {
    pub fn get(&self, provider: &str) -> Option<&RemoteKey> {
        match provider {
            "openai" => self.openai.as_ref(),
            _ => self.anthropic.as_ref(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ItemsResponse {
    #[serde(default)]
    pub items: Vec<SyncItem>,
    #[serde(default)]
    pub now: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StatsResponse {
    #[serde(default)]
    pub rows: Vec<StatRow>,
    #[serde(default)]
    pub now: i64,
    /// True when the reply was cut short: ask again from the last row's `server_at`.
    #[serde(default)]
    pub more: bool,
}

#[derive(Default)]
struct Tokens {
    access: Option<String>,
    /// Newest refresh token, kept in memory too in case saving it fails (each one is single use).
    refresh: Option<String>,
}

pub struct CloudClient {
    base: String,
    http: reqwest::Client,
    store: Arc<dyn TokenStore>,
    tokens: tokio::sync::Mutex<Tokens>,
}

/// Reads the service's `{"error": code, "message": text}` reply.
async fn error_from(resp: reqwest::Response) -> CloudError {
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let code = body.get("error").and_then(|c| c.as_str()).unwrap_or("");
    let message = body.get("message").and_then(|m| m.as_str()).unwrap_or("");
    if status.as_u16() == 401 {
        if code != "revoked" {
            return CloudError::Unauthorized;
        }
        // "reuse" (a replayed refresh token) and "account_deleted" keep local keys; "removed",
        // or no reason from an older server, means this computer was signed out of the account.
        return match body.get("reason").and_then(|r| r.as_str()) {
            Some("reuse") | Some("account_deleted") => CloudError::RevokedKeepKeys,
            _ => CloudError::Revoked,
        };
    }
    if !message.is_empty() {
        return CloudError::Server(message.to_string());
    }
    if status.is_server_error() {
        CloudError::Server(format!("The Wisper account service had a problem ({}). Try again in a few minutes.", status.as_u16()))
    } else {
        CloudError::Server(format!("The Wisper account service didn't accept that request ({}).", status.as_u16()))
    }
}

fn bad_reply<E>(_: E) -> CloudError {
    CloudError::Server("The Wisper account service sent a reply Wisper couldn't understand. Try again in a few minutes.".into())
}

impl CloudClient {
    pub fn new(base_url: &str, store: Arc<dyn TokenStore>) -> Arc<CloudClient> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("http client");
        Arc::new(CloudClient { base: base_url.trim_end_matches('/').to_string(), http, store, tokens: Default::default() })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// The URL to open in the browser to start Google sign-in.
    pub fn login_url(&self, challenge: &str, port: u16) -> String {
        format!("{}/auth/google/start?challenge={}&port={}", self.base, challenge, port)
    }

    /// True while this computer holds a sign-in (a refresh token).
    pub fn is_signed_in(&self) -> bool {
        self.store.load().is_some()
    }

    /// Forgets the sign-in on this computer (the server is not told).
    pub async fn forget_tokens(&self) {
        let mut t = self.tokens.lock().await;
        t.access = None;
        t.refresh = None;
        self.store.clear();
    }

    // ------------------------------------------------------------ auth

    /// Finishes Google sign-in: trades the one-time code (plus the PKCE verifier) for tokens.
    pub async fn exchange_code(&self, code: &str, verifier: &str, device_name: &str) -> CloudResult<AuthResponse> {
        let resp = self
            .http
            .post(format!("{}/auth/token", self.base))
            .json(&json!({"code": code, "verifier": verifier, "device_name": device_name}))
            .send()
            .await
            .map_err(|_| CloudError::Offline)?;
        if !resp.status().is_success() {
            return Err(error_from(resp).await);
        }
        let auth: AuthResponse = resp.json().await.map_err(bad_reply)?;
        let mut t = self.tokens.lock().await;
        t.access = Some(auth.access_token.clone());
        t.refresh = Some(auth.refresh_token.clone());
        if let Err(e) = self.store.save(&auth.refresh_token) {
            tracing::warn!("couldn't save the sign-in token: {e}");
        }
        Ok(auth)
    }

    /// Exchanges the refresh token for new tokens. The caller holds the token lock, so two
    /// requests can never present the same (single-use) refresh token.
    async fn refresh_locked(&self, t: &mut Tokens) -> CloudResult<String> {
        let Some(refresh) = t.refresh.clone().or_else(|| self.store.load()) else {
            return Err(CloudError::Unauthorized);
        };
        let resp = self
            .http
            .post(format!("{}/auth/refresh", self.base))
            .json(&json!({"refresh_token": refresh}))
            .send()
            .await
            .map_err(|_| CloudError::Offline)?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let err = error_from(resp).await;
            if status == 401 {
                // Signed out for good: forget everything.
                t.access = None;
                t.refresh = None;
                self.store.clear();
            }
            return Err(err);
        }
        let auth: AuthResponse = resp.json().await.map_err(bad_reply)?;
        t.refresh = Some(auth.refresh_token.clone());
        if let Err(e) = self.store.save(&auth.refresh_token) {
            tracing::warn!("couldn't save the new sign-in token: {e}");
        }
        t.access = Some(auth.access_token.clone());
        Ok(auth.access_token)
    }

    async fn access_token(&self) -> CloudResult<String> {
        let mut t = self.tokens.lock().await;
        if let Some(a) = &t.access {
            return Ok(a.clone());
        }
        self.refresh_locked(&mut t).await
    }

    /// Called after a 401 with `failed`. If another request already refreshed, just use that.
    async fn refresh_after(&self, failed: &str) -> CloudResult<String> {
        let mut t = self.tokens.lock().await;
        if let Some(a) = &t.access {
            if a != failed {
                return Ok(a.clone());
            }
        }
        t.access = None;
        self.refresh_locked(&mut t).await
    }

    /// One authenticated call; refreshes the access token once if the service says 401.
    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> CloudResult<reqwest::Response> {
        let mut token = self.access_token().await?;
        let mut retried = false;
        loop {
            let mut rb = self.http.request(method.clone(), format!("{}{}", self.base, path)).bearer_auth(&token);
            if let Some(b) = &body {
                rb = rb.json(b);
            }
            let resp = rb.send().await.map_err(|_| CloudError::Offline)?;
            if resp.status().as_u16() == 401 && !retried {
                retried = true;
                token = self.refresh_after(&token).await?;
                continue;
            }
            if resp.status().is_success() {
                return Ok(resp);
            }
            return Err(error_from(resp).await);
        }
    }

    async fn json<T: for<'de> Deserialize<'de>>(&self, method: Method, path: &str, body: Option<Value>) -> CloudResult<T> {
        self.call(method, path, body).await?.json::<T>().await.map_err(bad_reply)
    }

    // ------------------------------------------------------------ account

    /// Tells the service to sign this computer out. Callers ignore failures (e.g. offline).
    pub async fn logout(&self) -> CloudResult<()> {
        self.call(Method::POST, "/auth/logout", None).await.map(|_| ())
    }

    pub async fn me(&self) -> CloudResult<(UserInfo, String)> {
        let v: Value = self.json(Method::GET, "/v1/me", None).await?;
        let user: UserInfo = serde_json::from_value(v.get("user").cloned().unwrap_or(Value::Null)).map_err(bad_reply)?;
        Ok((user, v.get("device_id").and_then(|d| d.as_str()).unwrap_or("").to_string()))
    }

    pub async fn devices(&self) -> CloudResult<Vec<Device>> {
        self.json(Method::GET, "/v1/devices", None).await
    }

    pub async fn remove_device(&self, id: &str) -> CloudResult<()> {
        self.call(Method::DELETE, &format!("/v1/devices/{id}"), None).await.map(|_| ())
    }

    pub async fn delete_account(&self) -> CloudResult<()> {
        self.call(Method::DELETE, "/v1/account", None).await.map(|_| ())
    }

    // ------------------------------------------------------------ keys

    pub async fn get_keys(&self) -> CloudResult<RemoteKeys> {
        self.json(Method::GET, "/v1/keys", None).await
    }

    /// An empty key deletes it from the account.
    pub async fn put_key(&self, provider: &str, key: &str) -> CloudResult<()> {
        self.call(Method::PUT, &format!("/v1/keys/{provider}"), Some(json!({"key": key}))).await.map(|_| ())
    }

    pub async fn delete_keys(&self) -> CloudResult<()> {
        self.call(Method::DELETE, "/v1/keys", None).await.map(|_| ())
    }

    // ------------------------------------------------------------ settings, items, stats

    pub async fn get_settings(&self) -> CloudResult<BTreeMap<String, RemoteSection>> {
        #[derive(Deserialize)]
        struct R {
            #[serde(default)]
            sections: BTreeMap<String, RemoteSection>,
        }
        Ok(self.json::<R>(Method::GET, "/v1/settings", None).await?.sections)
    }

    pub async fn put_settings(&self, sections: &BTreeMap<String, RemoteSection>) -> CloudResult<()> {
        self.call(Method::PUT, "/v1/settings", Some(json!({"sections": sections}))).await.map(|_| ())
    }

    pub async fn get_items(&self, since: i64) -> CloudResult<ItemsResponse> {
        self.json(Method::GET, &format!("/v1/items?since={since}"), None).await
    }

    /// At most 500 per call (the caller splits).
    pub async fn post_items(&self, items: &[SyncItem]) -> CloudResult<()> {
        self.call(Method::POST, "/v1/items", Some(json!({"items": items}))).await.map(|_| ())
    }

    /// At most 1000 per call (the caller splits).
    pub async fn post_stats(&self, rows: &[StatRow]) -> CloudResult<()> {
        self.call(Method::POST, "/v1/stats", Some(json!({"rows": rows}))).await.map(|_| ())
    }

    /// `since` is a server-time cursor (the `now` of an earlier reply).
    pub async fn get_stats(&self, since: i64) -> CloudResult<StatsResponse> {
        self.json(Method::GET, &format!("/v1/stats?since={since}"), None).await
    }
}

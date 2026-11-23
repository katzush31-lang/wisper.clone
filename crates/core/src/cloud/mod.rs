//! Optional Wisper account: Google sign-in, API keys kept in the account, and sync of settings,
//! dictionary, snippets and usage numbers between computers. Everything here is pure (no
//! Windows code) so it is tested on Linux against a fake server. See docs/ACCOUNTS.md.

pub mod client;
pub mod merge;
pub mod sync;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Mutex;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};

pub use client::{AuthResponse, CloudClient, Device, UserInfo};
pub use sync::{SyncEngine, SyncReport};

/// The API-key providers the account can hold.
pub const PROVIDERS: [&str; 2] = ["openai", "anthropic"];

/// Why an account call failed, with a message fit to show the user.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CloudError {
    #[error("Can't reach the Wisper account service. Check your internet connection; Wisper will try again later.")]
    Offline,
    /// The sign-in expired or is invalid.
    #[error("Please sign in again.")]
    Unauthorized,
    /// This computer was removed from the account.
    #[error("This computer was signed out of your account. Please sign in again.")]
    Revoked,
    /// Signed out because the sign-in was replayed (maybe a dropped reply) or the account was
    /// deleted elsewhere. Local API keys must be kept: the cloud copy may be gone.
    #[error("Please sign in again.")]
    RevokedKeepKeys,
    #[error("{0}")]
    Server(String),
}

impl CloudError {
    pub fn message(&self) -> String {
        self.to_string()
    }

    /// True when the user has to sign in again (and the app should show itself as signed out).
    pub fn needs_sign_in(&self) -> bool {
        matches!(self, CloudError::Unauthorized | CloudError::Revoked | CloudError::RevokedKeepKeys)
    }
}

pub type CloudResult<T> = Result<T, CloudError>;

/// Where the long-lived refresh token is kept (Windows Credential Manager in the app, memory in tests).
pub trait TokenStore: Send + Sync {
    fn load(&self) -> Option<String>;
    fn save(&self, token: &str) -> Result<(), String>;
    fn clear(&self);
}

/// This computer's own copy of the API keys (the same store dictation reads from).
/// An empty `key` removes the key.
pub trait KeyStore: Send + Sync {
    fn get(&self, provider: &str) -> Option<String>;
    fn set(&self, provider: &str, key: &str);
}

#[derive(Default)]
pub struct MemoryTokenStore(Mutex<Option<String>>);

impl MemoryTokenStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TokenStore for MemoryTokenStore {
    fn load(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }
    fn save(&self, token: &str) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(token.to_string());
        Ok(())
    }
    fn clear(&self) {
        *self.0.lock().unwrap() = None;
    }
}

#[derive(Default)]
pub struct MemoryKeyStore(Mutex<HashMap<String, String>>);

impl MemoryKeyStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeyStore for MemoryKeyStore {
    fn get(&self, provider: &str) -> Option<String> {
        self.0.lock().unwrap().get(provider).cloned()
    }
    fn set(&self, provider: &str, key: &str) {
        let mut m = self.0.lock().unwrap();
        if key.is_empty() {
            m.remove(provider);
        } else {
            m.insert(provider.to_string(), key.to_string());
        }
    }
}

// ---------------------------------------------------------------- sign-in helpers (PKCE)

/// A fresh random PKCE verifier: 64 characters from the allowed set, from the OS random source.
pub fn new_verifier() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// `base64url(SHA-256(verifier))`, no padding.
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// What the one-shot sign-in listener received.
#[derive(Debug, Clone, PartialEq)]
pub enum Callback {
    /// `GET /callback?code=…`
    Code(String),
    /// `GET /callback?error=…`
    Error(String),
    /// Anything else (favicon requests, other paths, other methods): answer 404 and keep waiting.
    Other,
}

/// Reads the first line of an HTTP request ("GET /callback?code=abc HTTP/1.1").
pub fn parse_callback(request_line: &str) -> Callback {
    let mut parts = request_line.split_whitespace();
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else { return Callback::Other };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/callback" {
        return Callback::Other;
    }
    let mut code = None;
    let mut error = None;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "code" if !v.is_empty() => code = Some(percent_decode(v)),
            "error" => error = Some(percent_decode(v)),
            _ => {}
        }
    }
    match (code, error) {
        (Some(c), _) => Callback::Code(c),
        (None, Some(e)) => Callback::Error(e),
        _ => Callback::Other,
    }
}

fn hex_val(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// Works on bytes only, so odd input (multi-byte characters right after a `%`) can't panic.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() && hex_val(b[i + 1]).is_some() && hex_val(b[i + 2]).is_some() => {
                out.push(hex_val(b[i + 1]).unwrap() * 16 + hex_val(b[i + 2]).unwrap());
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Plain-English text for an `error=` value the browser brings back to the loopback.
pub fn callback_error_message(code: &str) -> String {
    match code {
        "access_denied" => "Sign-in was cancelled.".to_string(),
        "google_failed" => "Google sign-in didn't work. Please try again.".to_string(),
        other => format!("Google sign-in didn't complete ({other}). Please try again."),
    }
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// SHA-256 of a key as hex. Used to remember "what we last synced" without storing the key in the database.
pub(crate) fn hash_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_the_rfc_example() {
        assert_eq!(pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn verifiers_are_long_unique_and_url_safe() {
        let (a, b) = (new_verifier(), new_verifier());
        assert_ne!(a, b);
        assert!((43..=128).contains(&a.len()));
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn callback_errors_have_plain_english_messages() {
        assert_eq!(callback_error_message("access_denied"), "Sign-in was cancelled.");
        assert_eq!(callback_error_message("google_failed"), "Google sign-in didn't work. Please try again.");
        assert_eq!(parse_callback("GET /callback?error=google_failed HTTP/1.1"), Callback::Error("google_failed".into()));
    }

    #[test]
    fn callback_parsing_accepts_only_get_callback() {
        assert_eq!(parse_callback("GET /callback?code=abc123 HTTP/1.1"), Callback::Code("abc123".into()));
        assert_eq!(parse_callback("GET /callback?x=1&code=a%2Db HTTP/1.1"), Callback::Code("a-b".into()));
        assert_eq!(parse_callback("GET /callback?error=access_denied HTTP/1.1"), Callback::Error("access_denied".into()));
        assert_eq!(parse_callback("GET /callback HTTP/1.1"), Callback::Other);
        assert_eq!(parse_callback("GET /favicon.ico HTTP/1.1"), Callback::Other);
        assert_eq!(parse_callback("POST /callback?code=abc HTTP/1.1"), Callback::Other);
        assert_eq!(parse_callback("GET /callbackx?code=abc HTTP/1.1"), Callback::Other);
        assert_eq!(parse_callback(""), Callback::Other);
    }

    #[test]
    fn percent_decoding_handles_edges() {
        assert_eq!(percent_decode("a%2"), "a%2");
        // Multi-byte characters right after a % must not panic (they used to slice mid-character).
        assert_eq!(percent_decode("%a\u{e9}"), "%a\u{e9}");
        assert_eq!(percent_decode("%\u{e9}9"), "%\u{e9}9");
        assert_eq!(percent_decode("%c3%a9"), "\u{e9}");
        assert_eq!(percent_decode("%41%"), "A%");
        assert_eq!(percent_decode("a+b"), "a b");
    }
}

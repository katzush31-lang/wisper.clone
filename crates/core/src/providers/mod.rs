//! AI providers behind two small interfaces, so a provider or model can be swapped without
//! touching the rest of the app.

pub mod anthropic;
pub mod models;
pub mod openai;
pub mod realtime;

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    /// Key missing, wrong or revoked.
    Auth,
    /// Out of credit / billing problem.
    Quota,
    /// Too many requests; retry later.
    RateLimited,
    /// Provider is having trouble (5xx, overloaded).
    Server,
    /// Couldn't reach the provider (offline, DNS, TLS, connection reset).
    Network,
    Timeout,
    /// The request was rejected as invalid (a bug, or an unsupported model/setting).
    BadRequest,
    /// The model declined to answer.
    Refused,
    /// The provider answered with something we couldn't understand.
    BadResponse,
}

#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[error("{provider}: {message}")]
#[serde(rename_all = "camelCase")]
pub struct ProviderError {
    pub kind: ErrorKind,
    pub provider: String,
    /// Technical detail (for logs and the "details" view).
    pub message: String,
    /// Seconds the provider asked us to wait, if any.
    pub retry_after: Option<u64>,
}

impl ProviderError {
    pub fn new(kind: ErrorKind, provider: &str, message: impl Into<String>) -> Self {
        Self { kind, provider: provider.into(), message: message.into(), retry_after: None }
    }

    /// Temporary problems are retried automatically; the rest need the user.
    pub fn is_retryable(&self) -> bool {
        matches!(self.kind, ErrorKind::RateLimited | ErrorKind::Server | ErrorKind::Network | ErrorKind::Timeout)
    }

    pub fn is_offline(&self) -> bool {
        matches!(self.kind, ErrorKind::Network | ErrorKind::Timeout)
    }

    /// Plain-English explanation with what to do.
    pub fn user_message(&self) -> String {
        let who = &self.provider;
        match self.kind {
            ErrorKind::Auth => format!("Your {who} API key isn't working. Open Settings → AI providers, paste a valid key and press Test."),
            ErrorKind::Quota => format!("Your {who} account is out of credit. Add credit on the {who} billing page; your dictation is saved and can be re-processed from History."),
            ErrorKind::RateLimited => format!("{who} is busy (too many requests). Wisper will retry automatically."),
            ErrorKind::Server => format!("{who} is having problems right now. Wisper will retry automatically."),
            ErrorKind::Network => "No internet connection. Your dictation is saved and will be processed when you're back online.".into(),
            ErrorKind::Timeout => format!("{who} took too long to answer. Wisper will retry automatically."),
            ErrorKind::BadRequest => format!("{who} rejected the request. Try another model in Settings → AI providers. Details: {}", self.message),
            ErrorKind::Refused => format!("{who} declined to process this text, so it was inserted without AI cleanup."),
            ErrorKind::BadResponse => format!("{who} sent an unexpected reply. Your dictation is saved; try again from History."),
        }
    }
}

/// Classifies a transport-level error from reqwest.
pub(crate) fn from_reqwest(provider: &str, e: reqwest::Error) -> ProviderError {
    let kind = if e.is_timeout() {
        ErrorKind::Timeout
    } else if e.is_connect() || e.is_request() || e.is_body() {
        ErrorKind::Network
    } else if e.is_decode() {
        ErrorKind::BadResponse
    } else {
        ErrorKind::Network
    };
    ProviderError::new(kind, provider, e.to_string())
}

pub(crate) fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map(|s| s.ceil() as u64)
}

#[derive(Debug, Clone)]
pub struct TranscribeRequest {
    pub audio: Vec<u8>,
    pub filename: String,
    pub mime: String,
    pub model: String,
    pub prompt: Option<String>,
    pub keywords: Vec<String>,
    /// Expected languages (ISO codes); empty = auto-detect.
    pub languages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    pub text: String,
    pub languages: Vec<String>,
}

#[async_trait]
pub trait Transcriber: Send + Sync {
    async fn transcribe(&self, req: TranscribeRequest) -> Result<Transcript, ProviderError>;
}

#[derive(Debug, Clone)]
pub struct SystemBlock {
    pub text: String,
    /// Mark the end of this block as a prompt-cache breakpoint.
    pub cache: bool,
}

#[derive(Debug, Clone)]
pub struct TextRequest {
    pub model: String,
    pub system: Vec<SystemBlock>,
    pub user: String,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextResponse {
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
}

#[async_trait]
pub trait TextModel: Send + Sync {
    async fn complete(&self, req: TextRequest) -> Result<TextResponse, ProviderError>;
    fn provider_name(&self) -> &'static str;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub note: String,
    pub recommended: bool,
}

/// One HTTP client for the whole app, so every request (and the pre-warm) shares the same
/// connection pool and an already-open TLS connection is reused. Per-request timeouts are set
/// where the request is built.
pub(crate) fn shared_client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .pool_idle_timeout(Duration::from_secs(90))
                .tcp_keepalive(Duration::from_secs(30))
                .user_agent(concat!("Wisper/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("http client")
        })
        .clone()
}

/// How often the same host is pre-warmed at most.
const PREWARM_EVERY: Duration = Duration::from_secs(60);

/// Whether `host` is due for a pre-warm now (and marks it done). Pure bookkeeping, tested below.
fn prewarm_due(done: &mut std::collections::HashMap<String, std::time::Instant>, host: &str, now: std::time::Instant) -> bool {
    match done.get(host) {
        Some(t) if now.duration_since(*t) < PREWARM_EVERY => false,
        _ => {
            done.insert(host.to_string(), now);
            true
        }
    }
}

/// Opens (or keeps alive) the connection to an API host before it is needed, so the real call
/// after recording skips the TCP + TLS handshake. Sends a bodyless HEAD to the site root with
/// no API key: it costs nothing, uses no quota and the answer (usually 404/401) is ignored.
/// Never fails and never takes longer than a few seconds; skipped if done in the last minute.
pub async fn prewarm(base_url: &str) {
    static DONE: std::sync::Mutex<Option<std::collections::HashMap<String, std::time::Instant>>> = std::sync::Mutex::new(None);
    let Ok(url) = reqwest::Url::parse(base_url) else { return };
    let Some(host) = url.host_str().map(str::to_string) else { return };
    let due = {
        let Ok(mut g) = DONE.lock() else { return };
        prewarm_due(g.get_or_insert_with(Default::default), &host, std::time::Instant::now())
    };
    if !due {
        return;
    }
    let root = format!("{}://{}/", url.scheme(), url.authority());
    let started = std::time::Instant::now();
    let r = shared_client().head(root).timeout(Duration::from_secs(5)).send().await;
    tracing::debug!("pre-warm {host}: {:?} in {} ms", r.map(|r| r.status()).map_err(|e| e.to_string()), started.elapsed().as_millis());
}

/// Pre-warms every host a dictation will talk to: transcription (OpenAI) and the chosen
/// cleanup provider. Only hosts whose key is set are touched.
pub async fn prewarm_for(openai_key: bool, anthropic_key: bool, cleanup: crate::settings::CleanupProvider) {
    let cleanup_url = match cleanup {
        crate::settings::CleanupProvider::Anthropic if anthropic_key => Some(anthropic::BASE),
        crate::settings::CleanupProvider::OpenAi if openai_key => Some(openai::BASE),
        _ => None,
    };
    let a = async {
        if openai_key {
            prewarm(openai::BASE).await
        }
    };
    let b = async {
        if let Some(u) = cleanup_url {
            prewarm(u).await
        }
    };
    tokio::join!(a, b);
}

/// Retry policy for temporary errors: quick retries with backoff, honouring Retry-After.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub attempts: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { attempts: 4, base_delay_ms: 600, max_delay_ms: 8_000 }
    }
}

impl RetryPolicy {
    pub fn delay_for(&self, attempt: u32, err: &ProviderError) -> Duration {
        if let Some(s) = err.retry_after {
            return Duration::from_millis((s * 1000).min(self.max_delay_ms * 2));
        }
        let exp = self.base_delay_ms.saturating_mul(1u64 << attempt.min(10));
        Duration::from_millis(exp.min(self.max_delay_ms))
    }

    /// Runs `f` until it succeeds, a non-retryable error occurs, or attempts run out.
    pub async fn run<T, F, Fut>(&self, mut f: F) -> Result<T, ProviderError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, ProviderError>>,
    {
        let mut attempt = 0;
        loop {
            match f().await {
                Ok(v) => return Ok(v),
                Err(e) if e.is_retryable() && attempt + 1 < self.attempts => {
                    let d = self.delay_for(attempt, &e);
                    tracing::info!("retrying after {:?}: {}", d, e);
                    tokio::time::sleep(d).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test(start_paused = true)]
    async fn retries_temporary_errors_then_succeeds() {
        let calls = Arc::new(AtomicU32::new(0));
        let p = RetryPolicy::default();
        let c = calls.clone();
        let r: Result<u32, _> = p
            .run(|| {
                let c = c.clone();
                async move {
                    let n = c.fetch_add(1, Ordering::SeqCst);
                    if n < 2 {
                        Err(ProviderError::new(ErrorKind::Server, "OpenAI", "503"))
                    } else {
                        Ok(7)
                    }
                }
            })
            .await;
        assert_eq!(r.unwrap(), 7);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn does_not_retry_auth_errors() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let r: Result<(), _> = RetryPolicy::default()
            .run(|| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err(ProviderError::new(ErrorKind::Auth, "Claude", "401"))
                }
            })
            .await;
        assert_eq!(r.unwrap_err().kind, ErrorKind::Auth);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn prewarm_is_throttled_per_host() {
        let mut done = std::collections::HashMap::new();
        let t0 = std::time::Instant::now();
        assert!(prewarm_due(&mut done, "api.openai.com", t0));
        assert!(!prewarm_due(&mut done, "api.openai.com", t0 + Duration::from_secs(59)));
        assert!(prewarm_due(&mut done, "api.anthropic.com", t0 + Duration::from_secs(1)));
        assert!(prewarm_due(&mut done, "api.openai.com", t0 + Duration::from_secs(61)));
    }

    #[test]
    fn messages_are_plain_english() {
        let e = ProviderError::new(ErrorKind::Network, "OpenAI", "dns error");
        assert!(e.user_message().contains("No internet"));
        assert!(e.is_offline());
        let e = ProviderError::new(ErrorKind::Auth, "Claude", "401");
        assert!(e.user_message().contains("Claude API key"));
        assert!(!e.is_retryable());
    }

    #[test]
    fn backoff_grows_and_honours_retry_after() {
        let p = RetryPolicy::default();
        let e = ProviderError::new(ErrorKind::Server, "x", "y");
        assert!(p.delay_for(1, &e) > p.delay_for(0, &e));
        assert_eq!(p.delay_for(9, &e), Duration::from_millis(8000));
        let mut e2 = e.clone();
        e2.retry_after = Some(3);
        assert_eq!(p.delay_for(0, &e2), Duration::from_secs(3));
    }
}

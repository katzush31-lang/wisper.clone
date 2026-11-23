//! OpenAI Realtime transcription over a WebSocket: the wire protocol only (connect, the
//! session set-up message, audio/commit messages and the server events we care about).
//! The live-transcription logic built on top of it lives in `crate::live`.
//!
//! Protocol (GA Realtime API, transcription session):
//! - connect `wss://api.openai.com/v1/realtime?intent=transcription` with `Authorization: Bearer`;
//! - send `session.update` with `session.type = "transcription"`, 24 kHz PCM input, the
//!   transcription model/prompt/keywords/languages and `turn_detection: null` (Wisper decides
//!   where each piece ends, with its own pause detection);
//! - stream `input_audio_buffer.append` (base64 16-bit mono 24 kHz) and send
//!   `input_audio_buffer.commit` at each piece boundary (or `input_audio_buffer.clear` for a
//!   silent piece);
//! - the server answers each commit with `input_audio_buffer.committed` (in commit order) and,
//!   later and possibly out of order, `conversation.item.input_audio_transcription.completed`
//!   or `.failed` for that item.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, http};
use tokio_tungstenite::WebSocketStream;

use super::{ErrorKind, ProviderError};

pub const NAME: &str = "OpenAI";
pub const URL: &str = "wss://api.openai.com/v1/realtime?intent=transcription";
/// The only PCM rate the Realtime API accepts.
pub const SAMPLE_RATE: u32 = 24_000;

/// Anything we can run a WebSocket over (plain TCP in tests, TLS for the real API).
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

pub type Socket = WebSocketStream<Box<dyn Io>>;

/// What the transcription session is set up with.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionConfig {
    pub model: String,
    pub prompt: Option<String>,
    pub keywords: Vec<String>,
    /// Expected languages (ISO codes); empty = auto-detect.
    pub languages: Vec<String>,
}

/// Models that take `keywords` and a `languages` list (older ones take one `language`).
fn new_style(model: &str) -> bool {
    model.starts_with("gpt-transcribe") || model.starts_with("gpt-live")
}

/// Whether a transcription model can be used live at all.
pub fn supports_live(model: &str) -> bool {
    !model.contains("diarize")
}

/// Keywords as the Realtime API accepts them: one line each, no backticks or angle brackets,
/// no blanks, no duplicates.
pub fn sanitize_keywords(keywords: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for k in keywords {
        let cleaned = k.replace(['\r', '\n', '\t', '`', '<', '>'], " ");
        let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
        if !cleaned.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&cleaned)) {
            out.push(cleaned);
        }
    }
    out
}

/// The `session.update` message. `bare` leaves out every hint (used if the hints are rejected).
pub fn session_update(cfg: &SessionConfig, bare: bool) -> Value {
    let mut transcription = json!({ "model": cfg.model });
    if !bare {
        let prompt_ok = cfg.model != "gpt-realtime-whisper";
        if let Some(p) = cfg.prompt.as_deref().map(str::trim).filter(|p| !p.is_empty() && prompt_ok) {
            transcription["prompt"] = json!(p.replace(['\r', '\n'], " "));
        }
        if new_style(&cfg.model) {
            let kw = sanitize_keywords(&cfg.keywords);
            if !kw.is_empty() {
                transcription["keywords"] = json!(kw);
            }
            if !cfg.languages.is_empty() {
                transcription["languages"] = json!(cfg.languages);
            }
        } else if cfg.languages.len() == 1 {
            transcription["language"] = json!(cfg.languages[0]);
        }
    }
    json!({
        "type": "session.update",
        "event_id": if bare { "session-update-bare" } else { "session-update" },
        "session": {
            "type": "transcription",
            "audio": {
                "input": {
                    "format": { "type": "audio/pcm", "rate": SAMPLE_RATE },
                    "transcription": transcription,
                    "turn_detection": Value::Null,
                }
            }
        }
    })
}

pub fn append_message(samples: &[i16]) -> String {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    let audio = base64::engine::general_purpose::STANDARD.encode(bytes);
    json!({ "type": "input_audio_buffer.append", "audio": audio }).to_string()
}

pub fn commit_message(index: usize) -> String {
    json!({ "type": "input_audio_buffer.commit", "event_id": commit_event_id(index) }).to_string()
}

pub fn clear_message(index: usize) -> String {
    json!({ "type": "input_audio_buffer.clear", "event_id": format!("clear-{index}") }).to_string()
}

pub fn commit_event_id(index: usize) -> String {
    format!("commit-{index}")
}

/// The piece index a client event id refers to, if it was a commit.
pub fn commit_index(event_id: &str) -> Option<usize> {
    event_id.strip_prefix("commit-")?.parse().ok()
}

/// Server events Wisper reacts to.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerEvent {
    SessionUpdated,
    Committed { item_id: String, previous_item_id: Option<String> },
    Delta { item_id: String, delta: String },
    Completed { item_id: String, transcript: String },
    Failed { item_id: String, message: String },
    Error { event_id: Option<String>, code: Option<String>, message: String },
    Other(String),
}

pub fn parse_event(text: &str) -> ServerEvent {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return ServerEvent::Other(text.chars().take(80).collect()),
    };
    let s = |x: &Value| x.as_str().unwrap_or("").to_string();
    let typ = v["type"].as_str().unwrap_or("");
    match typ {
        "session.updated" | "transcription_session.updated" => ServerEvent::SessionUpdated,
        "input_audio_buffer.committed" => ServerEvent::Committed {
            item_id: s(&v["item_id"]),
            previous_item_id: v["previous_item_id"].as_str().map(String::from),
        },
        "conversation.item.input_audio_transcription.delta" => ServerEvent::Delta { item_id: s(&v["item_id"]), delta: s(&v["delta"]) },
        "conversation.item.input_audio_transcription.completed" => {
            ServerEvent::Completed { item_id: s(&v["item_id"]), transcript: s(&v["transcript"]).trim().to_string() }
        }
        "conversation.item.input_audio_transcription.failed" => {
            let e = &v["error"];
            let msg = e["message"].as_str().or(e["code"].as_str()).unwrap_or("transcription failed").to_string();
            ServerEvent::Failed { item_id: s(&v["item_id"]), message: msg }
        }
        "error" => {
            let e = &v["error"];
            ServerEvent::Error {
                event_id: e["event_id"].as_str().map(String::from),
                code: e["code"].as_str().map(String::from),
                message: e["message"].as_str().unwrap_or("unknown error").to_string(),
            }
        }
        other => ServerEvent::Other(other.to_string()),
    }
}

fn ws_error(e: tungstenite::Error) -> ProviderError {
    match e {
        tungstenite::Error::Http(resp) => {
            let status = resp.status().as_u16();
            let body = resp.body().as_ref().map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
            super::openai::parse_error(status, &body, &reqwest::header::HeaderMap::new())
        }
        tungstenite::Error::Io(io) => ProviderError::new(ErrorKind::Network, NAME, io.to_string()),
        other => ProviderError::new(ErrorKind::Network, NAME, other.to_string()),
    }
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, ProviderError> {
    static CONFIG: std::sync::OnceLock<Result<Arc<rustls::ClientConfig>, String>> = std::sync::OnceLock::new();
    CONFIG
        .get_or_init(|| {
            use rustls_platform_verifier::BuilderVerifierExt;
            let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let mut cfg = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())?
                .with_platform_verifier()
                .map_err(|e| e.to_string())?
                .with_no_client_auth();
            cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
            Ok(Arc::new(cfg))
        })
        .clone()
        .map_err(|e| ProviderError::new(ErrorKind::Network, NAME, format!("TLS set-up: {e}")))
}

/// Opens the WebSocket (TCP, then TLS for `wss://`, then the HTTP upgrade). No timeout here:
/// the caller bounds the whole set-up.
pub async fn connect(url: &str, key: &str) -> Result<Socket, ProviderError> {
    let mut req = url.into_client_request().map_err(ws_error)?;
    let auth = http::HeaderValue::from_str(&format!("Bearer {}", key.trim()))
        .map_err(|_| ProviderError::new(ErrorKind::Auth, NAME, "the API key contains characters that can't be sent"))?;
    req.headers_mut().insert(http::header::AUTHORIZATION, auth);
    let uri = req.uri().clone();
    let host = uri.host().ok_or_else(|| ProviderError::new(ErrorKind::BadRequest, NAME, "no host in URL"))?.to_string();
    let tls = uri.scheme_str() == Some("wss");
    let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
    let tcp = tokio::net::TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|e| ProviderError::new(ErrorKind::Network, NAME, e.to_string()))?;
    let _ = tcp.set_nodelay(true);
    let io: Box<dyn Io> = if tls {
        let name = rustls::pki_types::ServerName::try_from(host.clone())
            .map_err(|e| ProviderError::new(ErrorKind::BadRequest, NAME, e.to_string()))?;
        let stream = tokio_rustls::TlsConnector::from(tls_config()?)
            .connect(name, tcp)
            .await
            .map_err(|e| ProviderError::new(ErrorKind::Network, NAME, format!("TLS: {e}")))?;
        Box::new(stream)
    } else {
        Box::new(tcp)
    };
    let (ws, _resp) = tokio_tungstenite::client_async(req, io).await.map_err(ws_error)?;
    Ok(ws)
}

/// Connects and sets up the transcription session, waiting until the server confirms it (so
/// no audio reaches a session with the wrong settings). If the hints are rejected, retries once
/// without them, as the file endpoint does.
pub async fn open_session(url: &str, key: &str, cfg: &SessionConfig, limit: Duration) -> Result<Socket, ProviderError> {
    use futures_util::{SinkExt, StreamExt};
    let fut = async {
        let mut ws = connect(url, key).await?;
        let send = |v: Value| tungstenite::Message::Text(v.to_string().into());
        ws.send(send(session_update(cfg, false))).await.map_err(ws_error)?;
        let mut bare = false;
        while let Some(msg) = ws.next().await {
            let text = match msg.map_err(ws_error)? {
                tungstenite::Message::Text(t) => t.to_string(),
                tungstenite::Message::Close(f) => {
                    return Err(ProviderError::new(ErrorKind::Network, NAME, format!("closed during set-up: {f:?}")));
                }
                _ => continue,
            };
            match parse_event(&text) {
                ServerEvent::SessionUpdated => return Ok(ws),
                ServerEvent::Error { event_id, message, code } if event_id.as_deref().is_some_and(|e| e.starts_with("session-update")) => {
                    if bare {
                        return Err(ProviderError::new(ErrorKind::BadRequest, NAME, message));
                    }
                    tracing::warn!("live transcription hints rejected ({code:?}: {message}); retrying without them");
                    bare = true;
                    ws.send(send(session_update(cfg, true))).await.map_err(ws_error)?;
                }
                ServerEvent::Error { message, code, .. } => {
                    let kind = match code.as_deref() {
                        Some("invalid_api_key") => ErrorKind::Auth,
                        Some("insufficient_quota") => ErrorKind::Quota,
                        _ => ErrorKind::BadRequest,
                    };
                    return Err(ProviderError::new(kind, NAME, message));
                }
                _ => {}
            }
        }
        Err(ProviderError::new(ErrorKind::Network, NAME, "connection closed during set-up"))
    };
    match tokio::time::timeout(limit, fut).await {
        Ok(r) => r,
        Err(_) => Err(ProviderError::new(ErrorKind::Timeout, NAME, "live transcription took too long to connect")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(model: &str) -> SessionConfig {
        SessionConfig {
            model: model.into(),
            prompt: Some("Dictation\nby someone".into()),
            keywords: vec!["zechus".into(), "bad`tick".into(), "two\nlines".into(), "  ".into(), "Zechus".into(), "<tag>".into()],
            languages: vec!["en".into(), "he".into()],
        }
    }

    #[test]
    fn keywords_are_sanitised() {
        let k = sanitize_keywords(&cfg("gpt-transcribe").keywords);
        assert_eq!(k, vec!["zechus", "bad tick", "two lines", "tag"]);
        assert!(k.iter().all(|k| !k.contains(['\n', '\r', '`', '<', '>']) && !k.is_empty()));
    }

    #[test]
    fn session_update_for_gpt_transcribe() {
        let v = session_update(&cfg("gpt-transcribe"), false);
        assert_eq!(v["type"], "session.update");
        assert_eq!(v["session"]["type"], "transcription");
        let input = &v["session"]["audio"]["input"];
        assert_eq!(input["format"], json!({"type": "audio/pcm", "rate": 24000}));
        assert!(input["turn_detection"].is_null());
        assert!(input.as_object().unwrap().contains_key("turn_detection"), "turn_detection must be sent as null");
        let t = &input["transcription"];
        assert_eq!(t["model"], "gpt-transcribe");
        assert_eq!(t["prompt"], "Dictation by someone");
        assert_eq!(t["keywords"], json!(["zechus", "bad tick", "two lines", "tag"]));
        assert_eq!(t["languages"], json!(["en", "he"]));
        assert!(t.get("language").is_none(), "never send `language` together with `languages`");
        // The serialized message has no raw newline or backtick anywhere in the keywords.
        let s = v.to_string();
        assert!(!s.contains('`'));
    }

    #[test]
    fn session_update_for_older_models_and_bare() {
        let mut c = cfg("gpt-4o-transcribe");
        c.languages = vec!["en".into()];
        let t = session_update(&c, false)["session"]["audio"]["input"]["transcription"].clone();
        assert_eq!(t["language"], "en");
        assert!(t.get("keywords").is_none() && t.get("languages").is_none());
        let bare = session_update(&cfg("gpt-transcribe"), true);
        assert_eq!(bare["session"]["audio"]["input"]["transcription"], json!({"model": "gpt-transcribe"}));
        assert_eq!(bare["event_id"], "session-update-bare");
        assert!(!supports_live("gpt-4o-transcribe-diarize"));
        assert!(supports_live("gpt-transcribe"));
    }

    #[test]
    fn parses_server_events() {
        assert_eq!(
            parse_event(r#"{"type":"input_audio_buffer.committed","item_id":"b","previous_item_id":"a"}"#),
            ServerEvent::Committed { item_id: "b".into(), previous_item_id: Some("a".into()) }
        );
        assert_eq!(
            parse_event(r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"b","content_index":0,"transcript":" Hi there "}"#),
            ServerEvent::Completed { item_id: "b".into(), transcript: "Hi there".into() }
        );
        assert_eq!(
            parse_event(r#"{"type":"conversation.item.input_audio_transcription.failed","item_id":"c","error":{"message":"bad audio"}}"#),
            ServerEvent::Failed { item_id: "c".into(), message: "bad audio".into() }
        );
        assert_eq!(
            parse_event(r#"{"type":"error","error":{"type":"invalid_request_error","message":"empty","event_id":"commit-3"}}"#),
            ServerEvent::Error { event_id: Some("commit-3".into()), code: None, message: "empty".into() }
        );
        assert_eq!(parse_event(r#"{"type":"session.updated","session":{}}"#), ServerEvent::SessionUpdated);
        assert!(matches!(parse_event("not json"), ServerEvent::Other(_)));
        assert_eq!(commit_index("commit-12"), Some(12));
        assert_eq!(commit_index("clear-12"), None);
    }

    #[test]
    fn append_is_base64_pcm16_le() {
        let m: Value = serde_json::from_str(&append_message(&[1, -2])).unwrap();
        assert_eq!(m["type"], "input_audio_buffer.append");
        let bytes = base64::engine::general_purpose::STANDARD.decode(m["audio"].as_str().unwrap()).unwrap();
        assert_eq!(bytes, vec![1, 0, 0xFE, 0xFF]);
    }
}

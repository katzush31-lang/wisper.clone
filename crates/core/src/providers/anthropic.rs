//! Anthropic Claude: cleanup and command mode via the Messages API (raw HTTP; there is no
//! official Rust SDK).

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::*;

pub const NAME: &str = "Claude";
pub(crate) const BASE: &str = "https://api.anthropic.com/v1";
const VERSION: &str = "2023-06-01";

#[derive(Clone)]
pub struct Anthropic {
    key: String,
    base: String,
    client: reqwest::Client,
}

impl Anthropic {
    pub fn new(key: &str) -> Self {
        Self { key: key.trim().to_string(), base: BASE.into(), client: shared_client() }
    }

    pub fn with_base(mut self, base: &str) -> Self {
        self.base = base.trim_end_matches('/').to_string();
        self
    }

    pub async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        let resp = self
            .client
            .get(format!("{}/models?limit=100", self.base))
            .header("x-api-key", &self.key)
            .header("anthropic-version", VERSION)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| from_reqwest(NAME, e))?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let body = resp.text().await.map_err(|e| from_reqwest(NAME, e))?;
        if status != 200 {
            return Err(parse_error(status, &body, &headers));
        }
        let v: Value = serde_json::from_str(&body)
            .map_err(|e| ProviderError::new(ErrorKind::BadResponse, NAME, e.to_string()))?;
        Ok(v["data"]
            .as_array()
            .map(|a| a.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
            .unwrap_or_default())
    }
}

/// Fastest settings each model accepts: dictation cleanup needs no long reasoning.
pub fn speed_settings(model: &str) -> (Option<Value>, Option<&'static str>) {
    let m = model;
    if m.starts_with("claude-sonnet-5-5") {
        // Thinking can't be "disabled" on Sonnet 5.5; between_tools turns off up-front thinking.
        (Some(json!({"type": "between_tools"})), Some("low"))
    } else if m.starts_with("claude-haiku-5") || m.starts_with("claude-sonnet-5") || m == "claude-opus-5" {
        (Some(json!({"type": "disabled"})), Some("low"))
    } else if m.starts_with("claude-opus-5") || m.starts_with("claude-fable") || m.starts_with("claude-opus-4-8") || m.starts_with("claude-opus-4-7") {
        // Thinking can't be turned off here (or is off by default); keep effort low.
        (None, Some("low"))
    } else if m.starts_with("claude-opus-4-6") || m.starts_with("claude-sonnet-4-6") {
        (None, Some("low"))
    } else {
        (None, None)
    }
}

pub fn messages_body(req: &TextRequest) -> Value {
    let system: Vec<Value> = req
        .system
        .iter()
        .map(|b| {
            let mut block = json!({"type": "text", "text": b.text});
            if b.cache {
                block["cache_control"] = json!({"type": "ephemeral"});
            }
            block
        })
        .collect();
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "system": system,
        "messages": [{"role": "user", "content": req.user}],
    });
    let (thinking, effort) = speed_settings(&req.model);
    if let Some(t) = thinking {
        body["thinking"] = t;
    }
    if let Some(e) = effort {
        body["output_config"] = json!({"effort": e});
    }
    body
}

fn clip(s: &str) -> String {
    s.chars().take(300).collect()
}

pub fn parse_error(status: u16, body: &str, headers: &reqwest::header::HeaderMap) -> ProviderError {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let msg = v["error"]["message"].as_str().unwrap_or(body).to_string();
    let typ = v["error"]["type"].as_str().unwrap_or("");
    let lower = msg.to_lowercase();
    let kind = match (status, typ) {
        (401, _) | (_, "authentication_error") => ErrorKind::Auth,
        (403, _) | (_, "permission_error") => ErrorKind::Auth,
        (_, "billing_error") => ErrorKind::Quota,
        (400, _) if lower.contains("credit balance") || lower.contains("billing") => ErrorKind::Quota,
        (429, _) | (_, "rate_limit_error") => ErrorKind::RateLimited,
        (529, _) | (_, "overloaded_error") => ErrorKind::Server,
        (500..=599, _) | (_, "api_error") => ErrorKind::Server,
        (408, _) | (_, "timeout_error") => ErrorKind::Timeout,
        _ => ErrorKind::BadRequest,
    };
    let mut e = ProviderError::new(kind, NAME, format!("HTTP {status}: {}", clip(&msg)));
    e.retry_after = retry_after(headers);
    e
}

pub fn parse_message(body: &str) -> Result<TextResponse, ProviderError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| ProviderError::new(ErrorKind::BadResponse, NAME, format!("{e}: {}", clip(body))))?;
    let stop = v["stop_reason"].as_str().unwrap_or("");
    if stop == "refusal" {
        return Err(ProviderError::new(
            ErrorKind::Refused,
            NAME,
            v["stop_details"]["explanation"].as_str().unwrap_or("declined").to_string(),
        ));
    }
    let text: String = v["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("");
    if stop == "max_tokens" {
        return Err(ProviderError::new(ErrorKind::BadResponse, NAME, "reply was cut off (max_tokens)"));
    }
    if text.trim().is_empty() {
        return Err(ProviderError::new(ErrorKind::BadResponse, NAME, format!("empty reply: {}", clip(body))));
    }
    Ok(TextResponse {
        text,
        input_tokens: v["usage"]["input_tokens"].as_u64().unwrap_or(0),
        output_tokens: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
        cache_read_tokens: v["usage"]["cache_read_input_tokens"].as_u64().unwrap_or(0),
    })
}

impl Anthropic {
    async fn send(&self, body: &Value) -> Result<TextResponse, ProviderError> {
        let resp = self
            .client
            .post(format!("{}/messages", self.base))
            .timeout(Duration::from_secs(120))
            .header("x-api-key", &self.key)
            .header("anthropic-version", VERSION)
            .json(body)
            .send()
            .await
            .map_err(|e| from_reqwest(NAME, e))?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let text = resp.text().await.map_err(|e| from_reqwest(NAME, e))?;
        if status != 200 {
            return Err(parse_error(status, &text, &headers));
        }
        parse_message(&text)
    }
}

pub fn mentions_speed_settings(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("thinking") || m.contains("output_config") || m.contains("effort")
}

pub fn plain_body(full: &Value) -> Value {
    let mut b = full.clone();
    if let Some(o) = b.as_object_mut() {
        o.remove("thinking");
        o.remove("output_config");
    }
    b
}

#[async_trait]
impl TextModel for Anthropic {
    async fn complete(&self, req: TextRequest) -> Result<TextResponse, ProviderError> {
        if self.key.is_empty() {
            return Err(ProviderError::new(ErrorKind::Auth, NAME, "no Claude API key set"));
        }
        let full = messages_body(&req);
        match self.send(&full).await {
            // If the speed settings are ever rejected (e.g. a model that doesn't accept them),
            // fall back to a plain request rather than failing the dictation.
            Err(e) if e.kind == ErrorKind::BadRequest && mentions_speed_settings(&e.message) => {
                tracing::warn!("Claude rejected speed settings, retrying plain: {}", e.message);
                self.send(&plain_body(&full)).await
            }
            other => other,
        }
    }

    fn provider_name(&self) -> &'static str {
        NAME
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderMap;

    fn req(model: &str) -> TextRequest {
        TextRequest {
            model: model.into(),
            system: vec![SystemBlock { text: "rules".into(), cache: true }, SystemBlock { text: "mode".into(), cache: false }],
            user: "<transcript>hi</transcript>".into(),
            max_tokens: 2000,
        }
    }

    #[test]
    fn body_uses_fast_settings_per_model() {
        let b = messages_body(&req("claude-sonnet-5-5"));
        assert_eq!(b["thinking"]["type"], "between_tools");
        assert_eq!(b["output_config"]["effort"], "low");
        assert_eq!(b["system"][0]["cache_control"]["type"], "ephemeral");
        assert!(b["system"][1].get("cache_control").is_none());
        assert!(b.get("temperature").is_none());
        let b = messages_body(&req("claude-haiku-5-5"));
        assert_eq!(b["thinking"]["type"], "disabled");
        let b = messages_body(&req("claude-opus-5-5"));
        assert!(b.get("thinking").is_none());
        assert_eq!(b["output_config"]["effort"], "low");
        let b = messages_body(&req("claude-haiku-4-5"));
        assert!(b.get("thinking").is_none() && b.get("output_config").is_none());
    }

    #[test]
    fn fast_model_gets_no_thinking_low_effort_and_cached_prompt() {
        let b = messages_body(&req(crate::settings::FastCleanup::default().anthropic_model.as_str()));
        assert_eq!(b["model"], "claude-haiku-5-5");
        assert_eq!(b["thinking"]["type"], "disabled");
        assert_eq!(b["output_config"]["effort"], "low");
        // The cache breakpoint is model-independent, so the fast model caches the prompt too.
        assert_eq!(b["system"][0]["cache_control"]["type"], "ephemeral");
        // Dated/suffixed Haiku ids are covered as well.
        assert_eq!(speed_settings("claude-haiku-5-5-20260101").0.unwrap()["type"], "disabled");
    }

    #[test]
    fn plain_fallback_drops_only_speed_settings() {
        let full = messages_body(&req("claude-sonnet-5-5"));
        let plain = plain_body(&full);
        assert!(plain.get("thinking").is_none() && plain.get("output_config").is_none());
        assert_eq!(plain["system"], full["system"]);
        assert!(mentions_speed_settings("thinking.type: Input should be 'enabled'"));
        assert!(!mentions_speed_settings("model: not found"));
    }

    #[test]
    fn parses_reply_skipping_thinking() {
        let r = parse_message(r#"{"content":[{"type":"thinking","thinking":""},{"type":"text","text":"Clean."}],"stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":2,"cache_read_input_tokens":4000}}"#).unwrap();
        assert_eq!(r.text, "Clean.");
        assert_eq!(r.cache_read_tokens, 4000);
    }

    #[test]
    fn refusal_and_truncation_are_errors() {
        let e = parse_message(r#"{"content":[],"stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber","explanation":"x"}}"#).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Refused);
        let e = parse_message(r#"{"content":[{"type":"text","text":"half"}],"stop_reason":"max_tokens"}"#).unwrap_err();
        assert_eq!(e.kind, ErrorKind::BadResponse);
    }

    #[test]
    fn maps_errors() {
        let h = HeaderMap::new();
        assert_eq!(parse_error(401, r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#, &h).kind, ErrorKind::Auth);
        assert_eq!(parse_error(529, r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#, &h).kind, ErrorKind::Server);
        assert_eq!(parse_error(400, r#"{"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API."}}"#, &h).kind, ErrorKind::Quota);
        assert_eq!(parse_error(404, r#"{"type":"error","error":{"type":"not_found_error","message":"model: x"}}"#, &h).kind, ErrorKind::BadRequest);
    }
}

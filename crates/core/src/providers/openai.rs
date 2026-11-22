//! OpenAI: speech-to-text (gpt-transcribe) and, optionally, cleanup via the Responses API.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use serde_json::{json, Value};

use super::*;

pub const NAME: &str = "OpenAI";
const BASE: &str = "https://api.openai.com/v1";

#[derive(Clone)]
pub struct OpenAi {
    key: String,
    base: String,
    client: reqwest::Client,
}

impl OpenAi {
    pub fn new(key: &str) -> Self {
        Self { key: key.trim().to_string(), base: BASE.into(), client: http_client(Duration::from_secs(90)) }
    }

    pub fn with_base(mut self, base: &str) -> Self {
        self.base = base.trim_end_matches('/').to_string();
        self
    }

    /// Checks the key and returns the model ids the account can use.
    pub async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        let resp = self
            .client
            .get(format!("{}/models", self.base))
            .bearer_auth(&self.key)
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
        let mut ids: Vec<String> = v["data"]
            .as_array()
            .map(|a| a.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
            .unwrap_or_default();
        ids.sort();
        Ok(ids)
    }
}

pub fn transcription_form(req: &TranscribeRequest) -> Result<Form, ProviderError> {
    let part = Part::bytes(req.audio.clone())
        .file_name(req.filename.clone())
        .mime_str(&req.mime)
        .map_err(|e| ProviderError::new(ErrorKind::BadRequest, NAME, e.to_string()))?;
    let mut form = Form::new()
        .text("model", req.model.clone())
        .text("response_format", "json")
        .part("file", part);
    let new_style = req.model.starts_with("gpt-transcribe") || req.model.starts_with("gpt-live");
    if let Some(p) = &req.prompt {
        if !p.trim().is_empty() && req.model != "gpt-4o-transcribe-diarize" {
            form = form.text("prompt", p.clone());
        }
    }
    if new_style {
        for k in &req.keywords {
            let k = k.replace(['<', '>', '\r', '\n'], " ");
            let k = k.trim();
            if !k.is_empty() {
                form = form.text("keywords[]", k.to_string());
            }
        }
        for l in &req.languages {
            form = form.text("languages[]", l.clone());
        }
    } else if req.languages.len() == 1 {
        form = form.text("language", req.languages[0].clone());
    }
    Ok(form)
}

pub fn parse_transcript(body: &str) -> Result<Transcript, ProviderError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| ProviderError::new(ErrorKind::BadResponse, NAME, format!("{e}: {}", clip(body))))?;
    let text = v["text"]
        .as_str()
        .ok_or_else(|| ProviderError::new(ErrorKind::BadResponse, NAME, format!("no text in {}", clip(body))))?;
    let languages = v["languages"]
        .as_array()
        .map(|a| a.iter().filter_map(|l| l["code"].as_str().map(String::from)).collect())
        .unwrap_or_default();
    Ok(Transcript { text: text.trim().to_string(), languages })
}

fn clip(s: &str) -> String {
    s.chars().take(300).collect()
}

pub fn parse_error(status: u16, body: &str, headers: &reqwest::header::HeaderMap) -> ProviderError {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let msg = v["error"]["message"].as_str().unwrap_or(body).to_string();
    let code = v["error"]["code"].as_str().unwrap_or("");
    let typ = v["error"]["type"].as_str().unwrap_or("");
    let kind = match status {
        401 => ErrorKind::Auth,
        403 => ErrorKind::Auth,
        429 if code == "insufficient_quota" || typ == "insufficient_quota" => ErrorKind::Quota,
        429 => ErrorKind::RateLimited,
        402 => ErrorKind::Quota,
        408 => ErrorKind::Timeout,
        500..=599 => ErrorKind::Server,
        _ if code == "invalid_api_key" => ErrorKind::Auth,
        _ => ErrorKind::BadRequest,
    };
    let mut e = ProviderError::new(kind, NAME, format!("HTTP {status}: {}", clip(&msg)));
    e.retry_after = retry_after(headers);
    e
}

#[async_trait]
impl Transcriber for OpenAi {
    async fn transcribe(&self, req: TranscribeRequest) -> Result<Transcript, ProviderError> {
        if self.key.is_empty() {
            return Err(ProviderError::new(ErrorKind::Auth, NAME, "no OpenAI API key set"));
        }
        match self.send_transcription(&req).await {
            // If the hints are ever rejected (too many keywords, prompt too long, unsupported
            // language code), transcribe without them rather than failing.
            Err(e) if e.kind == ErrorKind::BadRequest && (!req.keywords.is_empty() || req.prompt.is_some() || !req.languages.is_empty()) => {
                tracing::warn!("transcription hints rejected, retrying without them: {}", e.message);
                let bare = TranscribeRequest { prompt: None, keywords: vec![], languages: vec![], ..req };
                self.send_transcription(&bare).await
            }
            other => other,
        }
    }
}

impl OpenAi {
    async fn send_responses(&self, body: &Value) -> Result<TextResponse, ProviderError> {
        let resp = self
            .client
            .post(format!("{}/responses", self.base))
            .bearer_auth(&self.key)
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
        parse_responses(&text)
    }

    async fn send_transcription(&self, req: &TranscribeRequest) -> Result<Transcript, ProviderError> {
        let form = transcription_form(req)?;
        let resp = self
            .client
            .post(format!("{}/audio/transcriptions", self.base))
            .bearer_auth(&self.key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| from_reqwest(NAME, e))?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let body = resp.text().await.map_err(|e| from_reqwest(NAME, e))?;
        if status != 200 {
            return Err(parse_error(status, &body, &headers));
        }
        parse_transcript(&body)
    }
}

pub fn reasoning_effort(model: &str) -> Option<&'static str> {
    if model.contains("luna") {
        Some("none")
    } else if model.starts_with("gpt-6") {
        Some("low")
    } else if model.starts_with("gpt-5") {
        Some("minimal")
    } else {
        None
    }
}

pub fn responses_body(req: &TextRequest) -> Value {
    let instructions = req.system.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n\n");
    let mut body = json!({
        "model": req.model,
        "instructions": instructions,
        "input": req.user,
        "max_output_tokens": req.max_tokens,
        "store": false,
    });
    if let Some(effort) = reasoning_effort(&req.model) {
        body["reasoning"] = json!({ "effort": effort });
    }
    body
}

pub fn parse_responses(body: &str) -> Result<TextResponse, ProviderError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| ProviderError::new(ErrorKind::BadResponse, NAME, format!("{e}: {}", clip(body))))?;
    let mut text = String::new();
    let mut refusal = None;
    for item in v["output"].as_array().into_iter().flatten() {
        if item["type"] == "message" {
            for c in item["content"].as_array().into_iter().flatten() {
                match c["type"].as_str() {
                    Some("output_text") => text.push_str(c["text"].as_str().unwrap_or("")),
                    Some("refusal") => refusal = c["refusal"].as_str().map(String::from),
                    _ => {}
                }
            }
        }
    }
    if text.trim().is_empty() {
        if let Some(r) = refusal {
            return Err(ProviderError::new(ErrorKind::Refused, NAME, r));
        }
        if v["status"] == "incomplete" {
            return Err(ProviderError::new(
                ErrorKind::BadResponse,
                NAME,
                format!("incomplete: {}", v["incomplete_details"]["reason"]),
            ));
        }
        return Err(ProviderError::new(ErrorKind::BadResponse, NAME, format!("empty reply: {}", clip(body))));
    }
    Ok(TextResponse {
        text,
        input_tokens: v["usage"]["input_tokens"].as_u64().unwrap_or(0),
        output_tokens: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
        cache_read_tokens: v["usage"]["input_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0),
    })
}

#[async_trait]
impl TextModel for OpenAi {
    async fn complete(&self, req: TextRequest) -> Result<TextResponse, ProviderError> {
        if self.key.is_empty() {
            return Err(ProviderError::new(ErrorKind::Auth, NAME, "no OpenAI API key set"));
        }
        let body = responses_body(&req);
        match self.send_responses(&body).await {
            Err(e) if e.kind == ErrorKind::BadRequest && e.message.to_lowercase().contains("reasoning") => {
                let mut plain = body.clone();
                if let Some(o) = plain.as_object_mut() {
                    o.remove("reasoning");
                }
                self.send_responses(&plain).await
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

    #[test]
    fn parses_transcript() {
        let t = parse_transcript(r#"{"text":" Hello zechus ","languages":[{"code":"en"}]}"#).unwrap();
        assert_eq!(t.text, "Hello zechus");
        assert_eq!(t.languages, vec!["en"]);
        assert!(parse_transcript("{}").is_err());
        assert!(parse_transcript("<html>").is_err());
    }

    #[test]
    fn maps_errors() {
        let h = HeaderMap::new();
        let e = parse_error(401, r#"{"error":{"message":"Incorrect API key","code":"invalid_api_key"}}"#, &h);
        assert_eq!(e.kind, ErrorKind::Auth);
        let e = parse_error(429, r#"{"error":{"message":"quota","type":"insufficient_quota","code":"insufficient_quota"}}"#, &h);
        assert_eq!(e.kind, ErrorKind::Quota);
        let mut h2 = HeaderMap::new();
        h2.insert("retry-after", "2".parse().unwrap());
        let e = parse_error(429, r#"{"error":{"message":"slow down"}}"#, &h2);
        assert_eq!(e.kind, ErrorKind::RateLimited);
        assert_eq!(e.retry_after, Some(2));
        assert_eq!(parse_error(503, "oops", &h).kind, ErrorKind::Server);
        assert_eq!(parse_error(400, r#"{"error":{"message":"bad"}}"#, &h).kind, ErrorKind::BadRequest);
    }

    #[test]
    fn responses_request_and_reply() {
        let req = TextRequest {
            model: "gpt-6.2-sol".into(),
            system: vec![SystemBlock { text: "A".into(), cache: true }, SystemBlock { text: "B".into(), cache: false }],
            user: "hi".into(),
            max_tokens: 100,
        };
        let b = responses_body(&req);
        assert_eq!(b["instructions"], "A\n\nB");
        assert_eq!(b["reasoning"]["effort"], "low");
        assert_eq!(responses_body(&TextRequest { model: "gpt-6-luna".into(), ..req.clone() })["reasoning"]["effort"], "none");
        let r = parse_responses(
            r#"{"status":"completed","output":[{"type":"reasoning"},{"type":"message","content":[{"type":"output_text","text":"Hello."}]}],"usage":{"input_tokens":10,"output_tokens":2,"input_tokens_details":{"cached_tokens":8}}}"#,
        )
        .unwrap();
        assert_eq!(r.text, "Hello.");
        assert_eq!(r.cache_read_tokens, 8);
        let e = parse_responses(r#"{"output":[{"type":"message","content":[{"type":"refusal","refusal":"no"}]}]}"#).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Refused);
    }

    #[test]
    fn form_builds_for_new_and_old_models() {
        let req = TranscribeRequest {
            audio: vec![1, 2, 3],
            filename: "a.flac".into(),
            mime: "audio/flac".into(),
            model: "gpt-transcribe".into(),
            prompt: Some("p".into()),
            keywords: vec!["zechus".into(), "bad<k>".into()],
            languages: vec!["en".into()],
        };
        assert!(transcription_form(&req).is_ok());
        let old = TranscribeRequest { model: "whisper-1".into(), ..req };
        assert!(transcription_form(&old).is_ok());
    }
}

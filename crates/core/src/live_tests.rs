//! End-to-end tests of live transcription: a real recording session talking to a fake
//! Realtime WebSocket server, with a fake file-endpoint transcriber for the fallback.
//!
//! Live transcripts are "live<N>" (N = 24 kHz samples the server received for that piece) and
//! file-endpoint transcripts are "file<N>" (N = 16 kHz samples uploaded), so every piece's text
//! can be checked against its audio, its position and where it came from.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;

use super::{LiveEndpoint, LiveTuning};
use crate::db::Db;
use crate::hotkey::RecordingKind;
use crate::pipeline::{ApiKeys, CaptureContext, Engine, FinishedRecording, Outcome, ProviderFactory};
use crate::prompts::CleanupLevel;
use crate::providers::{ErrorKind, ProviderError, RetryPolicy, TextModel, TextRequest, TextResponse, TranscribeRequest, Transcriber, Transcript};
use crate::settings::{CleanupProvider, Settings};

// ---------------------------------------------------------------------------- fakes

#[derive(Default)]
struct FileStt {
    calls: AtomicUsize,
}

#[async_trait]
impl Transcriber for FileStt {
    async fn transcribe(&self, req: TranscribeRequest) -> Result<Transcript, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let reader = claxon::FlacReader::new(std::io::Cursor::new(req.audio)).unwrap();
        let n = reader.streaminfo().samples.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        Ok(Transcript { text: format!("file{n}"), languages: vec![] })
    }
}

struct NoLlm;

#[async_trait]
impl TextModel for NoLlm {
    async fn complete(&self, _: TextRequest) -> Result<TextResponse, ProviderError> {
        Err(ProviderError::new(ErrorKind::BadRequest, "Fake", "not used"))
    }
    fn provider_name(&self) -> &'static str {
        "Fake"
    }
}

struct Fakes {
    stt: Arc<FileStt>,
    live: Option<LiveEndpoint>,
}

impl ProviderFactory for Fakes {
    fn transcriber(&self, _: &ApiKeys) -> Arc<dyn Transcriber> {
        self.stt.clone()
    }
    fn text_model(&self, _: &ApiKeys, _: CleanupProvider) -> Arc<dyn TextModel> {
        Arc::new(NoLlm)
    }
    fn live_endpoint(&self) -> Option<LiveEndpoint> {
        self.live.clone()
    }
}

/// What the fake server does when piece number `global` (counted across connections) is
/// committed as the `n`-th commit on connection `conn`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Act {
    Complete { after_ms: u64 },
    /// Completes with an empty transcript.
    Empty,
    Fail,
    /// Close the connection instead of answering.
    Drop,
    /// An `error` event with no event id, then carry on normally.
    ErrorNoId,
    /// `committed` names the wrong previous item, then carry on normally.
    WrongPrev,
    /// `committed` leaves previous_item_id out (null), then completes normally.
    NoPrev,
    /// Reject the commit (`error` naming it); the audio stays in the buffer, as on the real API.
    CommitError,
    /// Acknowledge the commit but never send a result.
    Never,
}

#[derive(Debug, Clone, Copy)]
struct CommitInfo {
    conn: usize,
    global: usize,
}

#[derive(Default)]
struct ServerLog {
    auth: Vec<String>,
    updates: Vec<Value>,
    /// (connection, 24 kHz samples) per commit.
    commits: Vec<(usize, usize)>,
    clears: usize,
    connections: usize,
    /// Connections that have ended.
    closed: usize,
}

struct FakeServer {
    url: String,
    log: Arc<Mutex<ServerLog>>,
}

type Behave = Arc<dyn Fn(CommitInfo) -> Act + Send + Sync>;

async fn fake_server(behave: impl Fn(CommitInfo) -> Act + Send + Sync + 'static, reject_hints: bool) -> FakeServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/v1/realtime?intent=transcription", listener.local_addr().unwrap());
    let log = Arc::new(Mutex::new(ServerLog::default()));
    let behave: Behave = Arc::new(behave);
    let global = Arc::new(AtomicUsize::new(0));
    {
        let log = log.clone();
        tokio::spawn(async move {
            let mut conn = 0;
            while let Ok((tcp, _)) = listener.accept().await {
                log.lock().unwrap().connections += 1;
                let log = log.clone();
                let (behave, global) = (behave.clone(), global.clone());
                tokio::spawn(async move {
                    serve(tcp, conn, log.clone(), behave, global, reject_hints).await;
                    log.lock().unwrap().closed += 1;
                });
                conn += 1;
            }
        });
    }
    FakeServer { url, log }
}

async fn serve(tcp: tokio::net::TcpStream, conn: usize, log: Arc<Mutex<ServerLog>>, behave: Behave, global: Arc<AtomicUsize>, reject_hints: bool) {
    let auth = Arc::new(Mutex::new(String::new()));
    let a2 = auth.clone();
    #[allow(clippy::result_large_err)]
    let cb = move |req: &Request, resp: Response| {
        *a2.lock().unwrap() = req.headers().get("authorization").map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
        Ok(resp)
    };
    let Ok(ws) = tokio_tungstenite::accept_hdr_async(tcp, cb).await else { return };
    log.lock().unwrap().auth.push(auth.lock().unwrap().clone());
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Option<Value>>();
    tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            match m {
                Some(v) => {
                    if sink.send(Message::Text(v.to_string().into())).await.is_err() {
                        break;
                    }
                }
                None => {
                    let _ = sink.close().await;
                    break;
                }
            }
        }
    });
    let mut buffered = 0usize;
    let mut n = 0usize;
    let mut last: Option<String> = None;
    let mut rejected = false;
    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(t) = msg else { continue };
        let v: Value = serde_json::from_str(&t).unwrap();
        match v["type"].as_str().unwrap() {
            "session.update" => {
                log.lock().unwrap().updates.push(v.clone());
                let has_hints = v["session"]["audio"]["input"]["transcription"].get("keywords").is_some();
                if reject_hints && has_hints && !rejected {
                    rejected = true;
                    let _ = tx.send(Some(json!({"type": "error", "error": {"type": "invalid_request_error", "message": "bad keyword", "event_id": v["event_id"]}})));
                } else {
                    let _ = tx.send(Some(json!({"type": "session.updated", "session": v["session"]})));
                }
            }
            "input_audio_buffer.append" => {
                let bytes = base64::engine::general_purpose::STANDARD.decode(v["audio"].as_str().unwrap()).unwrap();
                buffered += bytes.len() / 2;
            }
            "input_audio_buffer.clear" => {
                buffered = 0;
                log.lock().unwrap().clears += 1;
                let _ = tx.send(Some(json!({"type": "input_audio_buffer.cleared"})));
            }
            "input_audio_buffer.commit" => {
                let info = CommitInfo { conn, global: global.fetch_add(1, Ordering::SeqCst) };
                let act = behave(info);
                match act {
                    Act::Drop => {
                        let _ = tx.send(None);
                        return;
                    }
                    Act::CommitError => {
                        let _ = tx.send(Some(json!({"type": "error", "error": {"type": "invalid_request_error", "message": "commit rejected", "event_id": v["event_id"]}})));
                        continue;
                    }
                    Act::ErrorNoId => {
                        let _ = tx.send(Some(json!({"type": "error", "error": {"type": "server_error", "message": "something went wrong"}})));
                    }
                    _ => {}
                }
                let item = format!("item_{conn}_{n}");
                n += 1;
                let prev = match act {
                    Act::WrongPrev => Some("item_bogus".to_string()),
                    Act::NoPrev => None,
                    _ => last.clone(),
                };
                let _ = tx.send(Some(json!({"type": "input_audio_buffer.committed", "item_id": item, "previous_item_id": prev})));
                last = Some(item.clone());
                log.lock().unwrap().commits.push((conn, buffered));
                let samples = std::mem::take(&mut buffered);
                let after_ms = match act {
                    Act::Complete { after_ms } => Some(after_ms),
                    Act::NoPrev => Some(10),
                    Act::ErrorNoId | Act::WrongPrev => Some(10),
                    _ => None,
                };
                match act {
                    Act::Empty => {
                        let _ = tx.send(Some(json!({"type": "conversation.item.input_audio_transcription.completed", "item_id": item, "content_index": 0, "transcript": " "})));
                    }
                    _ if after_ms.is_some() => {
                        let after_ms = after_ms.unwrap();
                        let tx = tx.clone();
                        tokio::spawn(async move {
                            let _ = tx.send(Some(json!({"type": "conversation.item.input_audio_transcription.delta", "item_id": item, "delta": "live"})));
                            tokio::time::sleep(Duration::from_millis(after_ms)).await;
                            let _ = tx.send(Some(json!({"type": "conversation.item.input_audio_transcription.completed", "item_id": item, "content_index": 0, "transcript": format!("live{samples}")})));
                        });
                    }
                    Act::Fail => {
                        let _ = tx.send(Some(json!({"type": "conversation.item.input_audio_transcription.failed", "item_id": item, "error": {"message": "audio unclear"}})));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------- harness

struct Harness {
    engine: Arc<Engine>,
    stt: Arc<FileStt>,
    breaker: Arc<super::Breaker>,
    _dir: tempfile::TempDir,
}

fn quick_tuning() -> LiveTuning {
    LiveTuning { stop_grace: Duration::from_millis(400), connect_timeout: Duration::from_secs(3), ..Default::default() }
}

fn harness(url: Option<&str>, tuning: LiveTuning) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(&dir.path().join("wisper.db")).unwrap());
    let stt = Arc::new(FileStt::default());
    let live = url.map(|u| LiveEndpoint::new(u, tuning));
    let breaker = live.as_ref().map(|l| l.breaker.clone()).unwrap_or_default();
    let factory = Arc::new(Fakes { stt: stt.clone(), live });
    let keys = ApiKeys { openai: Some("sk-test".into()), anthropic: None };
    let settings = Settings { cleanup_level: CleanupLevel::None, languages: vec!["en".into(), "he".into()], ..Default::default() };
    let retry = RetryPolicy { attempts: 2, base_delay_ms: 5, max_delay_ms: 10 };
    let engine = Engine::new_with_retry(db, dir.path().to_path_buf(), settings, keys, factory, retry);
    Harness { engine, stt, breaker, _dir: dir }
}

/// 48 kHz stereo: a buzzy "voice" tone or near-silence, per (is_speech, ms).
fn audio48(pattern: &[(bool, u64)], offset: &mut usize) -> Vec<f32> {
    let mut out = Vec::new();
    for &(speech, ms) in pattern {
        for _ in 0..(ms as usize * 48) {
            let n = *offset;
            let t = n as f32 / 48_000.0;
            let v = if speech {
                0.25 * ((2.0 * std::f32::consts::PI * 170.0 * t).sin() + 0.4 * (2.0 * std::f32::consts::PI * 510.0 * t).sin())
            } else {
                0.0005 * ((n * 7919 % 97) as f32 / 97.0 - 0.5)
            };
            out.push(v);
            out.push(v);
            *offset += 1;
        }
    }
    out
}

/// Four pieces of ~12 s each.
fn four_pieces() -> Vec<(bool, u64)> {
    let mut p = vec![];
    for _ in 0..4 {
        p.push((true, 11_000));
        p.push((false, 1_000));
    }
    p
}

/// Records `pattern`, feeding it in blocks with short gaps (as real time would), and returns
/// the finished recording.
fn record(h: &Harness, pattern: &[(bool, u64)]) -> FinishedRecording {
    let ctx = CaptureContext { app: crate::apps::identify("notepad.exe", "Untitled"), ..Default::default() };
    let mut s = h.engine.start_session(RecordingKind::Dictation, ctx, 48_000, 2, tokio::runtime::Handle::current()).unwrap();
    let mut offset = 0;
    for block in pattern.chunks(2) {
        for chunk in audio48(block, &mut offset).chunks(960) {
            s.push(chunk);
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    s.finish()
}

async fn process(h: &Harness, rec: FinishedRecording) -> (String, Duration) {
    let id = rec.id.clone();
    let t0 = std::time::Instant::now();
    let out = h.engine.process(rec, &|_| {}).await.unwrap();
    assert!(matches!(out, Outcome::Text(_)), "{out:?}");
    (id, t0.elapsed())
}

/// Checks every piece's text matches its own audio and position, that the transcript is all
/// of them in order with nothing doubled, and returns where each piece's text came from.
fn sources(h: &Harness, id: &str) -> Vec<&'static str> {
    let rows = h.engine.db.segments(id).unwrap();
    let item = h.engine.db.get(id).unwrap().unwrap();
    let mut texts = vec![];
    let mut from = vec![];
    for r in rows.iter().filter(|r| r.has_speech) {
        assert_eq!(r.status, "done", "piece {} not done", r.idx);
        let t = r.text.clone().unwrap();
        let len = (r.end_sample - r.start_sample) as f64;
        if let Some(n) = t.strip_prefix("live") {
            let n: f64 = n.parse().unwrap();
            assert!((n - len * 1.5).abs() <= 3.0, "piece {}: live audio {n} vs recorded {len}", r.idx);
            from.push("live");
        } else if let Some(n) = t.strip_prefix("file") {
            assert_eq!(n.parse::<f64>().unwrap(), len, "piece {}", r.idx);
            from.push("file");
        } else {
            panic!("unexpected text {t}");
        }
        texts.push(t);
    }
    assert_eq!(item.raw_text.as_deref(), Some(texts.join(" ").as_str()));
    from
}

fn timings(h: &Harness, id: &str) -> Value {
    h.engine.db.get(id).unwrap().unwrap().timings.unwrap()
}

// ---------------------------------------------------------------------------- tests

#[tokio::test(flavor = "multi_thread")]
async fn live_pieces_come_back_in_order() {
    let srv = fake_server(|_| Act::Complete { after_ms: 20 }, false).await;
    let h = harness(Some(&srv.url), quick_tuning());
    let rec = record(&h, &four_pieces());
    let (id, _) = process(&h, rec).await;
    let from = sources(&h, &id);
    assert!(from.len() >= 3, "{from:?}");
    assert!(from.iter().all(|f| *f == "live"), "{from:?}");
    assert_eq!(h.stt.calls.load(Ordering::SeqCst), 0, "file endpoint not needed");
    let t = timings(&h, &id);
    assert_eq!(t["live"], true);
    assert_eq!(t["livePieces"], from.len());
    assert_eq!(t["fallbackPieces"], 0);
    assert!(t["transcribeWaitMs"].as_u64().is_some());

    let log = srv.log.lock().unwrap();
    assert_eq!(log.connections, 1);
    assert_eq!(log.auth, vec!["Bearer sk-test"]);
    let input = &log.updates[0]["session"]["audio"]["input"];
    assert_eq!(log.updates[0]["session"]["type"], "transcription");
    assert_eq!(input["format"]["rate"], 24_000);
    assert!(input["turn_detection"].is_null());
    let tr = &input["transcription"];
    assert_eq!(tr["model"], "gpt-transcribe");
    assert_eq!(tr["languages"], json!(["en", "he"]));
    assert!(tr.get("language").is_none());
    assert!(tr["prompt"].as_str().unwrap().contains("Notepad"));
    let kw: Vec<String> = serde_json::from_value(tr["keywords"].clone()).unwrap();
    assert!(kw.iter().any(|k| k == "zechus"), "{kw:?}");
    assert!(kw.iter().all(|k| !k.contains(['\n', '\r', '`'])));
}

#[tokio::test(flavor = "multi_thread")]
async fn out_of_order_completions_keep_spoken_order() {
    // Earlier pieces take longer, so completions arrive last-first.
    let srv = fake_server(|c| Act::Complete { after_ms: 600u64.saturating_sub(c.global as u64 * 150) }, false).await;
    let h = harness(Some(&srv.url), quick_tuning());
    let rec = record(&h, &four_pieces());
    let (id, _) = process(&h, rec).await;
    let from = sources(&h, &id);
    assert!(from.iter().all(|f| *f == "live"), "{from:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn dropped_connection_mid_recording_falls_back_for_the_rest() {
    // The connection dies when the third piece is committed; no reconnecting.
    let srv = fake_server(|c| if c.global == 2 { Act::Drop } else { Act::Complete { after_ms: 10 } }, false).await;
    let h = harness(Some(&srv.url), LiveTuning { max_reconnects: 0, ..quick_tuning() });
    let rec = record(&h, &four_pieces());
    let (id, _) = process(&h, rec).await;
    let from = sources(&h, &id);
    assert_eq!(&from[..2], &["live", "live"], "{from:?}");
    assert!(from[2..].iter().all(|f| *f == "file"), "{from:?}");
    assert_eq!(h.stt.calls.load(Ordering::SeqCst), from.len() - 2);
    assert_eq!(timings(&h, &id)["fallbackPieces"], from.len() - 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn dropped_connection_reconnects_for_later_pieces() {
    let srv = fake_server(|c| if c.conn == 0 && c.global == 1 { Act::Drop } else { Act::Complete { after_ms: 10 } }, false).await;
    let h = harness(Some(&srv.url), quick_tuning());
    let rec = record(&h, &four_pieces());
    let (id, _) = process(&h, rec).await;
    let from = sources(&h, &id);
    assert_eq!(from[0], "live");
    assert_eq!(from[1], "file", "{from:?}");
    // Pieces after the drop went live again on a fresh connection.
    assert_eq!(*from.last().unwrap(), "live", "{from:?}");
    let log = srv.log.lock().unwrap();
    assert_eq!(log.connections, 2);
    assert!(log.commits.iter().any(|(conn, _)| *conn == 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn late_live_result_is_ignored_after_fallback() {
    // The last piece's live result comes long after the user stopped.
    // (6 s late vs. a 0.3 s grace: a wide margin, so a slow machine can't blur the two.)
    let srv = fake_server(|c| if c.global == 3 { Act::Complete { after_ms: 6_000 } } else { Act::Complete { after_ms: 10 } }, false).await;
    let h = harness(Some(&srv.url), LiveTuning { stop_grace: Duration::from_millis(300), ..quick_tuning() });
    let rec = record(&h, &four_pieces());
    let (id, took) = process(&h, rec).await;
    assert!(took < Duration::from_millis(4_000), "waited {took:?} for a late result");
    let from = sources(&h, &id);
    assert_eq!(from, vec!["live", "live", "live", "file"]);
    // Let the late result arrive: nothing changes and nothing is doubled.
    tokio::time::sleep(Duration::from_millis(6_300)).await;
    let again = sources(&h, &id);
    assert_eq!(again, from);
    assert_eq!(h.stt.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_failure_uses_the_normal_path_without_waiting() {
    // Nothing listens on this port.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let url = format!("ws://127.0.0.1:{port}/v1/realtime");
    // A long grace: if the failed connection made us wait for it, this would take 10 s+.
    let h = harness(Some(&url), LiveTuning { stop_grace: Duration::from_secs(10), ..LiveTuning::default() });
    let rec = record(&h, &four_pieces());
    let (id, took) = process(&h, rec).await;
    let from = sources(&h, &id);
    assert!(from.iter().all(|f| *f == "file"), "{from:?}");
    assert!(took < Duration::from_secs(5), "took {took:?}");
    assert!(h.breaker.tripped(), "a failed connect pauses live transcription");
    let t = timings(&h, &id);
    assert_eq!(t["live"], false);
    assert_eq!(t["liveConnected"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_item_falls_back_for_that_piece_only() {
    let srv = fake_server(|c| if c.global == 1 { Act::Fail } else { Act::Complete { after_ms: 10 } }, false).await;
    let h = harness(Some(&srv.url), quick_tuning());
    let rec = record(&h, &four_pieces());
    let (id, _) = process(&h, rec).await;
    let from = sources(&h, &id);
    assert_eq!(from[1], "file");
    assert!(from.iter().enumerate().all(|(i, f)| i == 1 || *f == "live"), "{from:?}");
    assert_eq!(h.stt.calls.load(Ordering::SeqCst), 1);
    assert_eq!(srv.log.lock().unwrap().connections, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_hints_are_retried_without_them() {
    let srv = fake_server(|_| Act::Complete { after_ms: 10 }, true).await;
    let h = harness(Some(&srv.url), quick_tuning());
    let rec = record(&h, &[(true, 3_000), (false, 500)]);
    let (id, _) = process(&h, rec).await;
    assert_eq!(sources(&h, &id), vec!["live"]);
    let log = srv.log.lock().unwrap();
    assert_eq!(log.updates.len(), 2);
    assert_eq!(log.updates[1]["session"]["audio"]["input"]["transcription"], json!({"model": "gpt-transcribe"}));
}

#[tokio::test(flavor = "multi_thread")]
async fn silent_pieces_are_cleared_not_sent() {
    let srv = fake_server(|_| Act::Complete { after_ms: 10 }, false).await;
    let h = harness(Some(&srv.url), quick_tuning());
    // Speech, then a long silence (cut at the length limit as a silent piece), then speech.
    let rec = record(&h, &[(true, 9_000), (false, 1_200), (false, 47_000), (false, 100), (true, 3_000), (false, 400)]);
    let (id, _) = process(&h, rec).await;
    let rows = h.engine.db.segments(&id).unwrap();
    assert!(rows.iter().any(|r| !r.has_speech), "expected a silent piece");
    let from = sources(&h, &id);
    assert!(from.iter().all(|f| *f == "live"), "{from:?}");
    let log = srv.log.lock().unwrap();
    assert!(log.clears >= 1);
    assert_eq!(log.commits.len(), from.len(), "only pieces with speech are committed");
}

#[tokio::test(flavor = "multi_thread")]
async fn recording_on_disk_is_identical_with_live_on_or_off() {
    let srv = fake_server(|_| Act::Complete { after_ms: 10 }, false).await;
    let on = harness(Some(&srv.url), quick_tuning());
    let off = harness(None, quick_tuning());
    let pattern = four_pieces();
    let a = record(&on, &pattern);
    let b = record(&off, &pattern);
    let bytes_a = std::fs::read(&a.pcm).unwrap();
    let bytes_b = std::fs::read(&b.pcm).unwrap();
    assert!(!bytes_a.is_empty());
    assert_eq!(bytes_a, bytes_b);
    assert_eq!(on.engine.db.segments(&a.id).unwrap().len(), off.engine.db.segments(&b.id).unwrap().len());
    // And live off really is the normal path.
    let (id, _) = process(&off, b).await;
    assert!(sources(&off, &id).iter().all(|f| *f == "file"));
    assert_eq!(timings(&off, &id)["live"], false);
    let _ = process(&on, a).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn live_setting_off_opens_no_connection() {
    let srv = fake_server(|_| Act::Complete { after_ms: 10 }, false).await;
    let h = harness(Some(&srv.url), quick_tuning());
    let mut s = h.engine.settings();
    s.providers.live_transcription = false;
    h.engine.set_settings(s);
    let rec = record(&h, &[(true, 3_000), (false, 500)]);
    let (id, _) = process(&h, rec).await;
    assert_eq!(sources(&h, &id), vec!["file"]);
    assert_eq!(srv.log.lock().unwrap().connections, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_recording_stops_live_transcription() {
    let srv = fake_server(|_| Act::Complete { after_ms: 10 }, false).await;
    let h = harness(Some(&srv.url), quick_tuning());
    let ctx = CaptureContext::default();
    let mut s = h.engine.start_session(RecordingKind::Dictation, ctx, 48_000, 2, tokio::runtime::Handle::current()).unwrap();
    let mut offset = 0;
    for chunk in audio48(&[(true, 2_000)], &mut offset).chunks(960) {
        s.push(chunk);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(srv.log.lock().unwrap().connections, 1);
    s.cancel();
    // The connection is closed promptly and nothing was committed.
    for _ in 0..50 {
        if srv.log.lock().unwrap().closed == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let log = srv.log.lock().unwrap();
    assert_eq!(log.closed, 1, "live connection left open after cancel");
    assert!(log.commits.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn long_dictations_rotate_to_fresh_sessions_without_losing_pieces() {
    // Rotate at every piece boundary (in real use: after 25 minutes).
    let srv = fake_server(|_| Act::Complete { after_ms: 50 }, false).await;
    let h = harness(Some(&srv.url), LiveTuning { rotate_after: Duration::ZERO, ..quick_tuning() });
    let rec = record(&h, &four_pieces());
    let (id, _) = process(&h, rec).await;
    let from = sources(&h, &id);
    assert!(from.iter().all(|f| *f == "live"), "{from:?}");
    // Old sessions are closed once their pieces are back.
    for _ in 0..100 {
        let all_closed = {
            let log = srv.log.lock().unwrap();
            log.closed == log.connections
        };
        if all_closed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let log = srv.log.lock().unwrap();
    assert!(log.connections >= 2, "connections: {}", log.connections);
    assert_eq!(log.closed, log.connections);
    // Each piece was committed exactly once, across the sessions.
    assert_eq!(log.commits.len(), from.len());
}

/// A server that accepts the TCP connection and then never answers (a bad network or a
/// captive portal). Returns its URL and how many connections it has accepted.
async fn hanging_server() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/v1/realtime?intent=transcription", listener.local_addr().unwrap());
    let accepted = Arc::new(AtomicUsize::new(0));
    let a = accepted.clone();
    tokio::spawn(async move {
        let mut held = vec![];
        while let Ok((tcp, _)) = listener.accept().await {
            a.fetch_add(1, Ordering::SeqCst);
            held.push(tcp);
        }
    });
    (url, accepted)
}

#[tokio::test(flavor = "multi_thread")]
async fn hanging_connect_adds_no_wait_and_pauses_live() {
    let (url, accepted) = hanging_server().await;
    // Long grace: waiting on the hung connection would show up as 10 s+.
    let tuning = LiveTuning { connect_timeout: Duration::from_millis(1_000), stop_grace: Duration::from_secs(10), ..LiveTuning::default() };
    let h = harness(Some(&url), tuning);
    let rec = record(&h, &four_pieces());
    let (id, took) = process(&h, rec).await;
    assert!(took < Duration::from_secs(5), "took {took:?}");
    let from = sources(&h, &id);
    assert!(from.iter().all(|f| *f == "file"), "{from:?}");
    assert_eq!(timings(&h, &id)["liveConnected"], false);
    // The attempt times out in the background and pauses live transcription...
    for _ in 0..100 {
        if h.breaker.tripped() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(h.breaker.tripped());
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    // ...so the next dictation doesn't try at all.
    let rec = record(&h, &[(true, 3_000), (false, 500)]);
    let (id, took) = process(&h, rec).await;
    assert!(took < Duration::from_secs(5), "took {took:?}");
    assert_eq!(sources(&h, &id), vec!["file"]);
    assert_eq!(timings(&h, &id)["liveTried"], false);
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn stalled_session_falls_back_and_pauses_live() {
    let srv = fake_server(|_| Act::Never, false).await;
    let tuning = LiveTuning { stall_timeout: Duration::from_millis(500), stop_grace: Duration::from_secs(10), ..quick_tuning() };
    let h = harness(Some(&srv.url), tuning);
    let rec = record(&h, &four_pieces());
    let (id, took) = process(&h, rec).await;
    assert!(took < Duration::from_secs(5), "took {took:?}");
    let from = sources(&h, &id);
    assert!(from.iter().all(|f| *f == "file"), "{from:?}");
    assert!(h.breaker.tripped());
    let rec = record(&h, &[(true, 3_000), (false, 500)]);
    let (id, _) = process(&h, rec).await;
    assert_eq!(timings(&h, &id)["liveTried"], false);
    assert_eq!(srv.log.lock().unwrap().connections, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_live_transcript_is_transcribed_again_the_normal_way() {
    let srv = fake_server(|c| if c.global == 1 { Act::Empty } else { Act::Complete { after_ms: 10 } }, false).await;
    let h = harness(Some(&srv.url), quick_tuning());
    let rec = record(&h, &four_pieces());
    let (id, _) = process(&h, rec).await;
    let from = sources(&h, &id);
    assert_eq!(from[1], "file", "{from:?}");
    assert!(from.iter().enumerate().all(|(i, f)| i == 1 || *f == "live"), "{from:?}");
    assert_eq!(timings(&h, &id)["fallbackPieces"], 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn error_without_event_id_drops_the_connection_and_keeps_text_complete() {
    let srv = fake_server(|c| if c.conn == 0 && c.global == 1 { Act::ErrorNoId } else { Act::Complete { after_ms: 10 } }, false).await;
    let h = harness(Some(&srv.url), quick_tuning());
    let rec = record(&h, &four_pieces());
    let (id, _) = process(&h, rec).await;
    // sources() checks every piece's text matches its own audio, in order, nothing doubled.
    let from = sources(&h, &id);
    assert_eq!(from[1], "file", "{from:?}");
    assert!(from.len() >= 3);
    let log = srv.log.lock().unwrap();
    assert!(log.closed >= 1, "the connection that reported the error was dropped");
}

#[tokio::test(flavor = "multi_thread")]
async fn item_order_mismatch_drops_the_connection() {
    let srv = fake_server(|c| if c.conn == 0 && c.global == 2 { Act::WrongPrev } else { Act::Complete { after_ms: 10 } }, false).await;
    let h = harness(Some(&srv.url), quick_tuning());
    let rec = record(&h, &four_pieces());
    let (id, _) = process(&h, rec).await;
    let from = sources(&h, &id);
    assert_eq!(from[2], "file", "{from:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_commit_clears_leftover_audio() {
    // The server rejects piece 1's commit and keeps its audio, as the real API does. Without a
    // clear, piece 2's live text would cover both pieces' audio (sources() would catch that).
    let srv = fake_server(|c| if c.global == 1 { Act::CommitError } else { Act::Complete { after_ms: 10 } }, false).await;
    let h = harness(Some(&srv.url), quick_tuning());
    let rec = record(&h, &four_pieces());
    let (id, _) = process(&h, rec).await;
    let from = sources(&h, &id);
    assert_eq!(from[1], "file", "{from:?}");
    assert_eq!(*from.last().unwrap(), "live", "{from:?}");
    assert!(srv.log.lock().unwrap().clears >= 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn forced_cut_during_continuous_speech_streams_each_piece_exactly() {
    // 100 s without a pause: the segmenter cuts at its 45 s limit, at the quietest point in
    // the last 10 s (which is behind the live stream's position).
    let srv = fake_server(|_| Act::Complete { after_ms: 10 }, false).await;
    let h = harness(Some(&srv.url), quick_tuning());
    let rec = record(&h, &[(true, 100_000), (false, 400)]);
    let (id, _) = process(&h, rec).await;
    let rows = h.engine.db.segments(&id).unwrap();
    assert!(rows.len() >= 3, "pieces: {}", rows.len());
    let from = sources(&h, &id);
    assert!(from.iter().all(|f| *f == "live"), "{from:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_previous_item_id_keeps_live_working() {
    // If the server leaves previous_item_id out, commits still arrive in order, so live must
    // keep being used (no fallback), with every piece matching its own audio.
    let srv = fake_server(|_| Act::NoPrev, false).await;
    let h = harness(Some(&srv.url), quick_tuning());
    let rec = record(&h, &four_pieces());
    let (id, _) = process(&h, rec).await;
    let from = sources(&h, &id);
    assert!(from.iter().all(|f| *f == "live"), "{from:?}");
}

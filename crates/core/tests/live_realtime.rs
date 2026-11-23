//! Real-API check of live transcription (OpenAI Realtime). Costs well under a cent and needs a
//! key, so it's ignored by default. Run with:
//!   OPENAI_API_KEY=... cargo test -p wisper-core --test live_realtime -- --ignored --nocapture
//! It makes two short speech clips with OpenAI text-to-speech (set TTS_MODEL to override the
//! model), records them through a real Wisper session with live transcription on, and checks
//! the text came back over the live connection (not the file fallback) and in order.

use std::sync::Arc;
use std::time::{Duration, Instant};

use wisper_core::db::Db;
use wisper_core::hotkey::RecordingKind;
use wisper_core::pipeline::{ApiKeys, CaptureContext, Engine, Outcome, RealProviders};
use wisper_core::prompts::CleanupLevel;
use wisper_core::settings::Settings;

const RATE: u32 = 24_000;

async fn speech(key: &str, text: &str) -> Vec<f32> {
    let model = std::env::var("TTS_MODEL").unwrap_or_else(|_| "gpt-4o-mini-tts".into());
    let resp = reqwest::Client::new()
        .post("https://api.openai.com/v1/audio/speech")
        .bearer_auth(key)
        .json(&serde_json::json!({ "model": model, "voice": "alloy", "input": text, "response_format": "pcm" }))
        .send()
        .await
        .expect("TTS request");
    assert!(resp.status().is_success(), "TTS failed: {} {}", resp.status(), resp.text().await.unwrap_or_default());
    let bytes = resp.bytes().await.unwrap();
    bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn live_transcription_against_openai() {
    let Ok(key) = std::env::var("OPENAI_API_KEY") else {
        eprintln!("OPENAI_API_KEY not set; skipping");
        return;
    };
    let first = speech(&key, "Please send the quarterly report to Sarah by Thursday afternoon, and copy the finance team on it.").await;
    let second = speech(&key, "After that, remind everyone about the budget meeting next Wednesday morning.").await;

    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(&dir.path().join("wisper.db")).unwrap());
    let settings = Settings { cleanup_level: CleanupLevel::None, ..Default::default() };
    assert!(settings.providers.live_transcription);
    let keys = ApiKeys { openai: Some(key), anthropic: None };
    let engine = Engine::new(db, dir.path().to_path_buf(), settings, keys, Arc::new(RealProviders));

    let mut s = engine
        .start_session(RecordingKind::Dictation, CaptureContext::default(), RATE, 1, tokio::runtime::Handle::current())
        .unwrap();
    // Feed in real time (20 ms chunks), with a pause between the clips so they become two pieces.
    let silence = vec![0.0f32; RATE as usize * 3 / 2];
    let mut all = vec![0.0f32; RATE as usize / 4];
    all.extend(&first);
    all.extend(&silence);
    all.extend(&second);
    all.extend(&silence[..RATE as usize / 2]);
    let started = Instant::now();
    for (i, chunk) in all.chunks(RATE as usize / 50).enumerate() {
        s.push(chunk);
        let due = Duration::from_millis(20 * (i as u64 + 1));
        if let Some(wait) = due.checked_sub(started.elapsed()) {
            tokio::time::sleep(wait).await;
        }
    }
    let rec = s.finish();
    let id = rec.id.clone();
    let stopped = Instant::now();
    let out = engine.process(rec, &|_| {}).await.expect("processed");
    let waited = stopped.elapsed();
    let Outcome::Text(p) = out else { panic!("no speech found") };
    let item = engine.db.get(&id).unwrap().unwrap();
    let timings = item.timings.unwrap();
    let segments = engine.db.segments(&id).unwrap();
    eprintln!("text: {}", p.raw_text);
    eprintln!("pieces: {}  timings: {timings}", segments.len());
    eprintln!("wait after stop: {} ms", waited.as_millis());
    let lower = p.raw_text.to_lowercase();
    assert!(lower.contains("quarterly report") && lower.contains("budget meeting"), "{}", p.raw_text);
    assert!(lower.find("quarterly").unwrap() < lower.find("budget").unwrap(), "out of order: {}", p.raw_text);
    assert!(segments.len() >= 2, "expected two pieces");
    assert_eq!(timings["liveConnected"], true, "live connection failed: {timings}");
    assert_eq!(timings["fallbackPieces"], 0, "some pieces fell back to the file endpoint: {timings}");
    assert!(timings["livePieces"].as_u64().unwrap() >= 2);
}

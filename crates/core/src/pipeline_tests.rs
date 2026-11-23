use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::*;
use crate::apps;
use crate::providers::{ErrorKind, ProviderError, TextResponse, Transcript};

#[derive(Default)]
struct FakeStt {
    script: Mutex<VecDeque<Result<String, ProviderError>>>,
    requests: Mutex<Vec<TranscribeRequest>>,
    delay_ms: std::sync::atomic::AtomicU64,
}

#[async_trait]
impl Transcriber for FakeStt {
    async fn transcribe(&self, req: TranscribeRequest) -> Result<Transcript, ProviderError> {
        self.requests.lock().unwrap().push(req);
        let d = self.delay_ms.load(std::sync::atomic::Ordering::SeqCst);
        if d > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(d)).await;
        }
        let next = self.script.lock().unwrap().pop_front();
        match next {
            Some(Ok(t)) => Ok(Transcript { text: t, languages: vec![] }),
            Some(Err(e)) => Err(e),
            None => Ok(Transcript { text: "hello there".into(), languages: vec![] }),
        }
    }
}

#[derive(Default)]
struct FakeLlm {
    fail: Mutex<Option<ProviderError>>,
    /// Fails only requests for this model.
    fail_model: Mutex<Option<(String, ProviderError)>>,
    requests: Mutex<Vec<TextRequest>>,
}

fn between<'a>(s: &'a str, a: &str, b: &str) -> &'a str {
    let start = s.find(a).map(|i| i + a.len()).unwrap_or(0);
    let end = s[start..].find(b).map(|i| start + i).unwrap_or(s.len());
    &s[start..end]
}

#[async_trait]
impl TextModel for FakeLlm {
    async fn complete(&self, req: TextRequest) -> Result<TextResponse, ProviderError> {
        self.requests.lock().unwrap().push(req.clone());
        if let Some(e) = self.fail.lock().unwrap().clone() {
            return Err(e);
        }
        if let Some((m, e)) = self.fail_model.lock().unwrap().clone() {
            if m == req.model {
                return Err(e);
            }
        }
        let body = if req.user.contains("<instruction>") {
            format!("DONE: {}", between(&req.user, "<instruction>\n", "\n</instruction>"))
        } else {
            let t = between(&req.user, "<transcript>\n", "\n</transcript>").replace("um ", "");
            let mut t = crate::text::capitalise_first(t.trim());
            if !t.ends_with('.') {
                t.push('.');
            }
            t
        };
        Ok(TextResponse { text: body, input_tokens: 1, output_tokens: 1, cache_read_tokens: 0 })
    }
    fn provider_name(&self) -> &'static str {
        "Fake"
    }
}

struct Fakes {
    stt: Arc<FakeStt>,
    llm: Arc<FakeLlm>,
}

impl ProviderFactory for Fakes {
    fn transcriber(&self, _: &ApiKeys) -> Arc<dyn Transcriber> {
        self.stt.clone()
    }
    fn text_model(&self, _: &ApiKeys, _: CleanupProvider) -> Arc<dyn TextModel> {
        self.llm.clone()
    }
}

struct Harness {
    engine: Arc<Engine>,
    stt: Arc<FakeStt>,
    llm: Arc<FakeLlm>,
    _dir: tempfile::TempDir,
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(&dir.path().join("wisper.db")).unwrap());
    let stt = Arc::new(FakeStt::default());
    let llm = Arc::new(FakeLlm::default());
    let factory = Arc::new(Fakes { stt: stt.clone(), llm: llm.clone() });
    let keys = ApiKeys { openai: Some("sk-test".into()), anthropic: Some("sk-ant-test".into()) };
    let retry = RetryPolicy { attempts: 2, base_delay_ms: 5, max_delay_ms: 10 };
    let engine = Engine::new_with_retry(db, dir.path().to_path_buf(), Settings::default(), keys, factory, retry);
    Harness { engine, stt, llm, _dir: dir }
}

/// 48 kHz stereo: alternating "speech" (a buzzy tone) and silence, per (is_speech, ms).
fn audio48(pattern: &[(bool, u64)]) -> Vec<f32> {
    let mut out = Vec::new();
    let mut n = 0usize;
    for &(speech, ms) in pattern {
        for _ in 0..(ms as usize * 48) {
            let t = n as f32 / 48_000.0;
            let v = if speech {
                0.25 * ((2.0 * std::f32::consts::PI * 170.0 * t).sin() + 0.4 * (2.0 * std::f32::consts::PI * 510.0 * t).sin())
            } else {
                0.0005 * ((n * 7919 % 97) as f32 / 97.0 - 0.5)
            };
            out.push(v);
            out.push(v);
            n += 1;
        }
    }
    out
}

fn ctx(exe: &str, title: &str) -> CaptureContext {
    CaptureContext { app: apps::identify(exe, title), ..Default::default() }
}

fn record(h: &Harness, c: CaptureContext, pattern: &[(bool, u64)]) -> FinishedRecording {
    let mut s = h
        .engine
        .start_session(RecordingKind::Dictation, c, 48_000, 2, tokio::runtime::Handle::current())
        .unwrap();
    for chunk in audio48(pattern).chunks(960) {
        s.push(chunk);
    }
    s.finish()
}

fn no_progress() -> impl Fn(Stage) + Send + Sync {
    |_| {}
}

#[tokio::test(flavor = "multi_thread")]
async fn short_dictation_end_to_end() {
    let h = harness();
    h.stt.script.lock().unwrap().push_back(Ok("um we need parnassa and zchus".into()));
    let rec = record(&h, ctx("notepad.exe", "Untitled"), &[(false, 300), (true, 3000), (false, 400)]);
    let id = rec.id.clone();
    let out = h.engine.process(rec, &no_progress()).await.unwrap();
    let Outcome::Text(p) = out else { panic!("expected text") };
    assert_eq!(p.final_text, "We need parnasa and zechus.");
    let item = h.engine.db.get(&id).unwrap().unwrap();
    assert_eq!(item.status, "done");
    assert_eq!(item.raw_text.as_deref(), Some("um we need parnassa and zchus"));
    assert!(item.duration_ms >= 3500);
    // Audio was compressed for history (in the background, after the text was ready).
    let mut audio = item.audio_path.unwrap();
    for _ in 0..100 {
        if audio.ends_with(".flac") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        audio = h.engine.db.get(&id).unwrap().unwrap().audio_path.unwrap();
    }
    assert!(audio.ends_with(".flac"), "{audio}");
    assert!(std::path::Path::new(&audio).exists());
    // The speech model got the Hebrew/Yiddish hints.
    let req = h.stt.requests.lock().unwrap()[0].clone();
    assert!(req.keywords.iter().any(|k| k == "zechus"));
    assert_eq!(req.mime, "audio/flac");
    assert_eq!(&req.audio[..4], b"fLaC");
    // The cleanup model got the dictionary with sound-alike hints, cached.
    let llm_req = h.llm.requests.lock().unwrap()[0].clone();
    assert!(llm_req.system[0].cache);
    assert!(llm_req.system[0].text.contains("zechus (may come through as: Zeus"));
}

#[tokio::test(flavor = "multi_thread")]
async fn long_dictation_is_segmented_while_recording() {
    let h = harness();
    let mut pattern = vec![];
    for _ in 0..6 {
        pattern.push((true, 11_000));
        pattern.push((false, 1000));
    }
    let mut s = h
        .engine
        .start_session(RecordingKind::Dictation, ctx("notepad.exe", ""), 48_000, 2, tokio::runtime::Handle::current())
        .unwrap();
    // Feed audio in blocks with a short pause between, as real time would allow earlier
    // segments to finish transcribing before later ones start.
    for block in pattern.chunks(2) {
        for chunk in audio48(block).chunks(960) {
            s.push(chunk);
        }
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
    let rec = s.finish();
    assert!(rec.jobs.len() >= 5, "segments: {}", rec.jobs.len());
    let out = h.engine.process(rec, &no_progress()).await.unwrap();
    assert!(matches!(out, Outcome::Text(_)));
    assert!(h.stt.requests.lock().unwrap().len() >= 5);
    // Later segments get earlier text as context.
    let reqs = h.stt.requests.lock().unwrap();
    assert!(reqs.iter().skip(1).any(|r| r.prompt.as_deref().unwrap_or("").contains("hello there")));
}

#[tokio::test(flavor = "multi_thread")]
async fn silence_is_not_sent_anywhere() {
    let h = harness();
    let rec = record(&h, ctx("notepad.exe", ""), &[(false, 3000)]);
    let id = rec.id.clone();
    let out = h.engine.process(rec, &no_progress()).await.unwrap();
    assert!(matches!(out, Outcome::NoSpeech { .. }));
    assert!(h.stt.requests.lock().unwrap().is_empty());
    // Nothing was heard: the row and the audio are gone straight away.
    assert!(h.engine.db.get(&id).unwrap().is_none());
    assert!(h.engine.db.segments(&id).unwrap().is_empty());
    assert!(!h.engine.recordings_dir().join(format!("{id}.pcm")).exists());
    assert!(!h.engine.recordings_dir().join(format!("{id}.flac")).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_dictation_is_queued_then_processed_when_back() {
    let h = harness();
    let offline = ProviderError::new(ErrorKind::Network, "OpenAI", "dns");
    for _ in 0..4 {
        h.stt.script.lock().unwrap().push_back(Err(offline.clone()));
    }
    let rec = record(&h, ctx("notepad.exe", ""), &[(true, 2000), (false, 300)]);
    let id = rec.id.clone();
    let err = h.engine.process(rec, &no_progress()).await.unwrap_err();
    assert!(matches!(err, PipelineError::Queued { kind: ErrorKind::Network, .. }));
    assert!(err.message().contains("No internet"));
    let item = h.engine.db.get(&id).unwrap().unwrap();
    assert_eq!(item.status, "queued");
    assert!(item.audio_path.is_some());
    // Back online.
    h.stt.script.lock().unwrap().clear();
    h.stt.script.lock().unwrap().push_back(Ok("back online now".into()));
    let results = h.engine.process_queue().await;
    assert_eq!(results.len(), 1);
    let Ok(Outcome::Text(p)) = &results[0].1 else { panic!("{:?}", results[0].1) };
    assert_eq!(p.final_text, "Back online now.");
    assert_eq!(h.engine.db.get(&id).unwrap().unwrap().status, "done");
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_key_fails_without_retrying_and_keeps_audio() {
    let h = harness();
    h.stt.script.lock().unwrap().push_back(Err(ProviderError::new(ErrorKind::Auth, "OpenAI", "401")));
    let rec = record(&h, ctx("notepad.exe", ""), &[(true, 1500)]);
    let id = rec.id.clone();
    let err = h.engine.process(rec, &no_progress()).await.unwrap_err();
    assert!(matches!(err, PipelineError::Failed { kind: Some(ErrorKind::Auth), .. }));
    assert_eq!(h.stt.requests.lock().unwrap().len(), 1);
    let item = h.engine.db.get(&id).unwrap().unwrap();
    assert_eq!(item.status, "failed");
    assert_eq!(item.error_kind.as_deref(), Some("auth"));
    assert!(item.audio_path.is_some());
    // After the key is fixed, auth failures are re-queued and processed.
    assert_eq!(h.engine.requeue_auth_failures(), 1);
    let r = h.engine.process_queue().await;
    assert!(matches!(r[0].1, Ok(Outcome::Text(_))));
}

#[tokio::test(flavor = "multi_thread")]
async fn cleanup_failure_still_produces_text() {
    let h = harness();
    *h.llm.fail.lock().unwrap() = Some(ProviderError::new(ErrorKind::Auth, "Claude", "401"));
    h.stt.script.lock().unwrap().push_back(Ok("thank hashem new paragraph all good".into()));
    let rec = record(&h, ctx("notepad.exe", ""), &[(true, 1500)]);
    let Outcome::Text(p) = h.engine.process(rec, &no_progress()).await.unwrap() else { panic!() };
    assert_eq!(p.final_text, "Thank Hashem\n\nall good");
    assert!(p.cleanup_warning.unwrap().contains("Claude API key"));
}

#[tokio::test(flavor = "multi_thread")]
async fn spoken_mode_switch_and_app_styles() {
    let h = harness();
    h.stt.script.lock().unwrap().push_back(Ok("As an email, hi David, see you Sunday".into()));
    let rec = record(&h, ctx("notepad.exe", ""), &[(true, 1500)]);
    let Outcome::Text(p) = h.engine.process(rec, &no_progress()).await.unwrap() else { panic!() };
    assert_eq!(p.mode_id, "email");
    assert_eq!(p.final_text, "Hi David, see you Sunday.");
    let req = h.llm.requests.lock().unwrap().last().unwrap().clone();
    assert!(req.system[1].text.contains("Format for this dictation: Email"));
    assert!(!req.user.contains("As an email"));
    // Gmail in a browser defaults to Email; WhatsApp to Casual; Claude to AI Prompt.
    let (m, _) = h.engine.choose_mode("hi", &apps::identify("chrome.exe", "Inbox - Gmail - Google Chrome"), None);
    assert_eq!(m.id, "email");
    let (m, _) = h.engine.choose_mode("hi", &apps::identify("WhatsApp.exe", "WhatsApp"), None);
    assert_eq!(m.id, "casual");
    let (m, _) = h.engine.choose_mode("hi", &apps::identify("claude.exe", "Claude"), None);
    assert_eq!(m.id, "ai_prompt");
    // A user rule overrides.
    let mut s = h.engine.settings();
    s.app_styles.push(crate::settings::AppStyle { app_key: "whatsapp.exe".into(), app_name: "WhatsApp".into(), mode_id: "professional".into(), user_set: true });
    h.engine.set_settings(s);
    let (m, _) = h.engine.choose_mode("hi", &apps::identify("WhatsApp.exe", "WhatsApp"), None);
    assert_eq!(m.id, "professional");
}

#[tokio::test(flavor = "multi_thread")]
async fn snippets_survive_cleanup_verbatim() {
    let h = harness();
    h.engine.db.snippet_save(&crate::snippets::Snippet { id: 0, trigger: "my email address".into(), expansion: "Me@Example.com".into(), uses: 0 }).unwrap();
    h.stt.script.lock().unwrap().push_back(Ok("send it to my email address".into()));
    let rec = record(&h, ctx("notepad.exe", ""), &[(true, 1500)]);
    let Outcome::Text(p) = h.engine.process(rec, &no_progress()).await.unwrap() else { panic!() };
    assert_eq!(p.final_text, "Send it to Me@Example.com.");
    let req = h.llm.requests.lock().unwrap()[0].clone();
    assert!(req.user.contains('\u{27E6}'));
}

#[tokio::test(flavor = "multi_thread")]
async fn very_long_transcripts_are_cleaned_in_parallel_parts() {
    let h = harness();
    let long = "This is a fairly ordinary sentence about the plan. ".repeat(250);
    h.stt.script.lock().unwrap().push_back(Ok(long.clone()));
    let rec = record(&h, ctx("notepad.exe", ""), &[(true, 2000)]);
    let Outcome::Text(p) = h.engine.process(rec, &no_progress()).await.unwrap() else { panic!() };
    let calls = h.llm.requests.lock().unwrap().len();
    assert!(calls >= 2, "calls {calls}");
    assert_eq!(crate::text::word_count(&p.final_text), crate::text::word_count(&long));
    assert!(h.llm.requests.lock().unwrap()[1].user.contains("part 2 of"));
}

#[tokio::test(flavor = "multi_thread")]
async fn reprocess_in_another_mode_and_learns() {
    let h = harness();
    h.stt.script.lock().unwrap().push_back(Ok("eggs milk bread".into()));
    let rec = record(&h, ctx("slack.exe", "Slack"), &[(true, 1500)]);
    let Outcome::Text(p) = h.engine.process(rec, &no_progress()).await.unwrap() else { panic!() };
    assert_eq!(p.mode_id, "casual");
    let p2 = h.engine.reprocess(&p.id, "list").await.unwrap();
    assert_eq!(p2.mode_id, "list");
    assert_eq!(h.engine.db.get(&p.id).unwrap().unwrap().mode_id.as_deref(), Some("list"));
    // The STT wasn't called again.
    assert_eq!(h.stt.requests.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn crash_mid_recording_is_recovered_from_disk() {
    let h = harness();
    let mut s = h
        .engine
        .start_session(RecordingKind::Dictation, ctx("notepad.exe", ""), 48_000, 2, tokio::runtime::Handle::current())
        .unwrap();
    let id = s.id.clone();
    for chunk in audio48(&[(true, 2500), (false, 300)]).chunks(960) {
        s.push(chunk);
    }
    // Simulate a crash: the session is dropped without finish(); the file was flushed.
    std::mem::forget(s);
    std::thread::sleep(std::time::Duration::from_millis(300));
    h.engine.release(&id);
    assert_eq!(h.engine.db.get(&id).unwrap().unwrap().status, "recording");
    h.stt.script.lock().unwrap().push_back(Ok("recovered words".into()));
    let results = h.engine.process_queue().await;
    let (rid, r) = &results[0];
    assert_eq!(rid, &id);
    let Ok(Outcome::Text(p)) = r else { panic!("{r:?}") };
    assert_eq!(p.final_text, "Recovered words.");
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_keeps_real_recordings_and_drops_taps() {
    let h = harness();
    let mut s = h.engine.start_session(RecordingKind::Dictation, ctx("notepad.exe", ""), 48_000, 2, tokio::runtime::Handle::current()).unwrap();
    let tap_id = s.id.clone();
    s.push(&audio48(&[(true, 200)]));
    s.cancel();
    assert!(h.engine.db.get(&tap_id).unwrap().is_none());
    let mut s = h.engine.start_session(RecordingKind::Dictation, ctx("notepad.exe", ""), 48_000, 2, tokio::runtime::Handle::current()).unwrap();
    let id = s.id.clone();
    for chunk in audio48(&[(true, 2000)]).chunks(960) {
        s.push(chunk);
    }
    s.cancel();
    assert_eq!(h.engine.db.get(&id).unwrap().unwrap().status, "cancelled");
    // A long cancelled recording with no speech in it is dropped too.
    let mut s = h.engine.start_session(RecordingKind::Dictation, ctx("notepad.exe", ""), 48_000, 2, tokio::runtime::Handle::current()).unwrap();
    let quiet_id = s.id.clone();
    for chunk in audio48(&[(false, 2000)]).chunks(960) {
        s.push(chunk);
    }
    s.cancel();
    assert!(h.engine.db.get(&quiet_id).unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn command_mode_uses_instruction_and_selection() {
    let h = harness();
    h.stt.script.lock().unwrap().push_back(Ok("make this shorter".into()));
    let mut c = ctx("winword.exe", "Doc");
    c.selection = Some("A very long paragraph.".into());
    let mut s = h.engine.start_session(RecordingKind::Command, c, 48_000, 2, tokio::runtime::Handle::current()).unwrap();
    for chunk in audio48(&[(true, 1500)]).chunks(960) {
        s.push(chunk);
    }
    let rec = s.finish();
    let Outcome::Text(p) = h.engine.process(rec, &no_progress()).await.unwrap() else { panic!() };
    assert_eq!(p.final_text, "DONE: make this shorter");
    let req = h.llm.requests.lock().unwrap()[0].clone();
    assert!(req.user.contains("<selected_text>\nA very long paragraph.\n</selected_text>"));
}

#[tokio::test(flavor = "multi_thread")]
async fn retention_removes_old_audio_only() {
    let h = harness();
    let rec = record(&h, ctx("notepad.exe", ""), &[(true, 1500)]);
    let id = rec.id.clone();
    h.engine.process(rec, &no_progress()).await.unwrap();
    let mut s = h.engine.settings();
    s.keep_audio_days = Some(0);
    h.engine.set_settings(s);
    std::thread::sleep(std::time::Duration::from_millis(5));
    let (audio, items) = h.engine.apply_retention();
    assert_eq!((audio, items), (1, 0));
    let item = h.engine.db.get(&id).unwrap().unwrap();
    assert!(item.audio_path.is_none());
    assert!(item.final_text.is_some());
}

#[test]
fn cleanup_guard_accepts_normal_formatting() {
    // Email layout adds line breaks and keeps the spoken greeting/sign-off.
    assert!(looks_like_cleanup(
        "hi david thanks for sending the documents I'll review them tonight best asher",
        "Hi David,\n\nThanks for sending the documents. I'll review them tonight.\n\nBest,\nAsher"
    ));
    // AI Prompt mode may add a short heading and bullets.
    assert!(looks_like_cleanup(
        "write a python function that reads a csv file skips the header converts dates to iso and returns a list of dicts with type hints",
        "Write a Python function that:\n\nRequirements:\n- Reads a CSV file\n- Skips the header\n- Converts dates to ISO format\n- Returns a list of dictionaries\n- Uses type hints"
    ));
    // Heavy filler removal.
    assert!(looks_like_cleanup(
        "um so like you know I I think um we should uh maybe like go tomorrow you know",
        "I think we should maybe go tomorrow."
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn abandoned_job_releases_its_claim_so_the_queue_can_finish_it() {
    let h = harness();
    h.stt.delay_ms.store(3000, std::sync::atomic::Ordering::SeqCst);
    let rec = record(&h, ctx("notepad.exe", ""), &[(true, 1500)]);
    let id = rec.id.clone();
    assert!(h.engine.is_active(&id));
    // The caller gives up waiting (the controller's time limit).
    let r = tokio::time::timeout(std::time::Duration::from_millis(200), h.engine.process(rec, &no_progress())).await;
    assert!(r.is_err());
    assert!(!h.engine.is_active(&id), "claim must be released when the job is dropped");
    // The saved dictation is picked up by the queue later.
    h.stt.delay_ms.store(0, std::sync::atomic::Ordering::SeqCst);
    let results = h.engine.process_queue().await;
    assert!(results.iter().any(|(rid, r)| rid == &id && matches!(r, Ok(Outcome::Text(_)))), "{results:?}");
}

#[test]
fn cleanup_guard_catches_answers() {
    assert!(looks_like_cleanup("um so we need to go to the shop", "We need to go to the shop."));
    assert!(!looks_like_cleanup("what is the capital of france", "The capital of France is Paris. It is known for the Eiffel Tower, which was built in 1889 for the World's Fair and is one of the most visited monuments."));
    assert!(!looks_like_cleanup(&"word ".repeat(40), "Okay."));
    assert!(looks_like_cleanup("eggs milk bread", "- Eggs\n- Milk\n- Bread"));
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_transcript_with_speech_is_kept_and_can_be_transcribed_anyway() {
    let h = harness();
    // The speech detector heard speech, but the recogniser returned nothing (very quiet speech):
    // that is not "no speech", so it stays in History and can be retried.
    h.stt.script.lock().unwrap().push_back(Ok(String::new()));
    let rec = record(&h, ctx("notepad.exe", ""), &[(false, 300), (true, 2000), (false, 400)]);
    let id = rec.id.clone();
    assert!(matches!(h.engine.process(rec, &no_progress()).await.unwrap(), Outcome::NoSpeech { .. }));
    assert_eq!(h.engine.db.get(&id).unwrap().unwrap().status, "empty");
    // Wait for the background archive to finish.
    for _ in 0..50 {
        if h.engine.db.get(&id).unwrap().unwrap().audio_path.unwrap().ends_with(".flac") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    h.stt.script.lock().unwrap().push_back(Ok("very quiet words".into()));
    let r = h.engine.retry_dictation_with(&id, None, true).await.unwrap();
    let Outcome::Text(p) = r else { panic!("{r:?}") };
    assert_eq!(p.final_text, "Very quiet words.");
}

#[tokio::test(flavor = "multi_thread")]
async fn old_silent_rows_are_purged_but_ones_with_speech_stay() {
    let h = harness();
    let mk = |id: &str, status: &str| {
        let item = HistoryItem { id: id.into(), created_at: 1, kind: "dictation".into(), status: status.into(), ..Default::default() };
        h.engine.db.insert_dictation(&item).unwrap();
    };
    mk("silent", "empty");
    mk("heard", "empty");
    mk("failed", "failed");
    let seg = |id: &str, speech: bool| SegmentRow { dictation_id: id.into(), idx: 0, start_sample: 0, end_sample: 10, has_speech: speech, status: "skipped".into(), text: None, error: None };
    h.engine.db.upsert_segment(&seg("silent", false)).unwrap();
    h.engine.db.upsert_segment(&seg("heard", true)).unwrap();
    assert_eq!(h.engine.purge_silent_items(), 1);
    assert!(h.engine.db.get("silent").unwrap().is_none());
    assert!(h.engine.db.get("heard").unwrap().is_some());
    assert!(h.engine.db.get("failed").unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_recordings_with_speech_are_kept() {
    let h = harness();
    let err = ProviderError::new(ErrorKind::Auth, "OpenAI", "no key");
    for _ in 0..4 {
        h.stt.script.lock().unwrap().push_back(Err(err.clone()));
    }
    let rec = record(&h, ctx("notepad.exe", ""), &[(true, 2000), (false, 300)]);
    let id = rec.id.clone();
    assert!(h.engine.process(rec, &no_progress()).await.is_err());
    let item = h.engine.db.get(&id).unwrap().unwrap();
    assert_eq!(item.status, "failed");
    assert!(item.audio_path.is_some());
    // The history list can leave waiting items out (the UI shows them as one row).
    let shown = h.engine.db.list(&crate::db::HistoryQuery { exclude_status: Some(vec!["failed".into(), "queued".into()]), ..Default::default() }).unwrap();
    assert!(shown.iter().all(|i| i.id != id));
    let waiting = h.engine.db.list(&crate::db::HistoryQuery { status: Some("failed".into()), ..Default::default() }).unwrap();
    assert_eq!(waiting.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn prompt_preview_equals_what_cleanup_sends() {
    use crate::prompt_preview::{build, sample_for, PreviewRequest};
    let h = harness();
    let mut st = h.engine.settings();
    st.providers.fast_cleanup.skip_ai_for_tiny = false;
    h.engine.set_settings(st);
    for (mode, cat) in [("email", Some(apps::AppCategory::Email)), ("casual", Some(apps::AppCategory::PersonalChat)), ("auto", None)] {
        h.llm.requests.lock().unwrap().clear();
        let sample = sample_for(cat);
        // Same text the preview shows, after the (here no-op) dictionary pass.
        let r = h.engine.clean_text_for(&sample.transcript, &sample.ctx, Some(mode), None).await;
        assert!(r.error.is_none());
        let sent = h.llm.requests.lock().unwrap()[0].clone();
        let st = h.engine.settings();
        let entries = h.engine.db.dict_all().unwrap();
        let req = PreviewRequest { mode_id: mode.into(), app_category: cat, cleanup_level: None };
        let p = build(&st, &h.engine.modes(), &entries, &req, &sample).unwrap();
        assert_eq!(sent.system[0].text, p.base_system, "{mode}");
        assert_eq!(sent.system[1].text, p.mode_system, "{mode}");
        assert_eq!(sent.user, p.user_message, "{mode}");
        assert!(sent.system[0].text.contains("zechus (may come through as: Zeus"));
    }
}

// ---------------------------------------------------------------- fast model for short dictations

fn with_provider(provider: CleanupProvider) -> Settings {
    let mut s = Settings::default();
    s.providers.cleanup_provider = provider;
    s
}

#[test]
fn fast_model_routing_by_duration() {
    let s = Settings::default();
    assert_eq!(s.providers.fast_cleanup.max_seconds, 30);
    assert_eq!(dictation_model(&s, Some(29_999)), ("claude-haiku-5-5".to_string(), true));
    assert_eq!(dictation_model(&s, Some(30_000)), ("claude-sonnet-5-5".to_string(), false));
    assert_eq!(dictation_model(&s, Some(95_000)), ("claude-sonnet-5-5".to_string(), false));
    // Unknown length -> normal model.
    assert_eq!(dictation_model(&s, None), ("claude-sonnet-5-5".to_string(), false));
    assert_eq!(dictation_model(&s, Some(0)), ("claude-sonnet-5-5".to_string(), false));
}

#[test]
fn fast_model_can_be_disabled_or_retuned() {
    let mut s = Settings::default();
    s.providers.fast_cleanup.enabled = false;
    assert_eq!(dictation_model(&s, Some(5_000)), ("claude-sonnet-5-5".to_string(), false));
    s.providers.fast_cleanup.enabled = true;
    s.providers.fast_cleanup.max_seconds = 120;
    assert!(dictation_model(&s, Some(100_000)).1);
    s.providers.fast_cleanup.anthropic_model = "  ".into();
    assert_eq!(dictation_model(&s, Some(5_000)), ("claude-sonnet-5-5".to_string(), false));
}

#[test]
fn fast_model_follows_the_selected_provider() {
    let s = with_provider(CleanupProvider::OpenAi);
    assert_eq!(dictation_model(&s, Some(10_000)), ("gpt-6-luna".to_string(), true));
    assert_eq!(dictation_model(&s, Some(40_000)), ("gpt-6.2-sol".to_string(), false));
}

#[test]
fn old_settings_files_get_fast_cleanup_defaults() {
    let p: crate::settings::ProviderSettings = serde_json::from_str(r#"{"anthropicModel":"claude-opus-5-5"}"#).unwrap();
    assert!(p.fast_cleanup.enabled);
    assert_eq!(p.fast_cleanup.max_seconds, 30);
    assert_eq!(p.fast_cleanup.openai_model, "gpt-6-luna");
    let p: crate::settings::ProviderSettings = serde_json::from_str(r#"{"fastCleanup":{"maxSeconds":60}}"#).unwrap();
    assert_eq!(p.fast_cleanup.max_seconds, 60);
    assert_eq!(p.fast_cleanup.anthropic_model, "claude-haiku-5-5");
}

#[tokio::test(flavor = "multi_thread")]
async fn short_dictation_uses_fast_model_and_records_it() {
    let h = harness();
    h.stt.script.lock().unwrap().push_back(Ok("um we need milk".into()));
    let rec = record(&h, ctx("notepad.exe", ""), &[(false, 300), (true, 3000), (false, 400)]);
    let id = rec.id.clone();
    h.engine.process(rec, &no_progress()).await.unwrap();
    assert_eq!(h.llm.requests.lock().unwrap()[0].model, "claude-haiku-5-5");
    let item = h.engine.db.get(&id).unwrap().unwrap();
    assert_eq!(item.timings.as_ref().unwrap()["cleanupModel"], "claude-haiku-5-5");
    assert_eq!(item.timings.as_ref().unwrap()["cleanupFast"], true);
    assert_eq!(item.meta.as_ref().unwrap()["cleanupModel"], "claude-haiku-5-5");
}

#[tokio::test(flavor = "multi_thread")]
async fn redo_as_and_commands_use_the_normal_model() {
    let h = harness();
    h.stt.script.lock().unwrap().push_back(Ok("um we need milk".into()));
    let rec = record(&h, ctx("notepad.exe", ""), &[(false, 300), (true, 3000), (false, 400)]);
    let id = rec.id.clone();
    h.engine.process(rec, &no_progress()).await.unwrap();
    h.engine.reprocess(&id, "list").await.unwrap();
    assert_eq!(h.llm.requests.lock().unwrap().last().unwrap().model, "claude-sonnet-5-5");
    let item = h.engine.db.get(&id).unwrap().unwrap();
    assert_eq!(item.timings.as_ref().unwrap()["cleanupFast"], false);
    // Voice command / transform on a selection.
    let c = ctx("notepad.exe", "");
    h.engine.run_instruction("make it shorter", Some("some selected text"), &c).await.unwrap();
    assert_eq!(h.llm.requests.lock().unwrap().last().unwrap().model, "claude-sonnet-5-5");
}

#[tokio::test(flavor = "multi_thread")]
async fn tiny_dictation_skips_the_ai_only_when_switched_on() {
    // Off (the default): the AI is asked even for a tiny, clean sentence.
    let h = harness();
    h.stt.script.lock().unwrap().push_back(Ok("Sounds good to me.".into()));
    let rec = record(&h, ctx("notepad.exe", ""), &[(false, 300), (true, 1500), (false, 400)]);
    h.engine.process(rec, &no_progress()).await.unwrap();
    assert_eq!(h.llm.requests.lock().unwrap().len(), 1);

    // On: a tiny clean sentence is tidied locally; one with a filler still goes to the AI.
    let h = harness();
    let mut s = h.engine.settings();
    s.providers.fast_cleanup.skip_ai_for_tiny = true;
    h.engine.set_settings(s);
    h.stt.script.lock().unwrap().push_back(Ok("Sounds good to me.".into()));
    let rec = record(&h, ctx("notepad.exe", ""), &[(false, 300), (true, 1500), (false, 400)]);
    let id = rec.id.clone();
    h.engine.process(rec, &no_progress()).await.unwrap();
    assert_eq!(h.llm.requests.lock().unwrap().len(), 0, "no AI call for a tiny clean sentence");
    let item = h.engine.db.get(&id).unwrap().unwrap();
    assert_eq!(item.final_text.as_deref(), Some("Sounds good to me."));
    assert_eq!(item.timings.as_ref().unwrap()["cleanupModel"], "local");

    h.stt.script.lock().unwrap().push_back(Ok("Um, sounds good to me.".into()));
    let rec = record(&h, ctx("notepad.exe", ""), &[(false, 300), (true, 1500), (false, 400)]);
    h.engine.process(rec, &no_progress()).await.unwrap();
    assert_eq!(h.llm.requests.lock().unwrap().len(), 1, "a filler word still goes to the AI");
}

#[tokio::test(flavor = "multi_thread")]
async fn unavailable_fast_model_falls_back_to_normal_model() {
    let h = harness();
    *h.llm.fail_model.lock().unwrap() = Some(("claude-haiku-5-5".into(), ProviderError::new(ErrorKind::BadRequest, "Claude", "model: not found")));
    h.stt.script.lock().unwrap().push_back(Ok("um we need milk".into()));
    let rec = record(&h, ctx("notepad.exe", ""), &[(false, 300), (true, 3000), (false, 400)]);
    let id = rec.id.clone();
    let Outcome::Text(p) = h.engine.process(rec, &no_progress()).await.unwrap() else { panic!() };
    assert!(p.cleanup_warning.is_none());
    let models: Vec<_> = h.llm.requests.lock().unwrap().iter().map(|r| r.model.clone()).collect();
    assert_eq!(models, ["claude-haiku-5-5", "claude-sonnet-5-5"]);
    let t = h.engine.db.get(&id).unwrap().unwrap().timings.unwrap();
    assert_eq!(t["cleanupModel"], "claude-sonnet-5-5");
    assert_eq!(t["cleanupFast"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn fast_model_auth_or_offline_errors_do_not_retry_on_normal_model() {
    let h = harness();
    *h.llm.fail_model.lock().unwrap() = Some(("claude-haiku-5-5".into(), ProviderError::new(ErrorKind::Auth, "Claude", "401")));
    h.stt.script.lock().unwrap().push_back(Ok("um we need milk".into()));
    let rec = record(&h, ctx("notepad.exe", ""), &[(false, 300), (true, 3000), (false, 400)]);
    let Outcome::Text(p) = h.engine.process(rec, &no_progress()).await.unwrap() else { panic!() };
    assert!(p.cleanup_warning.is_some());
    assert_eq!(h.llm.requests.lock().unwrap().len(), 1);
}

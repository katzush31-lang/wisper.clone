//! "Tidy up while you talk": deterministic tests with the fake speech and text models.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::*;
use crate::db::HistoryItem;
use crate::hotkey::RecordingKind;
use crate::pipeline::tests::{audio48, between, ctx, harness, no_progress, Harness};
use crate::pipeline::Outcome;

const NAMES: &[&str] = &[
    "Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India", "Juliet", "Kilo", "Lima", "Mike", "November",
    "Oscar", "Papa", "Quebec", "Romeo", "Sierra", "Tango",
];

/// 13 words, one sentence.
fn sentence(k: usize) -> String {
    format!("{} went to the market today and bought some fresh bread and apples.", NAMES[k % NAMES.len()])
}

/// Pieces of two sentences (26 words), as the speech model returns them.
fn pieces(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("{} {}", sentence(2 * i), sentence(2 * i + 1))).collect()
}

/// Each piece is 10 seconds of audio.
const PIECE_SAMPLES: usize = 10 * 16_000;

fn tidy_for(h: &Harness, c: &CaptureContext) -> Arc<Tidy> {
    h.engine.start_tidy(c, tokio::runtime::Handle::current()).expect("tidy is on by default")
}

/// Feeds pieces in the given order, letting background work finish after each (talking time).
async fn feed(t: &Arc<Tidy>, p: &[String], order: &[usize]) {
    for &i in order {
        t.piece_done(i, (i + 1) * PIECE_SAMPLES, &p[i]);
        t.idle().await;
    }
}

async fn feed_all(t: &Arc<Tidy>, p: &[String]) {
    let order: Vec<usize> = (0..p.len()).collect();
    feed(t, p, &order).await;
}

fn transcripts(h: &Harness) -> Vec<String> {
    h.llm.requests.lock().unwrap().iter().map(|r| between(&r.user, "<transcript>\n", "\n</transcript>").to_string()).collect()
}

fn calls(h: &Harness) -> usize {
    h.llm.requests.lock().unwrap().len()
}

async fn stop(h: &Harness, t: &Arc<Tidy>, p: &[String], c: &CaptureContext) -> CleanResult {
    t.user_stopped();
    let raw = p.join(" ");
    h.engine.clean_dictation(&raw, c, Some(p.len() as u64 * 10_000), Some(t)).await
}

/// The whole-text path was used: one final call with the entire transcript.
fn assert_whole_text(h: &Harness, r: &CleanResult, p: &[String]) {
    let raw = p.join(" ");
    assert_eq!(r.chunked, 0);
    assert_eq!(transcripts(h).last().unwrap(), &raw, "the last call must be the whole text");
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(r.text, raw, "fake cleanup returns the text unchanged");
}

// ---------------------------------------------------------------------------- pure rules


#[test]
fn sentence_ends_skip_abbreviations_and_initials() {
    let s = "We met Mr. Cohen at 3.30 today. Did R. Levi come? Yes! \"Great.\" Done";
    let ends: Vec<&str> = sentence_ends(s).into_iter().map(|e| &s[..e]).collect();
    assert_eq!(ends, ["We met Mr. Cohen at 3.30 today.", "We met Mr. Cohen at 3.30 today. Did R. Levi come?", "We met Mr. Cohen at 3.30 today. Did R. Levi come? Yes!", "We met Mr. Cohen at 3.30 today. Did R. Levi come? Yes! \"Great.\""]);
}

#[test]
fn correction_cues_are_found() {
    for s in ["Let's meet Tuesday. No, wait, Wednesday.", "I mean the other one.", "Actually make it four.", "Sorry, it's Sunday.", "Scratch that.", "Let me rephrase.", "Forget that, we'll walk.", "Change that to ten."] {
        assert!(correction_cue(s).is_some(), "{s}");
    }
    for s in ["We will meet on Tuesday at the park.", "The weather was lovely and warm.", "Thank Hashem for everything."] {
        assert_eq!(correction_cue(s), None, "{s}");
    }
    assert_eq!(strong_cue("Scratch that, it's at home."), Some("scratch that"));
    assert_eq!(strong_cue("Actually it's at home."), None);
}

#[test]
fn spoken_lists_are_recognised() {
    assert!(looks_enumerated("Firstly the milk. Secondly the eggs."));
    assert!(looks_enumerated("First we go home, second we eat."));
    assert!(looks_enumerated("Number one is bread."));
    assert!(!looks_enumerated("We went home and then we ate dinner together."));
}

#[test]
fn cuts_at_the_latest_safe_sentence_end() {
    let p = pieces(3);
    let rest = p[..2].join(" ");
    // 52 words: the cut is at the very end (two pieces), not after 40 words in the middle.
    assert_eq!(choose_cut(&rest, &p[2]), Some(rest.len()));
    // Too short: no cut.
    assert_eq!(choose_cut(&p[0], &p[1]), None);
    // A correction anywhere after a possible cut (it may refer back across it): no cut before
    // it, however much text there is.
    let fix = "No wait, it was pears.";
    assert_eq!(choose_cut(&rest, fix), None);
    let rest3 = p.join(" ");
    assert_eq!(choose_cut(&rest3, fix), None);
    assert_eq!(choose_cut(&rest3, &format!("{} {} Actually Kilo bought pears.", sentence(9), sentence(10))), None);
    // A correction in the last words before a cut: no cut there either.
    let rest2 = format!("{} Sorry, it was Kilo.", p.join(" "));
    assert_eq!(choose_cut(&rest2, &pieces(4)[3]), None);
    // Once the correction is well inside the part, the part can end after it.
    let rest4 = format!("{} Sorry, Kilo came too. {} {}", p[0], p[1], p[2]);
    assert_eq!(choose_cut(&rest4, &pieces(4)[3]), Some(rest4.len()));
    // "Scratch that" anywhere later: no cut before it.
    assert_eq!(choose_cut(&rest, "Then lots more words here. Scratch that, none of it."), None);
}

#[test]
fn only_prose_modes_are_cleaned_in_parts() {
    let all = modes::all_modes(&Default::default(), &[]);
    let m = |id: &str| modes::find(&all, id).unwrap().clone();
    assert!(mode_allows_parts(&m("auto"), Some(AppCategory::Document)).is_ok());
    assert!(mode_allows_parts(&m("auto"), Some(AppCategory::PersonalChat)).is_ok());
    assert!(mode_allows_parts(&m("auto"), None).is_ok());
    assert!(mode_allows_parts(&m("professional"), None).is_ok());
    assert!(mode_allows_parts(&m("casual"), None).is_ok());
    for id in ["email", "list", "ai_prompt"] {
        assert!(mode_allows_parts(&m(id), None).is_err(), "{id}");
    }
    assert!(mode_allows_parts(&m("auto"), Some(AppCategory::Email)).is_err());
    assert!(mode_allows_parts(&m("auto"), Some(AppCategory::AiAssistant)).is_err());
    // Edited instructions or custom modes: only when they read as plain prose.
    let mut edited = m("casual");
    edited.instructions = "Turn it into bullet points.".into();
    assert!(mode_allows_parts(&edited, None).is_err());
    let custom = |instr: &str| Mode { id: "custom_x".into(), name: "X".into(), description: String::new(), instructions: instr.into(), builtin: false, aliases: vec![] };
    assert!(mode_allows_parts(&custom("Warm and friendly, in my own voice."), None).is_ok());
    assert!(mode_allows_parts(&custom("Format it as a numbered list."), None).is_err());
    assert!(mode_allows_parts(&custom("Keep it short and warm."), None).is_err());
}

// ---------------------------------------------------------------------------- the happy path

#[tokio::test(flavor = "multi_thread")]
async fn long_dictation_is_tidied_while_talking_and_only_the_tail_is_cleaned_at_stop() {
    let h = harness();
    let c = ctx("notepad.exe", "notes.txt - Notepad");
    let t = tidy_for(&h, &c);
    let p = pieces(8);
    feed_all(&t, &p).await;
    // Parts were cleaned while "talking": before any stop.
    let while_talking = transcripts(&h);
    assert_eq!(while_talking, [p[..3].join(" "), p[3..5].join(" "), p[5..7].join(" ")]);
    assert_eq!(t.stats().calls, 3);
    // Each part had the right context: the first is the start; later ones carry the tidied text.
    let reqs = h.llm.requests.lock().unwrap().clone();
    assert!(reqs[0].user.contains("start of a long dictation"));
    assert!(reqs[1].user.contains("part 2 of a long dictation"));
    assert!(reqs[1].user.contains("The dictation so far"));
    assert!(reqs[1].user.contains(&sentence(5)), "the tidied first part is the context");
    assert_eq!(reqs[0].system[0].text, reqs[2].system[0].text, "same cached system blocks");

    let r = stop(&h, &t, &p, &c).await;
    // Only the tail (the held-back last piece) was cleaned at stop.
    assert_eq!(calls(&h), 4);
    let reqs = h.llm.requests.lock().unwrap().clone();
    assert_eq!(between(&reqs[3].user, "<transcript>\n", "\n</transcript>"), p[7]);
    assert!(reqs[3].user.contains("final part"));
    assert!(reqs[3].user.contains(&sentence(13)), "context: the tidied text before the tail");
    // Every word was cleaned exactly once.
    let words: usize = transcripts(&h).iter().map(|s| text::word_count(s)).sum();
    assert_eq!(words, text::word_count(&p.join(" ")));
    // The result is the parts joined (a paragraph break between parts in a document).
    assert_eq!(r.chunked, 3);
    assert_eq!(r.tidy_calls, 3);
    assert!(r.tidy_fallback.is_none());
    assert!(r.error.is_none());
    assert_eq!(r.text, format!("{}\n\n{}\n\n{}\n\n{}", p[..3].join(" "), p[3..5].join(" "), p[5..7].join(" "), p[7]));
    assert_eq!(r.model.as_deref(), Some("claude-sonnet-5-5"));
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_parts_are_joined_with_a_space() {
    let h = harness();
    let c = ctx("WhatsApp.exe", "WhatsApp");
    let t = tidy_for(&h, &c);
    let p = pieces(6);
    feed_all(&t, &p).await;
    let r = stop(&h, &t, &p, &c).await;
    assert!(r.chunked >= 1);
    assert_eq!(r.mode_id, "casual");
    assert_eq!(r.text, p.join(" "));
}

#[tokio::test(flavor = "multi_thread")]
async fn short_dictations_are_never_tidied_in_parts() {
    let h = harness();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    // 26 words in 10 s, then 52 words in 20 s: still within the fast model's 30 seconds.
    let p = pieces(3);
    feed_all(&t, &p).await;
    assert_eq!(calls(&h), 0);
    t.user_stopped();
    let r = h.engine.clean_dictation(&p.join(" "), &c, Some(28_000), Some(&t)).await;
    assert_eq!(calls(&h), 1);
    assert!(r.fast, "the fast model cleans short dictations whole");
    assert!(r.tidy_fallback.is_none());
    assert_eq!(r.chunked, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn no_background_calls_when_off_without_a_key_or_for_commands() {
    let h = harness();
    let mut s = h.engine.settings();
    s.providers.clean_while_talking = false;
    h.engine.set_settings(s);
    assert!(h.engine.start_tidy(&ctx("notepad.exe", ""), tokio::runtime::Handle::current()).is_none());
    // Nor without a key for the cleanup provider, or with AI cleanup off.
    let h = harness();
    h.engine.set_keys(crate::pipeline::ApiKeys { openai: Some("sk".into()), anthropic: None });
    assert!(h.engine.start_tidy(&ctx("notepad.exe", ""), tokio::runtime::Handle::current()).is_none());
    let h = harness();
    let mut s = h.engine.settings();
    s.cleanup_level = CleanupLevel::None;
    h.engine.set_settings(s);
    assert!(h.engine.start_tidy(&ctx("notepad.exe", ""), tokio::runtime::Handle::current()).is_none());
    // A recording that becomes a voice command stops tidying.
    let h = harness();
    let mut s = h.engine.start_session(RecordingKind::Dictation, ctx("notepad.exe", ""), 48_000, 2, tokio::runtime::Handle::current()).unwrap();
    s.set_kind(RecordingKind::Command);
    assert!(s.tidy_stats().unwrap().off.unwrap().contains("voice command"));
    s.cancel();
}

#[tokio::test(flavor = "multi_thread")]
async fn pieces_finishing_out_of_order_are_put_back_in_order() {
    let h = harness();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let p = pieces(8);
    // Piece 0 arrives last of the first four: nothing can be tidied until it does.
    feed(&t, &p, &[1, 2, 3]).await;
    assert_eq!(calls(&h), 0);
    feed(&t, &p, &[0, 5, 4, 7, 6]).await;
    let r = stop(&h, &t, &p, &c).await;
    assert!(r.chunked >= 2, "{}", r.chunked);
    assert_eq!(r.text.replace("\n\n", " "), p.join(" "));
    // The parts were sent in order and cover the text once.
    assert_eq!(transcripts(&h).join(" "), p.join(" "));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_part_still_being_tidied_at_stop_runs_alongside_the_tail() {
    let h = harness();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let p = pieces(8);
    h.llm.delay_ms.store(2_000, Ordering::SeqCst);
    // Talking fast: no waiting for background work between pieces.
    for (i, x) in p.iter().enumerate() {
        t.piece_done(i, (i + 1) * PIECE_SAMPLES, x);
    }
    assert!(t.stats().in_flight);
    let started = std::time::Instant::now();
    let r = stop(&h, &t, &p, &c).await;
    // The tail didn't wait for the part in flight (2 s each, run side by side; one after the other would take 4 s).
    assert!(started.elapsed() < std::time::Duration::from_millis(3_500), "{:?}", started.elapsed());
    assert_eq!(r.chunked, 1);
    assert_eq!(r.text, format!("{}\n\n{}", p[..3].join(" "), p[3..].join(" ")));
    let reqs = h.llm.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2);
    // The tail's context includes the (not yet tidied) text of the part in flight.
    assert!(reqs[1].user.contains(&sentence(5)));
}

// ---------------------------------------------------------------------------- end to end

#[tokio::test(flavor = "multi_thread")]
async fn recording_tidies_while_talking_and_history_shows_it() {
    let h = harness();
    let p = pieces(7);
    for x in &p {
        h.stt.script.lock().unwrap().push_back(Ok(x.clone()));
    }
    let mut s = h.engine.start_session(RecordingKind::Dictation, ctx("notepad.exe", ""), 48_000, 2, tokio::runtime::Handle::current()).unwrap();
    for _ in 0..7 {
        for chunk in audio48(&[(true, 9_000), (false, 1_000)]).chunks(960) {
            s.push(chunk);
        }
        // Talking time: earlier pieces are transcribed and tidied meanwhile.
        std::thread::sleep(std::time::Duration::from_millis(80));
    }
    s.tidy.as_ref().unwrap().idle().await;
    let during = s.tidy_stats().unwrap();
    assert!(during.calls >= 1, "{during:?}");
    assert_eq!(calls(&h), during.calls, "the parts were cleaned while recording");
    let rec = s.finish();
    let id = rec.id.clone();
    let Outcome::Text(out) = h.engine.process(rec, &no_progress()).await.unwrap() else { panic!() };
    let item: HistoryItem = h.engine.db.get(&id).unwrap().unwrap();
    let raw = item.raw_text.clone().unwrap();
    assert_eq!(raw, p.join(" "), "the raw transcript is saved as always");
    assert_eq!(out.final_text.replace("\n\n", " "), raw);
    let tm = item.timings.unwrap();
    assert!(tm["chunked"].as_u64().unwrap() >= 1, "{tm}");
    assert_eq!(tm["chunkCalls"].as_u64().unwrap() as usize, during.calls);
    assert!(tm["chunkFallback"].is_null());
    assert!(tm["waitAfterStopMs"].is_u64());
    // Only the tail was cleaned after stop.
    assert_eq!(calls(&h), during.calls + 1);
    let tail = transcripts(&h).last().unwrap().clone();
    assert!(text::word_count(&tail) < text::word_count(&raw) / 2);

    // "Redo as" always cleans the whole text.
    h.engine.reprocess(&id, "professional").await.unwrap();
    assert_eq!(transcripts(&h).last().unwrap(), &raw);
    let tm = h.engine.db.get(&id).unwrap().unwrap().timings.unwrap();
    assert_eq!(tm["chunked"], 0);
    // So does a retry from History.
    h.engine.retry_dictation(&id, None).await.unwrap();
    assert_eq!(transcripts(&h).last().unwrap(), &raw);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_stops_background_tidying_and_keeps_the_recording() {
    let h = harness();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let p = pieces(8);
    h.llm.delay_ms.store(2_000, Ordering::SeqCst);
    for (i, x) in p.iter().take(4).enumerate() {
        t.piece_done(i, (i + 1) * PIECE_SAMPLES, x);
    }
    assert!(t.stats().in_flight);
    t.cancel();
    let st = t.stats();
    assert!(!st.in_flight);
    assert_eq!(st.parts_done, 0);
    // Later pieces do nothing.
    t.piece_done(4, 5 * PIECE_SAMPLES, &p[4]);
    assert_eq!(t.stats().calls, 1);
    // A recording cancelled mid-way is kept in History as before.
    h.llm.delay_ms.store(0, Ordering::SeqCst);
    let mut s = h.engine.start_session(RecordingKind::Dictation, ctx("notepad.exe", ""), 48_000, 2, tokio::runtime::Handle::current()).unwrap();
    let id = s.id.clone();
    for chunk in audio48(&[(true, 3_000)]).chunks(960) {
        s.push(chunk);
    }
    s.cancel();
    assert_eq!(h.engine.db.get(&id).unwrap().unwrap().status, "cancelled");
}

// ---------------------------------------------------------------------------- fallbacks to the whole text

#[tokio::test(flavor = "multi_thread")]
async fn correction_in_the_tail_cleans_the_whole_text() {
    let h = harness();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let mut p = pieces(8);
    p[7] = format!("{} No wait, make that Thursday.", sentence(14));
    feed_all(&t, &p).await;
    // The third part would end right before the correction, so it is not tidied at all.
    assert_eq!(t.stats().calls, 2);
    let r = stop(&h, &t, &p, &c).await;
    assert_whole_text(&h, &r, &p);
    assert!(r.tidy_fallback.as_deref().unwrap().contains("correction near the end"), "{:?}", r.tidy_fallback);
    assert_eq!(r.tidy_calls, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn correction_right_after_a_tidied_part_cleans_the_whole_text() {
    let h = harness();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let mut p = pieces(8);
    // A short piece, then (after a pause) a correction of something in the first part, which
    // was already being tidied when the correction was said.
    p[3] = "Kilo agreed.".into();
    p[4] = format!("Sorry, I mean pears not apples. {}", sentence(9));
    feed_all(&t, &p).await;
    let st = t.stats();
    assert_eq!(st.calls, 1, "no more parts once a part may be changed");
    assert!(st.off.as_deref().unwrap().contains("correction"));
    let r = stop(&h, &t, &p, &c).await;
    assert_whole_text(&h, &r, &p);
}

#[tokio::test(flavor = "multi_thread")]
async fn scratch_that_after_the_first_part_cleans_the_whole_text() {
    let h = harness();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let mut p = pieces(8);
    // Deep inside a later part, far from any boundary.
    p[4] = format!("{} Scratch that about Delta, it was someone else.", sentence(8));
    feed_all(&t, &p).await;
    let r = stop(&h, &t, &p, &c).await;
    assert_whole_text(&h, &r, &p);
    assert!(r.tidy_fallback.as_deref().unwrap().contains("scratch that"), "{:?}", r.tidy_fallback);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_short_correction_anywhere_after_the_first_part_cleans_the_whole_text() {
    for cue in ["Actually, Delta bought pears.", "Sorry, it was Echo who went.", "I mean the other market.", "Make it Thursday instead."] {
        let h = harness();
        let c = ctx("notepad.exe", "");
        let t = tidy_for(&h, &c);
        let mut p = pieces(8);
        // Deep inside a later piece, more than 12 words after any part boundary; it may refer
        // back to something already tidied.
        p[4] = format!("{} {cue}", sentence(8));
        feed_all(&t, &p).await;
        let st = t.stats();
        assert_eq!(st.calls, 1, "{cue}");
        assert!(st.off.as_deref().unwrap().contains("correction after a tidied part"), "{cue}: {st:?}");
        let r = stop(&h, &t, &p, &c).await;
        assert_whole_text(&h, &r, &p);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn context_around_the_cursor_at_stop_still_uses_the_parts() {
    // Context awareness on (default). The tidier started without the field's text; at stop
    // the text around the cursor is known (as on Windows, where it's read after the start).
    let h = harness();
    let start = ctx("notepad.exe", "");
    let t = tidy_for(&h, &start);
    let p = pieces(8);
    feed_all(&t, &p).await;
    t.user_stopped();
    let mut at_stop = start.clone();
    at_stop.text_before = Some("Notes from today:".into());
    at_stop.text_after = Some("more to follow".into());
    let raw = p.join(" ");
    let r = h.engine.clean_dictation(&raw, &at_stop, Some(80_000), Some(&t)).await;
    assert_eq!(r.chunked, 3, "{:?}", r.tidy_fallback);
    // Fitted to the stop-time context exactly as the whole-text path does: a space after the
    // colon, and the final full stop dropped before the lowercase text after the cursor.
    let joined = format!("{}\n\n{}\n\n{}\n\n{}", p[..3].join(" "), p[3..5].join(" "), p[5..7].join(" "), p[7]);
    assert_eq!(r.text, crate::text::fit_to_context(&joined, Some("Notes from today:"), Some("more to follow")));
    assert!(r.text.starts_with(" Alpha"));
    assert!(r.text.ends_with("apples "));
    // Empty fields (UI Automation reports Some("") for them) don't matter either.
    let h = harness();
    let t = tidy_for(&h, &start);
    feed_all(&t, &p).await;
    t.user_stopped();
    let mut empty = start.clone();
    empty.text_before = Some(String::new());
    empty.text_after = Some(String::new());
    let r = h.engine.clean_dictation(&raw, &empty, Some(80_000), Some(&t)).await;
    assert_eq!(r.chunked, 3, "{:?}", r.tidy_fallback);
}

#[tokio::test(flavor = "multi_thread")]
async fn field_context_arriving_after_the_start_reaches_the_first_part_and_the_result() {
    let h = harness();
    let p = pieces(7);
    for x in &p {
        h.stt.script.lock().unwrap().push_back(Ok(x.clone()));
    }
    let mut s = h.engine.start_session(RecordingKind::Dictation, ctx("notepad.exe", "notes.txt - Notepad"), 48_000, 2, tokio::runtime::Handle::current()).unwrap();
    for i in 0..7 {
        for chunk in audio48(&[(true, 9_000), (false, 1_000)]).chunks(960) {
            s.push(chunk);
        }
        if i == 0 {
            // UI Automation has read the field (the controller passes it on within ~1 s).
            s.set_field_context(Some("Shopping notes:".into()), Some(String::new()));
        }
        std::thread::sleep(std::time::Duration::from_millis(80));
    }
    s.tidy.as_ref().unwrap().idle().await;
    let during = s.tidy_stats().unwrap();
    assert!(during.calls >= 1, "{during:?}");
    let first = h.llm.requests.lock().unwrap()[0].user.clone();
    assert!(first.contains("Text already in the field"), "the first part gets the text before the cursor: {first}");
    assert!(first.contains("Shopping notes:"));
    let mut rec = s.finish();
    // After stop, the controller merges what it read (here: the field changed meanwhile).
    rec.ctx.text_before = Some("Shopping notes for Sunday:".into());
    rec.ctx.text_after = Some(String::new());
    let id = rec.id.clone();
    let Outcome::Text(out) = h.engine.process(rec, &no_progress()).await.unwrap() else { panic!() };
    let item = h.engine.db.get(&id).unwrap().unwrap();
    let tm = item.timings.unwrap();
    assert!(tm["chunked"].as_u64().unwrap() >= 1, "{tm}");
    assert!(tm["chunkFallback"].is_null(), "{tm}");
    assert!(out.final_text.starts_with(" Alpha"), "fitted after the colon: {:?}", out.final_text);
    assert_eq!(out.final_text.trim().replace("\n\n", " "), item.raw_text.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn code_and_unknown_apps_join_parts_with_a_space() {
    for (exe, title) in [("code.exe", "main.rs - VS Code"), ("someapp.exe", "Some App"), ("chrome.exe", "Some page - Google Chrome")] {
        let h = harness();
        let c = ctx(exe, title);
        let t = tidy_for(&h, &c);
        let p = pieces(8);
        feed_all(&t, &p).await;
        let r = stop(&h, &t, &p, &c).await;
        assert_eq!(r.chunked, 3, "{exe}");
        assert_eq!(r.text, p.join(" "), "{exe}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_tail_does_not_ask_the_ai_again_for_the_whole_text() {
    let h = harness();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let p = pieces(8);
    *h.llm.fail_if.lock().unwrap() = Some((sentence(14), sentence(0), ProviderError::new(ErrorKind::Network, "Fake", "offline")));
    feed_all(&t, &p).await;
    let r = stop(&h, &t, &p, &c).await;
    // The tail was tried (with the usual retries), then the text was finished without the AI,
    // exactly as the whole-text path does after its own failure; no whole-text AI call.
    let raw = p.join(" ");
    assert!(transcripts(&h).iter().all(|s| s != &raw), "no second wait for the whole text");
    assert_eq!(r.error.as_ref().map(|e| e.kind), Some(ErrorKind::Network));
    assert_eq!(r.chunked, 0);
    assert!(r.tidy_fallback.as_deref().unwrap().contains("last part"));
    assert_eq!(r.text, raw, "the transcript with the deterministic fixes, nothing lost");
    let h2 = harness();
    *h2.llm.fail.lock().unwrap() = Some(ProviderError::new(ErrorKind::Network, "Fake", "offline"));
    let whole = h2.engine.clean_text_for(&raw, &c, None, Some(80_000)).await;
    assert_eq!(r.text, whole.text);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_part_failing_with_a_bad_key_does_not_ask_the_ai_again_at_stop() {
    let h = harness();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let p = pieces(8);
    *h.llm.fail_if.lock().unwrap() = Some((sentence(6), sentence(0), ProviderError::new(ErrorKind::Auth, "Fake", "401")));
    feed_all(&t, &p).await;
    assert_eq!(t.stats().calls, 2);
    let r = stop(&h, &t, &p, &c).await;
    assert_eq!(calls(&h), 2, "nothing more is sent after a key failure");
    assert_eq!(r.error.as_ref().map(|e| e.kind), Some(ErrorKind::Auth));
    assert_eq!(r.text, p.join(" "));
}

#[tokio::test(flavor = "multi_thread")]
async fn whole_text_modes_are_never_tidied_in_parts() {
    // Email (Gmail), AI Prompt (Claude), List (chosen as the default), and a custom mode that
    // changes the layout.
    let cases: Vec<(CaptureContext, Option<&str>)> = vec![
        (ctx("chrome.exe", "Inbox - Gmail - Google Chrome"), None),
        (ctx("claude.exe", "Claude"), None),
        (ctx("notepad.exe", ""), Some("list")),
        (ctx("notepad.exe", ""), Some("custom_minutes")),
    ];
    for (c, default_mode) in cases {
        let h = harness();
        let mut s = h.engine.settings();
        s.custom_modes.push(Mode {
            id: "custom_minutes".into(),
            name: "Minutes".into(),
            description: String::new(),
            instructions: "Write it up as meeting minutes with a heading per topic.".into(),
            builtin: false,
            aliases: vec![],
        });
        if let Some(m) = default_mode {
            s.default_mode = m.into();
        }
        h.engine.set_settings(s);
        let t = tidy_for(&h, &c);
        let p = pieces(8);
        feed_all(&t, &p).await;
        let st = t.stats();
        assert_eq!(st.calls, 0, "{:?} {default_mode:?}", c.app.name);
        assert!(st.off.as_deref().unwrap().contains("mode"), "{st:?}");
        let r = stop(&h, &t, &p, &c).await;
        assert_eq!(calls(&h), 1);
        assert_eq!(transcripts(&h)[0], p.join(" "));
        assert_eq!(r.chunked, 0);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prose_custom_mode_is_tidied_in_parts() {
    let h = harness();
    let mut s = h.engine.settings();
    s.custom_modes.push(Mode {
        id: "custom_warm".into(),
        name: "Warm".into(),
        description: String::new(),
        instructions: "Warm and friendly, in my own voice.".into(),
        builtin: false,
        aliases: vec![],
    });
    s.default_mode = "custom_warm".into();
    h.engine.set_settings(s);
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let p = pieces(8);
    feed_all(&t, &p).await;
    let r = stop(&h, &t, &p, &c).await;
    assert_eq!(r.mode_id, "custom_warm");
    assert_eq!(r.chunked, 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn spoken_mode_switch_cleans_the_whole_text() {
    let h = harness();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let mut p = pieces(8);
    p[0] = format!("As a casual message, {}", p[0]);
    feed_all(&t, &p).await;
    assert_eq!(t.stats().calls, 0);
    let r = stop(&h, &t, &p, &c).await;
    assert_eq!(r.mode_id, "casual");
    assert_eq!(calls(&h), 1);
    assert!(!transcripts(&h)[0].contains("As a casual message"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_spoken_list_cleans_the_whole_text() {
    let h = harness();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let mut p = pieces(8);
    // Becomes a list after two parts were already tidied.
    p[5] = format!("Firstly, {}", p[5]);
    p[6] = format!("Secondly, {}", p[6]);
    feed_all(&t, &p).await;
    assert_eq!(t.stats().calls, 2);
    let r = stop(&h, &t, &p, &c).await;
    assert_whole_text(&h, &r, &p);
    assert!(r.tidy_fallback.as_deref().unwrap().contains("list"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_part_cleans_the_whole_text_and_nothing_is_lost() {
    let h = harness();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let p = pieces(8);
    // The second part fails (bad request, not retried); the whole text later succeeds.
    *h.llm.fail_if.lock().unwrap() = Some((sentence(6), sentence(0), ProviderError::new(ErrorKind::BadRequest, "Fake", "boom")));
    feed_all(&t, &p).await;
    let st = t.stats();
    assert_eq!(st.calls, 2);
    assert!(st.off.as_deref().unwrap().contains("couldn't be tidied"));
    let r = stop(&h, &t, &p, &c).await;
    assert_whole_text(&h, &r, &p);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_part_that_fails_the_answer_guard_cleans_the_whole_text() {
    let h = harness();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let p = pieces(8);
    *h.llm.reply_if.lock().unwrap() = Some((sentence(0), sentence(15), "Sure, here is a poem about markets.".into()));
    feed_all(&t, &p).await;
    assert!(t.stats().off.as_deref().unwrap().contains("didn't look like"));
    let r = stop(&h, &t, &p, &c).await;
    assert_whole_text(&h, &r, &p);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_tail_cleans_the_whole_text() {
    let h = harness();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let p = pieces(8);
    *h.llm.fail_if.lock().unwrap() = Some((sentence(14), sentence(0), ProviderError::new(ErrorKind::BadRequest, "Fake", "boom")));
    feed_all(&t, &p).await;
    let r = stop(&h, &t, &p, &c).await;
    assert_whole_text(&h, &r, &p);
    assert!(r.tidy_fallback.as_deref().unwrap().contains("last part"));
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_changed_while_talking_cleans_the_whole_text() {
    let h = harness();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let p = pieces(8);
    feed_all(&t, &p).await;
    let mut s = h.engine.settings();
    s.cleanup_level = CleanupLevel::Medium;
    h.engine.set_settings(s);
    let r = stop(&h, &t, &p, &c).await;
    assert_whole_text(&h, &r, &p);
    assert!(r.tidy_fallback.as_deref().unwrap().contains("changed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_transcript_cleans_the_whole_text() {
    let h = harness();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let p = pieces(8);
    feed_all(&t, &p).await;
    t.user_stopped();
    let mut other = p.clone();
    other[1] = sentence(19);
    let raw = other.join(" ");
    let r = h.engine.clean_dictation(&raw, &c, Some(80_000), Some(&t)).await;
    assert_eq!(transcripts(&h).last().unwrap(), &raw);
    assert_eq!(r.text, raw);
    assert_eq!(r.chunked, 0);
}

// ---------------------------------------------------------------------------- snippets and dictionary

#[tokio::test(flavor = "multi_thread")]
async fn snippets_and_dictionary_work_across_parts_exactly_as_for_the_whole_text() {
    let h = harness();
    h.engine.db.snippet_save(&crate::snippets::Snippet { id: 0, trigger: "my email address".into(), expansion: "Me@Example.com".into(), uses: 0 }).unwrap();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let mut p = pieces(8);
    // A snippet and a misspelt Hebrew word in the first part and again in the tail.
    p[1] = format!("Write to my email address for the zchus of it. {}", sentence(3));
    p[7] = format!("Again write to my email address for the zchus of it. {}", sentence(15));
    feed_all(&t, &p).await;
    let r = stop(&h, &t, &p, &c).await;
    assert_eq!(r.chunked, 3);
    // The tail used its own placeholder number, and the result matches the whole-text path.
    let reqs = h.llm.requests.lock().unwrap().clone();
    assert!(reqs[0].user.contains("\u{27E6}1\u{27E7}"));
    assert!(reqs[3].user.contains("\u{27E6}2\u{27E7}"));
    assert!(!reqs[3].user.contains("\u{27E6}1\u{27E7}"), "earlier context is shown with the snippet text");
    let h2 = harness();
    h2.engine.db.snippet_save(&crate::snippets::Snippet { id: 0, trigger: "my email address".into(), expansion: "Me@Example.com".into(), uses: 0 }).unwrap();
    let whole = h2.engine.clean_text_for(&p.join(" "), &c, None, Some(80_000)).await;
    assert_eq!(r.text.replace("\n\n", " "), whole.text);
    assert!(r.text.contains("Write to Me@Example.com for the zechus of it."));
    assert!(r.text.contains("Again write to Me@Example.com for the zechus of it."));
    assert_eq!(r.fixes, whole.fixes);
    // Used twice, counted twice (as the whole-text path does).
    assert_eq!(h.engine.db.snippets_all().unwrap()[0].uses, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snippet_at_a_part_boundary_cleans_the_whole_text() {
    let h = harness();
    h.engine.db.snippet_save(&crate::snippets::Snippet { id: 0, trigger: "my email address".into(), expansion: "Me@Example.com".into(), uses: 0 }).unwrap();
    let c = ctx("notepad.exe", "");
    let t = tidy_for(&h, &c);
    let mut p = pieces(8);
    // The first part ends with the snippet ("… to my email address."): alone, the trailing
    // full stop would be swallowed with it, so the parts wouldn't match the whole.
    p[2] = format!("{} Send everything to my email address.", sentence(4));
    feed_all(&t, &p).await;
    assert!(t.stats().calls >= 1);
    let r = stop(&h, &t, &p, &c).await;
    assert_eq!(r.chunked, 0);
    assert!(r.tidy_fallback.as_deref().unwrap().contains("snippet"), "{:?}", r.tidy_fallback);
    assert!(r.text.contains("Send everything to Me@Example.com."));
}

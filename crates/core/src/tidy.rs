//! "Tidy up while you talk": long dictations are cleaned up in parts while the user is still
//! talking, so when they stop only the last part needs the AI.
//!
//! How it works:
//! - Each transcribed piece is handed to `Tidy` as it finishes (in any order); pieces are put
//!   back in order, and only the text before the most recent piece with words is used, so the
//!   last thing said is never tidied before the user stops (they might correct themselves).
//! - Once the dictation is getting long, finished sentences are grouped into parts (at least
//!   `MIN_PART_WORDS` words, ending at a sentence end) and cleaned one at a time in the
//!   background, each with the tidied text before it as context. Same mode, app, dictionary,
//!   snippets, level and (main) model as a whole-text cleanup; same cached system blocks.
//! - At stop, only the rest (the tail) is cleaned, then everything is joined and finished
//!   exactly like the whole-text path (dictionary pass, snippets, fitting to the cursor).
//!
//! It is only an accelerator. The raw transcript is saved as always, and the normal whole-text
//! cleanup (today's path, unchanged) is used whenever the parts might not join up exactly:
//! a possible self-correction ("actually", "I mean", "scratch that", …) anywhere after the
//! first part, a mode whose layout depends on the whole text (Email, List, AI Prompt, custom
//! modes that change layout, Auto in email/AI apps), a spoken mode switch, something that
//! sounds like a list, settings/dictionary changed while talking, a snippet or dictionary
//! phrase across a boundary, any part failing (or failing the answer guard), and short
//! dictations (the fast model cleans those whole). Retries, "redo as" and the offline queue
//! never use parts.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::{
    cleanup_context_for, cleanup_model, cleanup_system_blocks, dictation_model, join_segments, looks_like_cleanup, max_tokens_for,
    CaptureContext, CleanResult, Engine,
};
use crate::apps::AppCategory;
use crate::audio;
use crate::dictionary::{self, DictEntry};
use crate::modes::{self, Mode};
use crate::prompts::{self, CleanupLevel, TalkingPart};
use crate::providers::{ErrorKind, ProviderError, SystemBlock, TextRequest};
use crate::settings::CleanupProvider;
use crate::snippets::{self, Snippet};
use crate::text;

/// Parts start once this much speech (by the end of the usable text) has been transcribed…
pub const START_AFTER_MS: u64 = 25_000;
/// …or this many words, whichever comes first (and never for recordings short enough for the
/// fast model, which cleans them whole).
pub const START_AFTER_WORDS: usize = 60;
/// Smallest part sent on its own.
pub const MIN_PART_WORDS: usize = 40;
/// How many words either side of a part boundary are checked for correction cues.
const CUE_WINDOW_WORDS: usize = 12;

/// Words that often take back something said just before ("Tuesday, no wait, Wednesday").
const WEAK_CUES: &[&str] = &[
    "no wait", "wait no", "wait wait", "no no", "i mean", "i meant", "actually", "sorry", "or rather", "change that",
    "make that", "make it", "correction", "oops", "hold on", "not that", "rephrase",
];
/// Phrases that can take back something said much earlier.
const STRONG_CUES: &[&str] = &[
    "scratch that", "scratch all that", "scratch everything", "forget that", "forget it", "forget what i said",
    "forget everything", "forget all that", "let me rephrase", "let me start over", "let me start again", "start over",
    "start again", "ignore that", "ignore what i said", "ignore everything", "delete that", "delete all that", "strike that",
    "never mind", "nevermind", "take that back", "disregard", "cancel that", "change that to", "instead of what i said",
    "go back and",
];
/// Markers of a spoken list, whose layout depends on all of it.
const LIST_MARKERS: &[&str] = &[
    "firstly", "secondly", "thirdly", "number one", "number two", "number 1", "number 2", "bullet point", "bullet points",
    "point one", "point two", "step one", "step two", "item one", "item two", "next bullet", "next item",
];
/// Words in a custom mode's instructions that suggest it changes the layout.
const LAYOUT_WORDS: &[&str] = &[
    "list", "bullet", "numbered", "email", "e-mail", "greeting", "sign-off", "sign off", "signoff", "heading", "title",
    "subject", "summar", "short", "concise", "translat", "table", "format", "structure", "organis", "organiz", "prompt",
    "outline", "section", "letter", "dear ", "regards", "reorder", "rearrange", "paragraph", "bold", "markdown", "line",
];
const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "st", "vs", "etc", "e.g", "i.e", "no", "rav", "prof", "jr", "sr", "approx", "mt", "ft", "lt",
    "rd", "ave", "inc", "ltd", "co", "ca", "fig", "vol", "rabbi",
];

// ---------------------------------------------------------------------------- text checks

/// Lowercase words with surrounding punctuation removed ("Wait," -> "wait").
fn words(s: &str) -> Vec<String> {
    s.split_whitespace()
        .map(|w| {
            w.replace(['\u{2019}', '\u{2018}'], "'")
                .trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
                .trim_matches('\'')
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

fn find_phrase(ws: &[String], phrase: &str) -> bool {
    let p: Vec<&str> = phrase.split_whitespace().collect();
    !p.is_empty() && ws.windows(p.len()).any(|w| w.iter().zip(&p).all(|(a, b)| a == b))
}

fn find_cue(ws: &[String], cues: &[&'static str]) -> Option<&'static str> {
    cues.iter().copied().find(|c| find_phrase(ws, c))
}

/// A self-correction cue anywhere in the text (weak or strong).
pub fn correction_cue(text: &str) -> Option<&'static str> {
    let ws = words(text);
    find_cue(&ws, STRONG_CUES).or_else(|| find_cue(&ws, WEAK_CUES))
}

/// A cue that can take back something said long before ("scratch that").
pub fn strong_cue(text: &str) -> Option<&'static str> {
    find_cue(&words(text), STRONG_CUES)
}

fn cue_in_last_words(text: &str, n: usize) -> Option<&'static str> {
    let ws = words(text);
    let ws = &ws[ws.len().saturating_sub(n)..];
    find_cue(ws, STRONG_CUES).or_else(|| find_cue(ws, WEAK_CUES))
}

/// Sounds like a spoken list ("firstly…", "number one…", "first… second…").
pub fn looks_enumerated(text: &str) -> bool {
    let ws = words(text);
    LIST_MARKERS.iter().any(|m| find_phrase(&ws, m)) || (find_phrase(&ws, "first") && find_phrase(&ws, "second"))
}

fn is_abbreviation(before: &str) -> bool {
    let w: String = before.chars().rev().take_while(|c| c.is_alphanumeric() || *c == '.').collect::<Vec<_>>().into_iter().rev().collect();
    let w = w.trim_start_matches('.').to_lowercase();
    let single_letter = w.chars().count() == 1 && w.chars().all(char::is_alphabetic);
    single_letter || ABBREVIATIONS.contains(&w.as_str())
}

/// Byte offsets just after each sentence end ("." "!" "?" "…", plus closing quotes or
/// brackets) that is followed by whitespace or the end of the text. "Mr." and initials are
/// not sentence ends.
pub fn sentence_ends(s: &str) -> Vec<usize> {
    let cs: Vec<(usize, char)> = s.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let (pos, c) = cs[i];
        if matches!(c, '.' | '!' | '?' | '\u{2026}') {
            let mut j = i + 1;
            while j < cs.len() && matches!(cs[j].1, '.' | '!' | '?' | '"' | '\'' | '\u{201d}' | '\u{2019}' | ')') {
                j += 1;
            }
            let end = cs.get(j).map(|x| x.0).unwrap_or(s.len());
            let followed_by_space = cs.get(j).map(|x| x.1.is_whitespace()).unwrap_or(true);
            if followed_by_space && !(c == '.' && is_abbreviation(&s[..pos])) {
                out.push(end);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Where to end the next part in `rest` (text not yet in a part); `after_known` is what was
/// said after `rest` so far (the held-back piece). Picks the latest sentence end that leaves at
/// least `MIN_PART_WORDS` words, with no correction cue in its last words and none anywhere
/// after it (a correction after a tidied part makes the whole text be cleaned at stop).
pub fn choose_cut(rest: &str, after_known: &str) -> Option<usize> {
    for cut in sentence_ends(rest).into_iter().rev() {
        let part = &rest[..cut];
        if text::word_count(part) < MIN_PART_WORDS {
            break;
        }
        let after = format!("{} {}", &rest[cut..], after_known);
        if cue_in_last_words(part, CUE_WINDOW_WORDS).is_some() || correction_cue(&after).is_some() {
            continue;
        }
        return Some(cut);
    }
    None
}

fn prose_instructions(instructions: &str) -> bool {
    let l = instructions.to_lowercase();
    !LAYOUT_WORDS.iter().any(|w| l.contains(w))
}

/// Whether a mode can be cleaned in parts: its layout must not depend on the whole text.
pub fn mode_allows_parts(mode: &Mode, category: Option<AppCategory>) -> Result<(), String> {
    let edited = mode.builtin && modes::default_instructions(&mode.id).is_some_and(|d| d != mode.instructions);
    let whole = || Err(format!("the {} mode lays out the whole text", mode.name));
    match mode.id.as_str() {
        "email" | "list" | "ai_prompt" => whole(),
        "auto" if matches!(category, Some(AppCategory::Email) | Some(AppCategory::AiAssistant)) => whole(),
        "auto" | "professional" | "casual" if !edited => Ok(()),
        _ if prose_instructions(&mode.instructions) => Ok(()),
        _ => Err(format!("the {} mode may change the layout", mode.name)),
    }
}

// ---------------------------------------------------------------------------- recipe

/// Everything that decides how a dictation is cleaned. Parts are only used when the recipe
/// at stop is the same as when they were cleaned. The text around the cursor is not compared:
/// it may arrive after parts were tidied (or only at stop); the first part gets it when it is
/// known by then, and the text at stop always uses the stop-time context.
#[derive(Clone)]
pub(crate) struct Recipe {
    mode: Mode,
    level: CleanupLevel,
    provider: CleanupProvider,
    model: String,
    system: Vec<SystemBlock>,
    entries: Vec<DictEntry>,
    snips: Vec<Snippet>,
    use_ctx: bool,
    before: Option<String>,
    after: Option<String>,
    /// How parts are joined: a blank line (documents, notes, email apps), or a space (chat,
    /// code, other apps).
    sep: &'static str,
}

impl Recipe {
    fn same_as(&self, o: &Recipe) -> bool {
        let rules = |e: &[DictEntry]| e.iter().map(|d| (d.word.clone(), d.replace.clone())).collect::<Vec<_>>();
        let snips = |s: &[Snippet]| s.iter().map(|x| (x.id, x.trigger.clone(), x.expansion.clone())).collect::<Vec<_>>();
        let sys = |s: &[SystemBlock]| s.iter().map(|b| (b.text.clone(), b.cache)).collect::<Vec<_>>();
        self.mode == o.mode
            && self.level == o.level
            && self.provider == o.provider
            && self.model == o.model
            && sys(&self.system) == sys(&o.system)
            && rules(&self.entries) == rules(&o.entries)
            && snips(&self.snips) == snips(&o.snips)
            && self.use_ctx == o.use_ctx
            && self.sep == o.sep
    }
}

/// A text after snippets and dictionary fixes, as words, with each snippet shown by its id (so
/// parts can be compared with the whole whatever the placeholder numbers), and the number of
/// snippets.
type Canonical = (Vec<String>, usize);

fn canonical(text: &str, r: &Recipe) -> Canonical {
    let (with_ph, map) = snippets::insert_placeholders(text, &r.snips);
    let (mut s, _) = dictionary::apply_replacements(&with_ph, &r.entries);
    for (ph, _, id) in &map {
        s = s.replace(ph.as_str(), &format!("\u{27E6}#{id}\u{27E7}"));
    }
    (s.split_whitespace().map(String::from).collect(), map.len())
}

/// True when snippets and dictionary fixes come out the same for the parts as for the whole
/// (nothing spans a boundary).
fn parts_match_whole(whole: &str, parts: &[&Canonical], r: &Recipe) -> bool {
    let (w, n) = canonical(whole, r);
    let mut p = Vec::with_capacity(w.len());
    let mut m = 0;
    for (c, k) in parts {
        p.extend(c.iter().cloned());
        m += k;
    }
    w == p && n == m
}

/// One part, cleaned.
#[derive(Clone, Debug)]
pub(crate) struct Part {
    /// `canonical(raw)`, worked out in the background so stopping has less to do.
    canon: Canonical,
    /// The model's answer, snippet placeholders still in (numbered for the whole dictation).
    out: String,
    /// The finished text of this part, for the next part's context.
    view: String,
    map: Vec<(String, String, i64)>,
    fixes: usize,
}

impl Engine {
    /// The cleanup recipe for a dictation that starts with `raw_start`, or why it can't be
    /// cleaned in parts. Has no side effects (a spoken mode switch is declined before
    /// `choose_mode`, which would record it).
    pub(crate) fn tidy_recipe(&self, raw_start: &str, ctx: &CaptureContext) -> Result<Recipe, String> {
        let settings = self.settings();
        if !settings.providers.clean_while_talking {
            return Err("tidy while talking is off".into());
        }
        if settings.cleanup_level == CleanupLevel::None {
            return Err("AI cleanup is off".into());
        }
        let all_modes = self.modes();
        if modes::detect_spoken_switch(raw_start, &all_modes).is_some() {
            return Err("a mode was chosen by voice".into());
        }
        let (mode, _) = self.choose_mode(raw_start, &ctx.app, None);
        mode_allows_parts(&mode, ctx.app.category)?;
        let entries = self.db.dict_all().unwrap_or_default();
        let snips = self.db.snippets_all().unwrap_or_default();
        let excluded = settings.context_excluded_apps.iter().any(|a| a.eq_ignore_ascii_case(&ctx.app.exe));
        let use_ctx = settings.context_awareness && !excluded;
        // Paragraphs are expected in documents, notes and email apps; elsewhere (chat, code,
        // unknown apps) a part boundary must not add a blank line.
        let paragraphs = mode.id != "casual" && matches!(ctx.app.category, Some(AppCategory::Document) | Some(AppCategory::Email));
        Ok(Recipe {
            system: cleanup_system_blocks(&entries, &mode, settings.cleanup_level),
            mode,
            level: settings.cleanup_level,
            provider: settings.providers.cleanup_provider,
            model: cleanup_model(&settings),
            entries,
            snips,
            use_ctx,
            before: if use_ctx { ctx.text_before.clone() } else { None },
            after: if use_ctx { ctx.text_after.clone() } else { None },
            sep: if paragraphs { "\n\n" } else { " " },
        })
    }

    /// Cleans one part: the same steps as the whole-text path, with placeholders numbered
    /// from `ph_offset` and the tidied text before it as context.
    #[allow(clippy::too_many_arguments)]
    async fn clean_part(
        &self,
        r: &Recipe,
        ctx: &CaptureContext,
        raw: &str,
        ph_offset: usize,
        before_part: Option<String>,
        index: usize,
        last: bool,
    ) -> Result<Part, ProviderError> {
        let (with_ph, map) = snippets::insert_placeholders(raw, &r.snips);
        let (with_ph, map) = snippets::renumber_placeholders(&with_ph, map, ph_offset);
        let (fixed, fixes) = dictionary::apply_replacements(&with_ph, &r.entries);
        // The first part gets the text before the cursor as known now (it can arrive after the
        // recording started); later parts get the tidied text before them instead.
        let before = if index == 1 && r.use_ctx { ctx.text_before.clone() } else { None };
        let mut c = cleanup_context_for(ctx, before, r.use_ctx);
        c.talking_part = Some(TalkingPart { index, last });
        c.before_part = before_part;
        let req = TextRequest {
            model: r.model.clone(),
            system: r.system.clone(),
            user: prompts::cleanup_user_message(&fixed, &c),
            max_tokens: max_tokens_for(&fixed),
        };
        let model = self.factory.text_model(&self.keys(), r.provider);
        let resp = self.retry.run(|| model.complete(req.clone())).await?;
        let out = text::sanitize_model_output(&resp.text, &fixed);
        if !looks_like_cleanup(&fixed, &out) {
            return Err(ProviderError::new(
                ErrorKind::BadResponse,
                model.provider_name(),
                "the AI's reply didn't look like a tidied version of this part",
            ));
        }
        let (fixed_out, _) = dictionary::apply_replacements(&out, &r.entries);
        let (view, _) = snippets::restore_placeholders(&fixed_out, &map);
        // The tail's is worked out before its call, at stop.
        let canon = if last { (Vec::new(), 0) } else { canonical(raw, r) };
        Ok(Part { canon, out, view, map, fixes })
    }

    /// Starts tidying a dictation in parts while the user talks, when it is switched on.
    pub fn start_tidy(self: &Arc<Self>, ctx: &CaptureContext, rt: tokio::runtime::Handle) -> Option<Arc<Tidy>> {
        let s = self.settings();
        if !s.providers.clean_while_talking || s.cleanup_level == CleanupLevel::None {
            return None;
        }
        // No key for the cleanup provider: every call would fail; the whole-text path explains.
        let keys = self.keys();
        let key = match s.providers.cleanup_provider {
            CleanupProvider::Anthropic => keys.anthropic,
            CleanupProvider::OpenAi => keys.openai,
        };
        if key.as_deref().map(str::trim).unwrap_or("").is_empty() {
            return None;
        }
        Some(Arc::new(Tidy {
            engine: self.clone(),
            ctx: Mutex::new(ctx.clone()),
            rt,
            st: Mutex::new(TidyState::default()),
            changed: watch::channel(0).0,
        }))
    }

    /// Cleans a finished dictation: from the parts tidied while the user talked when they can
    /// be used, otherwise exactly as `clean_text_for` (the whole text at once).
    pub async fn clean_dictation(&self, raw: &str, ctx: &CaptureContext, duration_ms: Option<u64>, tidy: Option<&Arc<Tidy>>) -> CleanResult {
        let Some(t) = tidy else { return self.clean_text_for(raw, ctx, None, duration_ms).await };
        let (why, ai_failed) = match self.finish_in_parts(raw, ctx, duration_ms, t).await {
            Ok(r) => return r,
            Err(NotUsed::Nothing) => (None, None),
            Err(NotUsed::Whole(reason)) => (Some(reason), None),
            Err(NotUsed::AiDown(reason, e)) => (Some(reason), Some(e)),
        };
        if let Some(reason) = &why {
            tracing::info!("tidy while talking not used ({reason}); cleaning the whole text");
        }
        // The AI just failed in a way a second try won't fix right now (offline, key, credit):
        // finish as the whole-text path does after its own failure, without waiting again.
        let mut r = self.clean_text_with(raw, ctx, None, duration_ms, ai_failed).await;
        r.tidy_calls = t.stats().calls;
        r.tidy_fallback = why;
        r
    }

    async fn finish_in_parts(&self, raw: &str, ctx: &CaptureContext, duration_ms: Option<u64>, tidy: &Arc<Tidy>) -> Result<CleanResult, NotUsed> {
        let snap = tidy.stop();
        if snap.calls == 0 {
            return Err(NotUsed::Nothing);
        }
        let why = |s: &str| Err(NotUsed::Whole(s.to_string()));
        if let Some(off) = snap.off {
            // A part that failed earlier while talking: only a bad key or no credit is still true
            // at stop. A network blip or timeout may be over, so clean the whole text normally.
            return Err(snap.failure.filter(key_or_credit).map(|e| NotUsed::AiDown(off.clone(), e)).unwrap_or(NotUsed::Whole(off)));
        }
        let Some(recipe) = snap.recipe else { return why("no recipe") };
        let settings = self.settings();
        if dictation_model(&settings, duration_ms).1 {
            return why("short dictation (the faster model cleans it whole)");
        }
        if settings.providers.fast_cleanup.skip_ai_for_tiny && text::word_count(raw) <= crate::quick::DEFAULT_MAX_WORDS {
            return why("tiny dictation");
        }
        let now = self.tidy_recipe(raw, ctx).map_err(NotUsed::Whole)?;
        if !now.same_as(&recipe) {
            return why("the mode, settings or dictionary changed while talking");
        }
        if looks_enumerated(raw) {
            return why("it sounds like a list");
        }
        let covered = snap.covered.as_str();
        let continues = raw.get(covered.len()..).map(|r| r.chars().next().is_none_or(char::is_whitespace)).unwrap_or(false);
        if !raw.starts_with(covered) || !continues {
            return why("the transcript changed");
        }
        let tail = raw[covered.len()..].trim();
        if let Some(c) = correction_cue(tail) {
            return Err(NotUsed::Whole(format!("a possible correction near the end (\"{c}\")")));
        }
        let flight_canon = snap.in_flight_raw.as_deref().map(|f| canonical(f, &recipe));
        let tail_canon = canonical(tail, &recipe);
        let mut canons: Vec<&Canonical> = snap.parts.iter().map(|p| &p.canon).collect();
        canons.extend(flight_canon.as_ref());
        canons.push(&tail_canon);
        if !parts_match_whole(raw, &canons, &recipe) {
            return why("a snippet or dictionary phrase spans two parts");
        }
        let n_parts = snap.parts.len() + usize::from(snap.in_flight_raw.is_some()) + usize::from(!tail.is_empty());

        // The tail runs now, alongside a part still being tidied (if any); its context is the
        // tidied text before it, plus the still-raw text of that part.
        let mut before: Vec<&str> = snap.parts.iter().map(|p| p.view.as_str()).collect();
        if let Some(f) = &snap.in_flight_raw {
            before.push(f);
        }
        let before = before.join(recipe.sep);
        let tail_index = n_parts;
        let tail_job = async {
            if tail.is_empty() {
                Ok(None)
            } else {
                self.clean_part(&recipe, ctx, tail, snap.ph_used, Some(before), tail_index, true).await.map(Some)
            }
        };
        let (flight, tail_result) = tokio::join!(tidy.wait_in_flight(snap.parts.len()), tail_job);
        let mut parts = snap.parts;
        match flight {
            Ok(Some(p)) => parts.push(p),
            // A part was being tidied at stop but nothing came back: don't guess.
            Ok(None) if snap.in_flight_raw.is_some() => return why("a part being tidied at stop went missing"),
            Ok(None) => {}
            Err((reason, Some(e))) if ai_unreachable(&e) => return Err(NotUsed::AiDown(reason, e)),
            Err((reason, _)) => return Err(NotUsed::Whole(reason)),
        }
        let tidied_while_talking = parts.len();
        match tail_result {
            Ok(Some(p)) => parts.push(p),
            Ok(None) => {}
            Err(e) => {
                let reason = format!("the last part couldn't be tidied ({})", e.message);
                return Err(if ai_unreachable(&e) { NotUsed::AiDown(reason, e) } else { NotUsed::Whole(reason) });
            }
        }

        // Finish exactly like the whole-text path: dictionary pass on the model's answer,
        // snippets back in, fitted to the text around the cursor as known at stop.
        let joined = parts.iter().map(|p| p.out.as_str()).collect::<Vec<_>>().join(recipe.sep);
        let (t2, more) = dictionary::apply_replacements(&joined, &recipe.entries);
        let map: Vec<(String, String, i64)> = parts.iter().flat_map(|p| p.map.clone()).collect();
        for (_, _, sid) in &map {
            let _ = self.db.snippet_bump(*sid);
        }
        let (restored, _) = snippets::restore_placeholders(&t2, &map);
        let final_text = text::fit_to_context(&restored, now.before.as_deref(), now.after.as_deref());
        let fixes = parts.iter().map(|p| p.fixes).sum::<usize>() + more;
        Ok(CleanResult {
            text: final_text,
            clean: Some(t2),
            mode_id: recipe.mode.id.clone(),
            mode_name: recipe.mode.name.clone(),
            fixes: fixes as i64,
            error: None,
            model: Some(recipe.model.clone()),
            fast: false,
            chunked: tidied_while_talking,
            tidy_calls: snap.calls,
            tidy_fallback: None,
        })
    }
}

/// Why parts tidied while talking weren't used.
enum NotUsed {
    /// Nothing was tidied while talking.
    Nothing,
    /// Clean the whole text instead (the reason is recorded in History).
    Whole(String),
    /// The AI can't be reached right now (offline, key, credit): no point asking again.
    AiDown(String, ProviderError),
}

fn key_or_credit(e: &ProviderError) -> bool {
    matches!(e.kind, ErrorKind::Auth | ErrorKind::Quota)
}

fn ai_unreachable(e: &ProviderError) -> bool {
    matches!(e.kind, ErrorKind::Network | ErrorKind::Timeout | ErrorKind::Auth | ErrorKind::Quota)
}

// ---------------------------------------------------------------------------- the background tidier

/// What happened in the background, for History and tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TidyStats {
    /// Background cleanup calls started while talking.
    pub calls: usize,
    /// Parts tidied so far.
    pub parts_done: usize,
    pub in_flight: bool,
    /// Why no (more) parts are tidied, once that is decided.
    pub off: Option<String>,
}

struct InFlight {
    raw: String,
    handle: JoinHandle<()>,
}

#[derive(Default)]
struct TidyState {
    /// Finished pieces waiting for an earlier one: index -> (end sample, text).
    waiting: BTreeMap<usize, (usize, String)>,
    /// Pieces in order from the first: (end sample, text).
    ordered: Vec<(usize, String)>,
    recipe: Option<Recipe>,
    parts: Vec<Part>,
    /// Bytes of the transcript (from the start) taken by finished and in-flight parts.
    claimed: usize,
    in_flight: Option<InFlight>,
    /// Snippet placeholders used by parts so far.
    ph_used: usize,
    off: Option<String>,
    /// The error of a part that failed, when one did.
    failure: Option<ProviderError>,
    stopped: bool,
    cancelled: bool,
    calls: usize,
}

struct Snapshot {
    calls: usize,
    off: Option<String>,
    failure: Option<ProviderError>,
    recipe: Option<Recipe>,
    parts: Vec<Part>,
    covered: String,
    in_flight_raw: Option<String>,
    ph_used: usize,
}

/// Tidies a dictation in parts while the user talks. Fed by the piece jobs; read at stop.
pub struct Tidy {
    engine: Arc<Engine>,
    /// Where the text is going. The text around the cursor may arrive after the start.
    ctx: Mutex<CaptureContext>,
    rt: tokio::runtime::Handle,
    st: Mutex<TidyState>,
    changed: watch::Sender<u64>,
}

/// Usable text: every piece before the most recent one with words (held back), and that one.
/// Returns (usable text, held-back text, end sample of the usable text).
fn usable(ordered: &[(usize, String)]) -> (String, String, usize) {
    match ordered.iter().rposition(|(_, t)| !t.trim().is_empty()) {
        None => (String::new(), String::new(), 0),
        Some(k) => {
            let texts: Vec<String> = ordered[..k].iter().map(|p| p.1.clone()).collect();
            let end = if k > 0 { ordered[k - 1].0 } else { 0 };
            (join_segments(&texts), ordered[k].1.trim().to_string(), end)
        }
    }
}

impl Tidy {
    fn lock(&self) -> std::sync::MutexGuard<'_, TidyState> {
        self.st.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn bump(&self) {
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
    }

    /// A piece was transcribed (`text` is empty for a piece without speech). Pieces may
    /// finish in any order.
    pub fn piece_done(self: &Arc<Self>, index: usize, end_sample: usize, text: &str) {
        {
            let mut st = self.lock();
            if st.cancelled {
                return;
            }
            st.waiting.insert(index, (end_sample, text.to_string()));
            loop {
                let next = st.ordered.len();
                let Some(p) = st.waiting.remove(&next) else { break };
                st.ordered.push(p);
            }
            self.pump(&mut st);
        }
        self.bump();
    }

    /// A piece couldn't be transcribed: the dictation will be retried later as a whole.
    pub fn piece_failed(&self, index: usize) {
        self.lock().off.get_or_insert_with(|| format!("piece {index} couldn't be transcribed"));
        self.bump();
    }

    /// The text around the cursor, read once recording has started (UI Automation takes a
    /// moment). Used for the first part's context; the finished text is fitted to the context
    /// known at stop.
    pub fn set_field_context(&self, before: Option<String>, after: Option<String>) {
        let mut c = self.ctx.lock().unwrap_or_else(|p| p.into_inner());
        c.text_before = before;
        c.text_after = after;
    }

    /// No more parts from now on (the recording became a voice command, …).
    pub fn disable(&self, why: &str) {
        self.lock().off.get_or_insert_with(|| why.to_string());
        self.bump();
    }

    /// The user stopped: no new parts are started; the last part is cleaned by the pipeline.
    pub fn user_stopped(&self) {
        self.lock().stopped = true;
        self.bump();
    }

    /// Cancelled (or the dictation went the normal way): background work stops and its
    /// results are discarded. Nothing here is ever the only copy of anything.
    pub fn cancel(&self) {
        {
            let mut st = self.lock();
            st.cancelled = true;
            st.stopped = true;
            if let Some(f) = st.in_flight.take() {
                f.handle.abort();
            }
            st.parts.clear();
        }
        self.bump();
    }

    pub fn stats(&self) -> TidyStats {
        let st = self.lock();
        TidyStats { calls: st.calls, parts_done: st.parts.len(), in_flight: st.in_flight.is_some(), off: st.off.clone() }
    }

    /// Waits until no part is being tidied.
    pub async fn idle(&self) {
        loop {
            let mut rx = self.changed.subscribe();
            if self.lock().in_flight.as_ref().is_none_or(|f| f.handle.is_finished()) {
                return;
            }
            let _ = tokio::time::timeout(Duration::from_millis(50), rx.changed()).await;
        }
    }

    /// Starts the next part if one is ready. Called with the state locked.
    fn pump(self: &Arc<Self>, st: &mut TidyState) {
        if st.cancelled || st.stopped || st.off.is_some() || st.in_flight.is_some() {
            return;
        }
        let (text, held, end_sample) = usable(&st.ordered);
        if text.len() <= st.claimed {
            return;
        }
        if st.parts.is_empty() {
            // Only for dictations that are getting long, and never for ones short enough for
            // the fast model (which cleans the whole text quickly at stop).
            let ms = audio::samples_to_ms(end_sample);
            let long = ms >= START_AFTER_MS || text::word_count(&text) >= START_AFTER_WORDS;
            if !long || dictation_model(&self.engine.settings(), Some(ms)).1 {
                return;
            }
        }
        if looks_enumerated(&text) {
            st.off = Some("it sounds like a list".into());
            return;
        }
        let ctx = self.ctx.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if st.recipe.is_none() {
            match self.engine.tidy_recipe(&text, &ctx) {
                Ok(r) => st.recipe = Some(r),
                Err(e) => {
                    st.off = Some(e);
                    return;
                }
            }
        }
        let rest = &text[st.claimed..];
        if !st.parts.is_empty() {
            // A correction anywhere after a tidied part ("actually…", "I mean…", "scratch
            // that") may refer back into it: the whole text is cleaned at stop instead.
            if let Some(c) = correction_cue(rest) {
                st.off = Some(format!("a possible correction after a tidied part (\"{c}\")"));
                return;
            }
        }
        let Some(cut) = choose_cut(rest, &held) else { return };
        let raw = rest[..cut].trim().to_string();
        let Some(recipe) = st.recipe.clone() else { return };
        let index = st.parts.len() + 1;
        let before_part = (index > 1).then(|| st.parts.iter().map(|p| p.view.as_str()).collect::<Vec<_>>().join(recipe.sep));
        let ph_offset = st.ph_used;
        st.ph_used += snippets::insert_placeholders(&raw, &recipe.snips).1.len();
        st.claimed += cut;
        st.calls += 1;
        let me = self.clone();
        let part_raw = raw.clone();
        let handle = self.rt.spawn(async move {
            let r = me.engine.clean_part(&recipe, &ctx, &part_raw, ph_offset, before_part, index, false).await;
            me.part_finished(r);
        });
        st.in_flight = Some(InFlight { raw, handle });
    }

    fn part_finished(self: &Arc<Self>, r: Result<Part, ProviderError>) {
        {
            let mut st = self.lock();
            st.in_flight = None;
            if st.cancelled {
                return;
            }
            match r {
                Ok(p) => st.parts.push(p),
                Err(e) => {
                    tracing::info!("tidying a part while talking failed: {e}");
                    st.off.get_or_insert_with(|| format!("a part couldn't be tidied ({})", e.message));
                    st.failure.get_or_insert(e);
                }
            }
            self.pump(&mut st);
        }
        self.bump();
    }

    /// At stop: no new parts; returns what was done.
    fn stop(&self) -> Snapshot {
        let mut st = self.lock();
        st.stopped = true;
        let all: Vec<String> = st.ordered.iter().map(|p| p.1.clone()).collect();
        let full = join_segments(&all);
        Snapshot {
            calls: st.calls,
            off: if st.cancelled { Some("cancelled".into()) } else { st.off.clone() },
            failure: st.failure.clone(),
            recipe: st.recipe.clone(),
            parts: st.parts.clone(),
            covered: full.get(..st.claimed).unwrap_or("").to_string(),
            in_flight_raw: st.in_flight.as_ref().map(|f| f.raw.clone()),
            ph_used: st.ph_used,
        }
    }

    /// Waits for the part being tidied at stop (if any), which becomes part `done` (from 0).
    async fn wait_in_flight(&self, done: usize) -> Result<Option<Part>, (String, Option<ProviderError>)> {
        loop {
            let mut rx = self.changed.subscribe();
            {
                let st = self.lock();
                match &st.in_flight {
                    None => {
                        if let Some(off) = &st.off {
                            return Err((off.clone(), st.failure.clone()));
                        }
                        if st.cancelled {
                            return Err(("cancelled".into(), None));
                        }
                        return Ok(st.parts.get(done).cloned());
                    }
                    Some(f) if f.handle.is_finished() => return Err(("tidying a part stopped unexpectedly".into(), None)),
                    Some(_) => {}
                }
            }
            let _ = tokio::time::timeout(Duration::from_millis(50), rx.changed()).await;
        }
    }
}

#[cfg(test)]
#[path = "tidy_tests.rs"]
mod tests;

//! Evidence for the quick path (`wisper_core::quick`): tiny dictations tidied locally instead
//! of by the AI.
//!
//! Two tests share one corpus of realistic short dictations:
//!
//! - `corpus_decisions_match_labels` (runs everywhere, no API): every dictation labelled "needs
//!   the AI" must be declined by the quick path (a safety check), every one labelled "quick"
//!   must be accepted (keeps the corpus honest), and it prints how many the quick path covers.
//!
//! - `quick_path_matches_ai` (real API, ignored by default): for every dictation the quick path
//!   accepts, runs the real AI cleanup through the same code the app uses (`Engine::clean_text`,
//!   with the same dictionary, snippets, mode choice and app context) and compares.
//!   Run with:
//!   ANTHROPIC_API_KEY=... cargo test -p wisper-core --test quick_eval -- --ignored --nocapture
//!   CI runs it when the ANTHROPIC_API_KEY secret is set. The model is the app's default
//!   (claude-sonnet-5-5); set QUICK_EVAL_MODELS=model1,model2 (or WISPER_CLEANUP_MODEL) to
//!   check others, e.g. the fast model used for short recordings (claude-haiku-5-5).
//!
//! How results are compared (after both go through `text::fit_to_context`, as in the app):
//! - "same": identical text.
//! - "full stop": the only difference is one final full stop present in one and missing in
//!   the other. Allowed, and listed. Reason: the owner's rule lets the local tidy own
//!   capitalisation and end punctuation, the casual instructions ask for "light punctuation"
//!   (so the model itself is inconsistent about a closing full stop on a chat message), and
//!   the words, internal punctuation and capitals are identical.
//! - "DIFFERENT": anything else — a word added, removed or changed, a comma, a capital, "?" or
//!   "!" instead of ".". Any one of these fails the test.
//! - "ERROR": the AI call failed; also fails the test, since it proves nothing.

use std::sync::Arc;
use std::time::Instant;

use wisper_core::apps::{self, AppInfo};
use wisper_core::db::Db;
use wisper_core::pipeline::{ApiKeys, CaptureContext, Engine, RealProviders};
use wisper_core::quick::{quick_check, Declined, QuickContext};
use wisper_core::settings::Settings;
use wisper_core::snippets::Snippet;
use wisper_core::text;

#[derive(Clone, Copy, Debug)]
enum Target {
    WhatsApp,
    Slack,
    Teams,
    Notepad,
    Word,
    Gmail,
    Claude,
    Unknown,
}

impl Target {
    fn info(self) -> AppInfo {
        let (exe, title) = match self {
            Target::WhatsApp => ("whatsapp.exe", "WhatsApp"),
            Target::Slack => ("slack.exe", "general - Slack"),
            Target::Teams => ("ms-teams.exe", "Chat | Microsoft Teams"),
            Target::Notepad => ("notepad.exe", "notes.txt - Notepad"),
            Target::Word => ("winword.exe", "Report - Word"),
            Target::Gmail => ("chrome.exe", "Inbox - Gmail - Google Chrome"),
            Target::Claude => ("claude.exe", "Claude"),
            Target::Unknown => ("someapp.exe", "Some App"),
        };
        apps::identify(exe, title)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Expect {
    /// The quick path should handle it.
    Quick,
    /// It needs the AI (filler, correction, Hebrew/Yiddish, number, …).
    Ai,
}

struct Case {
    raw: &'static str,
    app: Target,
    before: Option<&'static str>,
    expect: Expect,
}

const fn q(raw: &'static str, app: Target) -> Case {
    Case { raw, app, before: None, expect: Expect::Quick }
}
const fn a(raw: &'static str, app: Target) -> Case {
    Case { raw, app, before: None, expect: Expect::Ai }
}
const fn qb(raw: &'static str, app: Target, before: &'static str) -> Case {
    Case { raw, app, before: Some(before), expect: Expect::Quick }
}
const fn ab(raw: &'static str, app: Target, before: &'static str) -> Case {
    Case { raw, app, before: Some(before), expect: Expect::Ai }
}

use Target::*;

/// Short dictations as the speech recogniser returns them (it usually punctuates; a few are
/// left unpunctuated on purpose).
const CORPUS: &[Case] = &[
    // --- Everyday chat (WhatsApp → Casual message) ---
    q("Sounds good, see you tomorrow.", WhatsApp),
    q("On my way.", WhatsApp),
    q("I'm running late.", WhatsApp),
    q("Can you pick up some milk on the way home?", WhatsApp),
    q("Love you too.", WhatsApp),
    q("Thanks so much!", WhatsApp),
    q("No problem.", WhatsApp),
    q("I'll call you later.", WhatsApp),
    q("Where are you?", WhatsApp),
    q("Are you home yet?", WhatsApp),
    q("That's great news.", WhatsApp),
    q("Let me know when you get there.", WhatsApp),
    q("Dinner is ready.", WhatsApp),
    q("Can't wait to see you!", WhatsApp),
    q("Have a safe flight.", WhatsApp),
    q("Good night.", WhatsApp),
    q("I'm almost done.", WhatsApp),
    q("Did you see my message?", WhatsApp),
    q("Just landed.", WhatsApp),
    q("Happy birthday!", WhatsApp),
    q("Congratulations on the new job!", WhatsApp),
    q("I'll be there soon.", WhatsApp),
    q("We're leaving now.", WhatsApp),
    q("Do you want to grab lunch?", WhatsApp),
    q("I forgot my keys.", WhatsApp),
    q("Call me when you're free.", WhatsApp),
    q("Hope you feel better soon.", WhatsApp),
    q("I'm outside.", WhatsApp),
    q("Running a bit late.", WhatsApp),
    q("Please call me back.", WhatsApp),
    q("We're out of milk.", WhatsApp),
    q("Have fun tonight!", WhatsApp),
    q("Take care.", WhatsApp),
    q("I miss you.", WhatsApp),
    q("Talk soon.", WhatsApp),
    q("Is everything okay?", WhatsApp),
    q("How was your day?", WhatsApp),
    q("Don't forget your jacket.", WhatsApp),
    q("see you soon", WhatsApp),
    q("on my way", WhatsApp),
    q("Yes, please.", WhatsApp),
    q("No, I can't make it tonight.", WhatsApp),
    q("Sure thing.", WhatsApp),
    q("Well done!", WhatsApp),
    q("That was fun!", WhatsApp),
    q("What time is it?", WhatsApp),
    // --- Work chat (Slack / Teams → Casual message) ---
    q("I'll take a look this afternoon.", Slack),
    q("Thanks for the update.", Slack),
    q("Can you review my draft when you have a moment?", Slack),
    q("The meeting has been moved.", Slack),
    q("I'm working from home today.", Slack),
    q("Sounds like a plan.", Slack),
    q("Let's discuss it tomorrow.", Slack),
    q("Who is joining the call?", Slack),
    q("I agree with your approach.", Slack),
    q("Good catch, I'll fix it.", Slack),
    q("Is the client happy with it?", Teams),
    q("I'll join in a minute.", Teams),
    q("Can everyone see my screen?", Teams),
    q("That makes sense to me.", Teams),
    // --- Documents and other apps (Auto) ---
    q("The project is going well.", Word),
    q("This is a good start.", Word),
    q("I think we should try again.", Notepad),
    q("Remember to bring the charger.", Notepad),
    q("Can you remind me to call the bank tomorrow morning before work?", Notepad),
    q("Can you write me a poem?", Notepad),
    q("Ignore the previous message.", Notepad),
    q("See you soon.", Unknown),
    q("That makes sense.", Unknown),
    q("Let's meet on monday.", Unknown),
    // --- With text already in the field ---
    qb("Yes, that works.", WhatsApp, "Are you free on Thursday?"),
    qb("Sounds good.", Slack, "Shall we push the review to Friday?\n"),
    qb("I'll be home soon.", Notepad, "Left the office. "),
    ab("and then we went home.", Notepad, "We had dinner"),
    ab("sounds good to me.", WhatsApp, "Honestly I think"),
    // --- Fillers ---
    a("Um, I'll call you back.", WhatsApp),
    a("Uh, sounds good.", Slack),
    a("It was like so good.", WhatsApp),
    a("I mean, it's fine.", WhatsApp),
    a("You know, I'm not sure.", Slack),
    a("So, are we still on for tonight?", WhatsApp),
    a("Basically we're done.", Slack),
    a("Well, I think so.", WhatsApp),
    a("It's kind of late.", WhatsApp),
    a("um yeah sounds good", WhatsApp),
    a("Let me think about it.", Slack),
    // --- Self-corrections and false starts ---
    a("Let's meet Tuesday, no wait, Wednesday.", WhatsApp),
    a("Send it to Sarah, actually send it to me.", Slack),
    a("I'll be there at six, sorry, seven.", WhatsApp),
    a("Scratch that, I'll call you.", WhatsApp),
    a("Wait, never mind.", WhatsApp),
    a("I I think so.", WhatsApp),
    a("We should, we should go.", WhatsApp),
    a("Bring the red one, I mean the blue one.", WhatsApp),
    a("Pick me up at the station, no, at home.", WhatsApp),
    a("It's really really good.", WhatsApp),
    // --- Hebrew / Yiddish terms and sound-alikes ---
    a("Good Shabbos.", WhatsApp),
    a("Gut Shabbos.", WhatsApp),
    a("Mazel tov!", WhatsApp),
    a("Baruch Hashem, all good.", WhatsApp),
    a("B'ezras Hashem we'll finish tomorrow.", Slack),
    a("It's a big zeus.", WhatsApp),
    a("May we have the Zeus.", WhatsApp),
    a("May we have the zechus.", WhatsApp),
    a("See you at shul.", WhatsApp),
    a("Have a good Yom Tov.", WhatsApp),
    a("What a simcha!", WhatsApp),
    a("We'll see, be'H.", WhatsApp),
    a("Such nachas.", WhatsApp),
    a("I'm going to daven.", WhatsApp),
    a("Shabbat shalom.", WhatsApp),
    a("Refuah shleimah.", WhatsApp),
    a("Chag sameach!", WhatsApp),
    a("Kein ayin hara.", WhatsApp),
    a("Have a good walk.", WhatsApp),
    a("L'chaim!", WhatsApp),
    a("He's learning in kollel.", Notepad),
    a("Can you bring the chess said?", WhatsApp),
    // --- Snippet trigger (the eval adds "my address" as a snippet) ---
    a("Here's my address.", WhatsApp),
    // --- Numbers, dates, money, addresses ---
    a("See you at 3pm.", WhatsApp),
    a("I'll be there in ten minutes.", WhatsApp),
    a("It costs twenty pounds.", WhatsApp),
    a("The meeting is on June 4th.", Slack),
    a("Can you send me two copies?", Slack),
    a("We need a dozen eggs.", WhatsApp),
    a("My email is david at gmail dot com.", Notepad),
    a("It's half past five.", WhatsApp),
    a("That's 50% off.", WhatsApp),
    a("See you in march.", WhatsApp),
    // --- Lists, mode switches, spoken formatting ---
    a("As a list, eggs, milk and bread.", Notepad),
    a("Make it a list.", Notepad),
    a("First, call the bank, second, pay the bill.", Notepad),
    a("Bullet points please.", Notepad),
    a("Email mode. Hi David.", Notepad),
    a("In casual style, see you later.", Notepad),
    a("Hi new line how are you.", WhatsApp),
    a("Thanks full stop.", WhatsApp),
    a("Hello comma how are you question mark", WhatsApp),
    // --- Names and uncommon words ---
    a("Thanks David.", WhatsApp),
    a("Tell Sarah I said hi.", WhatsApp),
    a("I'll meet you at Starbucks.", WhatsApp),
    a("Menachem is coming.", WhatsApp),
    a("Did you call Zaidy?", WhatsApp),
    a("What is the capital of France?", Notepad),
    // --- Punctuation the AI would decide ---
    a("thanks see you soon", WhatsApp),
    a("yes please", WhatsApp),
    a("Okay I'll call you.", WhatsApp),
    a("Hey how are you?", WhatsApp),
    a("See you later everyone.", Slack),
    a("can you call me", WhatsApp),
    a("Great see you then.", WhatsApp),
    a("happy birthday", WhatsApp),
    a("thanks so much", WhatsApp),
    a("are you coming", WhatsApp),
    a("I'm at the store, do you need anything?", WhatsApp),
    a("Okay. Sounds good.", WhatsApp),
    // --- Grammar slips and slang ---
    a("I should of called.", WhatsApp),
    a("Your welcome.", WhatsApp),
    a("Its late.", WhatsApp),
    a("Me and him went home.", WhatsApp),
    a("I'm gonna be late.", WhatsApp),
    a("I'll check and let you know.", Slack),
    // --- Apps where the cleanup reshapes text ---
    a("Sounds good, see you tomorrow.", Gmail),
    a("Thanks for the update.", Gmail),
    a("Write a poem about cats.", Claude),
    a("Can you explain this code?", Claude),
    // --- Too long for the quick path ---
    a("I was thinking we could go to the park after lunch and then get ice cream.", WhatsApp),
];

struct Setup {
    engine: Arc<Engine>,
    db: Arc<Db>,
    settings: Settings,
    _dir: tempfile::TempDir,
}

fn setup(mut settings: Settings, keys: ApiKeys) -> Setup {
    // Learning per-app styles from spoken mode switches would make the result depend on the
    // order of the corpus; every case here should see the app's default choice.
    settings.learn_app_styles = false;
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    db.snippet_save(&Snippet { id: 0, trigger: "my address".into(), expansion: "12 High Street, London".into(), uses: 0 }).unwrap();
    let engine = Engine::new(db.clone(), dir.path().to_path_buf(), settings.clone(), keys, Arc::new(RealProviders));
    Setup { engine, db, settings, _dir: dir }
}

fn capture(c: &Case) -> CaptureContext {
    CaptureContext { app: c.app.info(), text_before: c.before.map(String::from), ..Default::default() }
}

/// What the app would pass to the quick path for this dictation: the same dictionary,
/// snippets, modes, chosen mode, app and (with context awareness on) text before the cursor.
fn quick_for(s: &Setup, c: &Case) -> Result<String, Declined> {
    let ctx = capture(c);
    let (mode, _) = s.engine.choose_mode(c.raw, &ctx.app, None);
    let entries = s.db.dict_all().unwrap();
    let snippets = s.db.snippets_all().unwrap();
    let before = if s.settings.context_awareness { ctx.text_before.as_deref() } else { None };
    let qc = QuickContext::new(&entries, &snippets, &s.engine.modes(), &mode, ctx.app.category, s.settings.cleanup_level)
        .with_text_before(before);
    quick_check(c.raw, &qc).map(|t| text::fit_to_context(&t, before, None))
}

fn one_line(s: &str) -> String {
    s.replace('\n', "\\n")
}

#[tokio::test]
async fn corpus_decisions_match_labels() {
    let s = setup(Settings::default(), ApiKeys::default());
    let mut unsafe_accepts = Vec::new();
    let mut missed = Vec::new();
    let mut accepted = 0;
    for c in CORPUS {
        match (quick_for(&s, c), c.expect) {
            (Ok(t), Expect::Ai) => unsafe_accepts.push(format!("{:?} → {:?} ({:?})", c.raw, t, c.app)),
            (Ok(_), Expect::Quick) => accepted += 1,
            (Err(d), Expect::Quick) => missed.push(format!("{:?} ({:?}): {d}", c.raw, c.app)),
            (Err(_), Expect::Ai) => {}
        }
    }
    let total = CORPUS.len();
    println!(
        "quick path corpus: {total} short dictations, {accepted} handled locally ({:.0}%), {} sent to the AI",
        accepted as f64 * 100.0 / total as f64,
        total - accepted
    );
    assert!(total >= 150, "corpus has {total} cases");
    assert!(unsafe_accepts.is_empty(), "accepted dictations that need the AI:\n{}", unsafe_accepts.join("\n"));
    assert!(missed.is_empty(), "declined dictations labelled quick (fix the label or the rule):\n{}", missed.join("\n"));
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum Verdict {
    Same,
    FullStop,
    Different,
    Error,
}

/// Compares the quick result with the AI's (see the module comment for the definitions).
fn compare(quick: &str, ai: &str) -> Verdict {
    if quick == ai {
        return Verdict::Same;
    }
    let strip = |s: &str| s.strip_suffix('.').unwrap_or(s).to_string();
    let (a, b) = (strip(quick), strip(ai));
    // Exactly one of them ended in a single full stop and the rest is identical.
    if a == b && (quick.ends_with('.') != ai.ends_with('.')) && !a.ends_with(['.', '?', '!']) {
        return Verdict::FullStop;
    }
    Verdict::Different
}

fn models_to_check(default: &str) -> Vec<String> {
    let from_env = std::env::var("QUICK_EVAL_MODELS").ok().or_else(|| std::env::var("WISPER_CLEANUP_MODEL").ok());
    let list: Vec<String> = from_env
        .unwrap_or_default()
        .split(',')
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect();
    if list.is_empty() {
        vec![default.to_string()]
    } else {
        list
    }
}

#[tokio::test]
#[ignore]
async fn quick_path_matches_ai() {
    let Some(key) = std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.is_empty()) else {
        eprintln!("ANTHROPIC_API_KEY not set; skipping");
        return;
    };
    let defaults = Settings::default();
    let mut failed = Vec::new();
    for model in models_to_check(&defaults.providers.anthropic_model) {
        let mut settings = defaults.clone();
        settings.providers.anthropic_model = model.clone();
        let keys = ApiKeys { openai: None, anthropic: Some(key.clone()) };
        let s = setup(settings, keys);

        println!("\n=== Quick path vs AI cleanup — model {model} ===\n");
        println!("{:<10} {:<9} {:<8} {:<52} {:<52} AI", "result", "app", "mode", "dictation", "quick");
        let mut counts = std::collections::BTreeMap::new();
        let mut differences = Vec::new();
        let mut accepted = 0;
        let mut ai_ms = Vec::new();
        for c in CORPUS {
            let Ok(quick) = quick_for(&s, c) else { continue };
            accepted += 1;
            let started = Instant::now();
            let r = s.engine.clean_text(c.raw, &capture(c), None).await;
            ai_ms.push(started.elapsed().as_millis() as u64);
            let verdict = if r.error.is_some() { Verdict::Error } else { compare(&quick, &r.text) };
            *counts.entry(format!("{verdict:?}")).or_insert(0usize) += 1;
            let label = match verdict {
                Verdict::Same => "same",
                Verdict::FullStop => "full stop",
                Verdict::Different => "DIFFERENT",
                Verdict::Error => "ERROR",
            };
            let (app, raw, quick_q, ai_q) =
                (format!("{:?}", c.app), format!("{:?}", c.raw), format!("{quick:?}"), format!("{:?}", one_line(&r.text)));
            println!("{label:<10} {app:<9} {:<8} {raw:<52} {quick_q:<52} {ai_q}", r.mode_id);
            if let Some(e) = &r.error {
                println!("           error: {e}");
            }
            if matches!(verdict, Verdict::Different | Verdict::Error) {
                differences.push(format!(
                    "  {:?} ({:?}{})\n    quick: {:?}\n    AI:    {:?}{}",
                    c.raw,
                    c.app,
                    c.before.map(|b| format!(", after {b:?}")).unwrap_or_default(),
                    quick,
                    one_line(&r.text),
                    r.error.as_ref().map(|e| format!("\n    error: {e}")).unwrap_or_default()
                ));
            }
        }
        let total = CORPUS.len();
        let same = counts.get("Same").copied().unwrap_or(0);
        let stop = counts.get("FullStop").copied().unwrap_or(0);
        let diff = counts.get("Different").copied().unwrap_or(0);
        let err = counts.get("Error").copied().unwrap_or(0);
        ai_ms.sort_unstable();
        let median = ai_ms.get(ai_ms.len() / 2).copied().unwrap_or(0);
        println!("\n--- Summary for {model} ---");
        println!("Short dictations in the corpus:         {total}");
        println!("Handled by the quick path (coverage):   {accepted} ({:.0}%)", accepted as f64 * 100.0 / total as f64);
        println!("  identical to the AI:                  {same}");
        println!("  only the final full stop differs:     {stop} (allowed)");
        println!("  materially different:                 {diff}");
        println!("  AI errors (inconclusive):             {err}");
        println!("Median AI cleanup time skipped:         {median} ms per dictation");
        if !differences.is_empty() {
            println!("\nDifferences:\n{}", differences.join("\n"));
            failed.push(format!("{model}: {diff} different, {err} errors"));
        }
    }
    assert!(failed.is_empty(), "the quick path is not yet safe: {failed:?}");
}

#[test]
fn comparison_rules() {
    assert_eq!(compare("Sounds good.", "Sounds good."), Verdict::Same);
    assert_eq!(compare("Sounds good.", "Sounds good"), Verdict::FullStop);
    assert_eq!(compare("Sounds good", "Sounds good."), Verdict::FullStop);
    assert_eq!(compare("Sounds good.", "Sounds good!"), Verdict::Different);
    assert_eq!(compare("Can you call me.", "Can you call me?"), Verdict::Different);
    assert_eq!(compare("Thanks see you soon.", "Thanks, see you soon."), Verdict::Different);
    assert_eq!(compare("Sounds good.", "sounds good."), Verdict::Different);
    assert_eq!(compare("I'm on my way.", "On my way."), Verdict::Different);
    assert_eq!(compare("Wait.", "Wait..."), Verdict::Different);
}

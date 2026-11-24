//! Real-API evaluation of "tidy up while you talk" (`pipeline/tidy.rs`): long dictations are
//! cleaned in parts while the user talks, and only the tail at stop. For realistic long
//! dictations this runs both the parts path and the normal whole-text path with the same
//! model, checks that the parts path loses or adds nothing and keeps Ashkenazi spellings, and
//! reports how long the user waits after stopping on each path.
//!
//! Needs a key, costs a few cents, so it's ignored by default. Run with:
//!   ANTHROPIC_API_KEY=... cargo test -p wisper-core --test chunked_eval -- --ignored --nocapture
//! CI runs it when the ANTHROPIC_API_KEY secret is set. The model is the app's default
//! cleanup model; set CHUNKED_EVAL_MODELS=model1,model2 (or WISPER_CLEANUP_MODEL) for others.
//!
//! How a dictation is simulated: the speech model's pieces (as it returns them at natural
//! pauses) are handed over one by one, and background tidying is allowed to finish between
//! pieces (a person takes several seconds to say each piece, longer than a cleanup call). Then
//! the user stops: the wait measured is the time to clean the tail and join everything.
//!
//! What fails the test (parts path only; the whole-text path is printed for comparison):
//! - a key fact, name, number or Hebrew/Yiddish term missing (`must`, `facts`);
//! - a wrong spelling or a corrected-away word present (`must_not`);
//! - the text being much longer or shorter than the whole-text cleanup (> 25 % apart in
//!   words: something was added or lost);
//! - a cleanup error.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use wisper_core::apps;
use wisper_core::db::Db;
use wisper_core::pipeline::{ApiKeys, CaptureContext, Engine, ProviderFactory, RealProviders};
use wisper_core::providers::{ProviderError, TextModel, TextRequest, TextResponse, TranscribeRequest, Transcriber, Transcript};
use wisper_core::settings::{CleanupProvider, Settings};
use wisper_core::text;

struct Case {
    name: &'static str,
    exe: &'static str,
    title: &'static str,
    /// What the speech model returned, piece by piece.
    pieces: &'static [&'static str],
    /// Must appear exactly (case-sensitive): names and Hebrew/Yiddish spellings.
    must: &'static [&'static str],
    /// Each group: at least one alternative must appear (case-insensitive): facts, numbers.
    facts: &'static [&'static [&'static str]],
    /// Must not appear (case-sensitive): wrong spellings, self-corrected words.
    must_not: &'static [&'static str],
    /// Expected to be tidied in parts (false: one of the whole-text rules applies).
    parts: bool,
}

const WRONG_SPELLINGS: &[&str] = &["Shabbat", "Shabbas", "zchus", "zehus", "parnassa", "Sukkot", "Shavuot", "b'ezrat", "nachat", "Baruch HaShem", "HaShem"];

const CASES: &[Case] = &[
    Case {
        name: "casual chat: weekend plans",
        exe: "whatsapp.exe",
        title: "WhatsApp",
        pieces: &[
            "Hey, so um, I wanted to tell you about this Shabbos. We're going to be at my parents for the Friday night meal,",
            "and then on Shabbos morning we'll go to the early minyan at the shul on Main Street, the one near the park.",
            "After that there's a kiddush for the Levines because their son had his bar mitzvah last week, so we'll stay for that.",
            "Uh, in the afternoon I think the kids want to go to the park with your kids if the weather is nice. It's supposed to be sunny,",
            "like around 70 degrees, so it should be perfect. Let me know if that works for you guys.",
            "Oh, and Rivky said she's making her famous potato kugel, so you know, save some room.",
            "Anyway, we'll head home around five because the little one needs a nap. Talk soon.",
        ],
        must: &["Shabbos", "kiddush", "Levine", "bar mitzvah", "Rivky"],
        facts: &[&["main street"], &["70"], &["kugel"], &["five", "5"], &["minyan"]],
        must_not: &[],
        parts: true,
    },
    Case {
        name: "work notes with numbers",
        exe: "notepad.exe",
        title: "notes.txt - Notepad",
        pieces: &[
            "Okay, notes from the meeting with the contractor this morning. So the main thing is the kitchen renovation is going to start on March 3rd,",
            "and they think it'll take about six weeks, maybe seven if the cabinets are late. The total quote came in at $18,500,",
            "which is about $2,000 more than we budgeted, mostly because of the countertops. Um, they want a deposit of 30% before they start.",
            "The plumber is coming separately on the 10th to move the sink line. We need to pick the tiles by the end of next week,",
            "otherwise it pushes everything back. David said he can do the electrical himself which saves us around $1,200.",
            "I also need to call the insurance company about the water damage from last year because that might cover part of the floor.",
            "And we should ask about a payment plan for the second half of the money.",
        ],
        must: &["David"],
        facts: &[&["march 3", "3 march", "3rd march"], &["six weeks", "6 weeks"], &["18,500", "18500"], &["2,000", "2000"], &["30%", "30 percent"], &["10th"], &["1,200", "1200"], &["insurance"], &["payment plan"]],
        must_not: &[],
        parts: true,
    },
    Case {
        name: "a story",
        exe: "winword.exe",
        title: "Memories - Word",
        pieces: &[
            "My Zeidy used to tell us a story every Pesach about how he came to America. He was only fourteen years old,",
            "and he traveled by himself on a ship from Poland with just one small suitcase and a pair of tefillin his father gave him.",
            "The trip took almost three weeks, and he said the food was so bad that he mostly ate bread and potatoes.",
            "When he arrived in New York, he didn't know anyone, but a man from the shul on the Lower East Side took him in",
            "and found him a job in a bakery. He worked there for six years and studied in a yeshiva at night.",
            "Eventually he saved enough money to open his own bakery in Brooklyn, which he ran for over forty years.",
            "Every year at the Seder he would hold up the matzah and say that freedom tastes better than any bread he ever baked.",
        ],
        must: &["Zeidy", "Pesach", "tefillin", "yeshiva", "Poland", "Brooklyn", "Seder"],
        facts: &[&["fourteen", "14"], &["three weeks", "3 weeks"], &["six years", "6 years"], &["forty years", "40 years"], &["lower east side"], &["bakery"]],
        must_not: &[],
        parts: true,
    },
    Case {
        name: "Hebrew and Yiddish mix",
        exe: "whatsapp.exe",
        title: "WhatsApp",
        pieces: &[
            "Hi Shmuli, I just wanted to say what a zechus it was to be at the chasunah last night. B'ezras Hashem,",
            "you should have so much nachas from the kallah and chosson. The music was mamesh geshmak, and the dancing went on until like one in the morning.",
            "Baruch Hashem, my mother was able to come too, which was a big deal for her after the surgery.",
            "I wanted to ask you, is there a list for the sheva brachos meals? We'd love to host one on Tuesday night if it's still open.",
            "Also, my brother-in-law is looking for parnasa, he's really good with computers, so if you hear of anything let me know.",
            "Mazel tov again to the whole family, and be'H we'll see you at the next simcha.",
        ],
        must: &["Shmuli", "zechus", "chasunah", "b'ezras Hashem", "nachas", "kallah", "chosson", "mamesh", "geshmak", "Baruch Hashem", "sheva brachos", "parnasa", "be'H", "simcha"],
        facts: &[&["mazel tov"], &["tuesday"], &["one in the morning", "1 in the morning", "1am", "1 am", "1 a.m."], &["computers"], &["surgery"]],
        must_not: &[],
        parts: true,
    },
    Case {
        name: "self-correction in the middle",
        exe: "notepad.exe",
        title: "plan.txt - Notepad",
        pieces: &[
            "Here's the plan for the fundraiser. We have the hall booked from six to ten in the evening, and the caterer is confirmed for 150 people.",
            "Sarah is handling the raffle, and she already has about twenty prizes donated, including a weekend at a hotel in the mountains.",
            "The speeches will start at 7:30. So the committee meeting to go over everything is on Tuesday, no wait, Wednesday at ten in the morning, at my house.",
            "Before then, I need everyone to send me their lists of who they invited so we can make sure nobody gets two invitations.",
            "We also need volunteers to set up the tables starting at four, and to clean up afterwards.",
            "The goal this year is to raise $50,000 for the school, which is ten thousand more than last year.",
            "I'm optimistic, the response so far has been really good.",
        ],
        must: &["Sarah", "Wednesday"],
        facts: &[&["150"], &["raffle"], &["7:30"], &["50,000", "50000"], &["ten thousand", "10,000", "10000"], &["volunteers"], &["four", "4"]],
        must_not: &["Tuesday"],
        parts: true,
    },
    Case {
        name: "self-correction in the last sentence",
        exe: "slack.exe",
        title: "general - Slack",
        pieces: &[
            "Quick update on the website project. The new homepage design is done and the team really likes it.",
            "We fixed the bug where the contact form wasn't sending emails, it was a problem with the server settings.",
            "The mobile version still needs some work, especially the menu, which doesn't open properly on older phones.",
            "I've asked Dovid to look at it this week. We also got the new photos from the photographer, and they look great,",
            "so those will go up on the about page. The plan is still to launch at the end of the month.",
            "Can everyone review the staging site and send me comments by Thursday, actually no, make it Friday.",
        ],
        must: &["Dovid", "Friday"],
        facts: &[&["homepage"], &["contact form"], &["menu"], &["photos", "photographer"], &["end of the month"], &["staging"]],
        must_not: &["Thursday"],
        parts: false, // correction cue in the tail
    },
    Case {
        name: "budget numbers",
        exe: "notepad.exe",
        title: "budget.txt - Notepad",
        pieces: &[
            "Monthly budget for the house. Rent is $2,350, utilities usually around $180 in the winter and $120 in the summer.",
            "Groceries we're spending about $900 a month, which is too much, I want to try to get it down to $750.",
            "Tuition for the kids is $1,600 a month for the three of them, and that's after the discount.",
            "Car insurance went up to $210. Phone and internet together are $95.",
            "We give 10% to tzedakah, which comes out to about $650 a month right now.",
            "Savings, I want to put away at least $400 a month, and the rest is for clothes, gifts and Yom Tov expenses.",
            "We should review this again after Sukkos when the new rent starts.",
        ],
        must: &["tzedakah", "Yom Tov", "Sukkos"],
        facts: &[&["2,350", "2350"], &["180"], &["120"], &["900"], &["750"], &["1,600", "1600"], &["210"], &["95"], &["10%", "10 percent"], &["650"], &["400"]],
        must_not: &[],
        parts: true,
    },
    Case {
        name: "wedding logistics",
        exe: "whatsapp.exe",
        title: "WhatsApp",
        pieces: &[
            "Okay, so here's what we know so far about the chasunah. It's going to be on the 18th of June at the hall in Lakewood.",
            "The chuppah is at six thirty, and the kabbalas panim starts at five. Um, the kallah wants all the girls there by four for pictures.",
            "For the sheva brachos, we have Sunday night at Bubby's house and Monday at the Friedmans, we still need someone for Wednesday and Thursday.",
            "The band is confirmed, and the photographer too. We still need to order the bentchers, I think 300 is enough.",
            "Can you ask your sister if she can help with the flowers? She has such good taste.",
            "Oh, and the mechutanim are coming in from Eretz Yisroel on the 15th, so we need to figure out where they're staying.",
        ],
        must: &["chasunah", "Lakewood", "chuppah", "kallah", "sheva brachos", "Bubby", "Friedman", "mechutanim", "Eretz Yisroel"],
        facts: &[&["18th of june", "june 18", "18 june"], &["6:30", "six thirty"], &["300"], &["15th"], &["flowers"], &["photographer"]],
        must_not: &[],
        parts: true,
    },
    Case {
        name: "professional status update",
        exe: "ms-teams.exe",
        title: "Chat | Microsoft Teams",
        pieces: &[
            "Hi team, here's where we are with the quarterly report. The sales numbers came in yesterday and we're up 12% compared to last quarter,",
            "mostly because of the new client in Chicago. Expenses were a bit higher than planned, about $45,000 over,",
            "because we had to hire two contractors to finish the migration on time. Rachel is putting the slides together",
            "and should have a draft ready by Wednesday afternoon. I'll need everyone's section by Monday so she has time.",
            "The presentation to the board is on the 22nd at 2pm. Let's try to keep it under twenty minutes this time,",
            "last quarter it went way over. Thanks everyone for all the hard work.",
        ],
        must: &["Rachel", "Chicago"],
        facts: &[&["12%", "12 percent"], &["45,000", "45000"], &["two contractors", "2 contractors"], &["wednesday"], &["monday"], &["22nd"], &["2pm", "2 pm", "2 p.m.", "2:00"], &["twenty minutes", "20 minutes"]],
        must_not: &[],
        parts: true,
    },
    Case {
        name: "shiur notes",
        exe: "notepad.exe",
        title: "shiur.txt - Notepad",
        pieces: &[
            "Notes from the shiur tonight. The Rav started with the Gemara in Bava Metzia about returning lost objects,",
            "and he asked why the Torah needs a separate mitzvah for this when it's already included in not stealing.",
            "He brought the answer that hashavas aveidah is about caring for another person's property, not just avoiding taking it.",
            "Then we spent a long time on the case where you find something in the street with no identifying marks.",
            "My chavrusa and I want to go over the Tosafos again on Sunday morning because we didn't fully understand it.",
            "Next week the shiur is moving to Tuesday night because of the levaya, the Rav said to bring a Gemara.",
        ],
        must: &["shiur", "Rav", "Gemara", "Bava Metzia", "Torah", "mitzvah", "chavrusa", "levaya"],
        facts: &[&["sunday"], &["tuesday"], &["tosafos", "tosfos"], &["lost objects", "lost object"], &["identifying"]],
        must_not: &[],
        parts: true,
    },
    Case {
        name: "email app (cleaned as a whole by design)",
        exe: "chrome.exe",
        title: "Inbox - Gmail - Google Chrome",
        pieces: &[
            "Hi Mr. Katz, thank you for meeting with me last week about the position. I really enjoyed learning more about the company",
            "and the team. As we discussed, I've attached my references and the writing sample you asked for.",
            "I'm available to start on the first of the month, and I'm happy to come in for a second interview whenever is convenient.",
            "Please let me know if you need anything else from me. Best regards, Yossi Weiss.",
        ],
        must: &["Katz", "Yossi"],
        facts: &[&["references"], &["writing sample"], &["second interview"]],
        must_not: &[],
        parts: false, // Email layout depends on the whole text
    },
    Case {
        name: "rambling voice note with fillers",
        exe: "whatsapp.exe",
        title: "WhatsApp",
        pieces: &[
            "So um, Ma, I just got back from the doctor and like, everything's fine, baruch Hashem, but uh, he wants me to do some more tests,",
            "just to be sure, you know, nothing serious. The appointment is next Thursday at 9:15 at the hospital on Ocean Parkway.",
            "Um, I was wondering if you could, like, watch the kids that morning? Because Yaakov has work and I can't really bring them.",
            "They'll need to be at school by 8:30, the bus comes to the corner, so you'd just need to walk them there.",
            "And uh, the baby has a bottle at ten, it's in the fridge, top shelf. I should be back by like twelve or one.",
            "Thank you so much, I really appreciate it. Tell Tatty I said hi.",
        ],
        must: &["Baruch Hashem", "Yaakov", "Tatty"],
        facts: &[&["thursday"], &["9:15"], &["ocean parkway"], &["8:30"], &["bottle"], &["fridge"], &["ten", "10"]],
        must_not: &[" um", " uh"],
        parts: true,
    },
    Case {
        name: "recipe",
        exe: "notepad.exe",
        title: "recipes.txt - Notepad",
        pieces: &[
            "This is my mother's cholent recipe for Shabbos. Start with two pounds of flanken and about a pound of beans, I use the mix.",
            "Put six potatoes on the bottom of the pot, cut in big pieces, and then the beans and the meat on top.",
            "Add two tablespoons of paprika, a little garlic powder, salt and pepper, and about three tablespoons of honey.",
            "Cover everything with water, about an inch over the top. Put it in the oven at 200 degrees before Shabbos,",
            "and it'll be ready for lunch the next day. If you want, you can add a kishke on top for the last few hours.",
            "My Bubby always said the secret is not to stir it once it's in.",
        ],
        must: &["cholent", "Shabbos", "Bubby", "kishke"],
        facts: &[&["two pounds", "2 pounds", "2 lb"], &["six potatoes", "6 potatoes"], &["paprika"], &["honey"], &["200"], &["inch"], &["flanken"]],
        must_not: &[],
        parts: true,
    },
    Case {
        name: "refuah shleimah update",
        exe: "whatsapp.exe",
        title: "WhatsApp",
        pieces: &[
            "Update on Moshe for everyone who's been asking. Baruch Hashem the surgery went well yesterday and the doctors are happy.",
            "He's still in the hospital and will probably be there for another four or five days. Please continue saying Tehillim,",
            "his name is Moshe ben Sarah. The family really appreciates all the meals, they're covered through next Friday.",
            "If anyone wants to visit, please call first, he gets tired very quickly. Visiting hours are from two to six.",
            "Wishing him a refuah shleimah, and thank you all for the chizuk.",
        ],
        must: &["Moshe", "Baruch Hashem", "Tehillim", "Moshe ben Sarah", "refuah shleimah", "chizuk"],
        facts: &[&["four or five days", "4 or 5 days", "4-5 days", "four to five days"], &["friday"], &["two to six", "2 to 6", "2-6", "2 to 6pm", "2:00"], &["call first"]],
        must_not: &[],
        parts: true,
    },
    Case {
        name: "gemach and tzedakah",
        exe: "winword.exe",
        title: "Gemach - Word",
        pieces: &[
            "The gemach had a really good year, baruch Hashem. We gave out 46 interest free loans, for a total of about $230,000.",
            "Almost all of them have been paid back on time, only three are late and we're in touch with those families.",
            "We also started a new fund for chasunah expenses, which helped eleven families make weddings this year.",
            "The biggest need right now is for Yom Tov, especially before Pesach, when a lot of families are struggling.",
            "If you'd like to help, every donation counts, and of course it's a big zechus. Tizku l'mitzvos.",
            "We're also looking for one more volunteer to help with the paperwork, about two hours a week.",
        ],
        must: &["gemach", "Baruch Hashem", "chasunah", "Yom Tov", "Pesach", "zechus"],
        facts: &[&["46"], &["230,000", "230000"], &["three", "3"], &["eleven", "11"], &["two hours", "2 hours"], &["volunteer"]],
        must_not: &[],
        parts: true,
    },
];

// ---------------------------------------------------------------------------- offline check

/// Returns each transcript unchanged and records what it was asked to clean.
#[derive(Default)]
struct Echo {
    seen: Mutex<Vec<String>>,
}

#[async_trait]
impl TextModel for Echo {
    async fn complete(&self, req: TextRequest) -> Result<TextResponse, ProviderError> {
        let start = req.user.find("<transcript>\n").map(|i| i + 13).unwrap_or(0);
        let end = req.user[start..].find("\n</transcript>").map(|i| start + i).unwrap_or(req.user.len());
        let t = req.user[start..end].to_string();
        self.seen.lock().unwrap().push(t.clone());
        Ok(TextResponse { text: t, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0 })
    }
    fn provider_name(&self) -> &'static str {
        "Echo"
    }
}

#[async_trait]
impl Transcriber for Echo {
    async fn transcribe(&self, _: TranscribeRequest) -> Result<Transcript, ProviderError> {
        Ok(Transcript { text: String::new(), languages: vec![] })
    }
}

struct EchoFactory(Arc<Echo>);

impl ProviderFactory for EchoFactory {
    fn transcriber(&self, _: &ApiKeys) -> Arc<dyn Transcriber> {
        self.0.clone()
    }
    fn text_model(&self, _: &ApiKeys, _: CleanupProvider) -> Arc<dyn TextModel> {
        self.0.clone()
    }
}

/// Runs without a key, everywhere: which dictations of the corpus are tidied in parts, how
/// much is left for the tail, and that the joined text keeps every word exactly once.
#[tokio::test(flavor = "multi_thread")]
async fn corpus_parts_and_tail_sizes() {
    let mut total_words = 0;
    let mut tail_words = 0;
    for c in CASES {
        let echo = Arc::new(Echo::default());
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let keys = ApiKeys { openai: None, anthropic: Some("test".into()) };
        let engine = Engine::new(db, dir.path().to_path_buf(), Settings::default(), keys, Arc::new(EchoFactory(echo.clone())));
        let ctx = CaptureContext { app: apps::identify(c.exe, c.title), ..Default::default() };
        let raw = c.pieces.join(" ");
        let tidy = engine.start_tidy(&ctx, tokio::runtime::Handle::current()).unwrap();
        let mut end_ms = 0u64;
        for (i, p) in c.pieces.iter().enumerate() {
            end_ms += text::word_count(p) as u64 * 400;
            tidy.piece_done(i, (end_ms * 16) as usize, p);
            tidy.idle().await;
        }
        let before_stop = echo.seen.lock().unwrap().len();
        tidy.user_stopped();
        let r = engine.clean_dictation(&raw, &ctx, Some(end_ms), Some(&tidy)).await;
        let seen = echo.seen.lock().unwrap().clone();
        let words = text::word_count(&raw);
        let at_stop: usize = seen[before_stop..].iter().map(|s| text::word_count(s)).sum();
        println!(
            "{:<40} {:>3} words, {} parts while talking, {} words cleaned after stop{}",
            c.name,
            words,
            r.chunked,
            at_stop,
            r.tidy_fallback.as_deref().map(|f| format!(" (whole text: {f})")).unwrap_or_default()
        );
        assert_eq!(r.chunked > 0, c.parts, "{}: {:?}", c.name, r.tidy_fallback);
        assert!(r.error.is_none());
        // Nothing lost or doubled: the echoed parts joined are the transcript (after the
        // dictionary's spelling fixes, as in the whole-text path).
        let whole = engine.clean_text_for(&raw, &ctx, None, Some(end_ms)).await;
        assert_eq!(r.text.split_whitespace().collect::<Vec<_>>(), whole.text.split_whitespace().collect::<Vec<_>>(), "{}", c.name);
        if c.parts {
            let cleaned_once: usize = seen[..before_stop].iter().chain(&seen[before_stop..]).map(|s| text::word_count(s)).sum();
            assert_eq!(cleaned_once, words, "{}: every word cleaned exactly once", c.name);
            total_words += words;
            tail_words += at_stop;
        }
    }
    println!("tidied in parts: {tail_words} of {total_words} words were left for after the stop");
    assert!(tail_words * 2 < total_words, "most of the text should be tidied while talking");
}

fn models_to_test() -> Vec<String> {
    let listed: Vec<String> =
        std::env::var("CHUNKED_EVAL_MODELS").unwrap_or_default().split(',').map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect();
    if !listed.is_empty() {
        return listed;
    }
    vec![std::env::var("WISPER_CLEANUP_MODEL").unwrap_or_else(|_| Settings::default().providers.anthropic_model)]
}

/// Problems with one cleaned text, by the case's checks.
fn check(c: &Case, out: &str) -> Vec<String> {
    let lower = out.to_lowercase();
    let mut problems = Vec::new();
    for m in c.must {
        // Exact spelling, except the first letter may be a capital at the start of a sentence
        // ("Baruch Hashem" / "baruch Hashem").
        let mut ch = m.chars();
        let first = ch.next().unwrap_or(' ');
        let rest = ch.as_str();
        let upper = format!("{}{rest}", first.to_uppercase());
        let lower = format!("{}{rest}", first.to_lowercase());
        if !out.contains(&upper) && !out.contains(&lower) {
            problems.push(format!("missing \"{m}\""));
        }
    }
    for group in c.facts {
        if !group.iter().any(|g| lower.contains(&g.to_lowercase())) {
            problems.push(format!("missing fact {group:?}"));
        }
    }
    for w in c.must_not.iter().chain(WRONG_SPELLINGS) {
        if out.contains(w) {
            problems.push(format!("contains \"{w}\""));
        }
    }
    problems
}

fn median(v: &[u128]) -> u128 {
    let mut v = v.to_vec();
    v.sort_unstable();
    match v.len() {
        0 => 0,
        n if n % 2 == 1 => v[n / 2],
        n => (v[n / 2 - 1] + v[n / 2]) / 2,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn chunked_matches_whole_text() {
    let Some(key) = std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.is_empty()) else {
        eprintln!("ANTHROPIC_API_KEY not set; skipping");
        return;
    };
    let mut failures = Vec::new();
    let mut summary = Vec::new();
    for model in models_to_test() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let mut settings = Settings::default();
        settings.providers.anthropic_model = model.clone();
        let keys = ApiKeys { openai: None, anthropic: Some(key.clone()) };
        let engine = Engine::new(db, dir.path().to_path_buf(), settings, keys, Arc::new(RealProviders));
        println!("===== {model} =====");
        let (mut tail_waits, mut whole_waits, mut used_parts) = (Vec::new(), Vec::new(), 0);
        for c in CASES {
            let ctx = CaptureContext { app: apps::identify(c.exe, c.title), ..Default::default() };
            let raw = c.pieces.join(" ");
            let words = text::word_count(&raw);
            // About 2.5 spoken words a second.
            let duration_ms = words as u64 * 400;

            // Parts path: pieces arrive as the user talks; background work runs in between.
            let tidy = engine.start_tidy(&ctx, tokio::runtime::Handle::current()).expect("tidy while talking is on by default");
            let mut end_ms = 0u64;
            for (i, p) in c.pieces.iter().enumerate() {
                end_ms += text::word_count(p) as u64 * 400;
                tidy.piece_done(i, (end_ms * 16) as usize, p);
                tidy.idle().await;
            }
            let while_talking = tidy.stats();
            tidy.user_stopped();
            let started = Instant::now();
            let parts = engine.clean_dictation(&raw, &ctx, Some(duration_ms), Some(&tidy)).await;
            let tail_wait = started.elapsed().as_millis();

            // Whole-text path (today's), same model.
            let started = Instant::now();
            let whole = engine.clean_text_for(&raw, &ctx, None, Some(duration_ms)).await;
            let whole_wait = started.elapsed().as_millis();

            let how = if parts.chunked > 0 {
                used_parts += 1;
                tail_waits.push(tail_wait);
                whole_waits.push(whole_wait);
                format!("{} parts while talking + tail", parts.chunked)
            } else {
                format!("whole text ({})", parts.tidy_fallback.clone().unwrap_or_else(|| while_talking.off.clone().unwrap_or_else(|| "not started".into())))
            };
            let (pw, ww) = (text::word_count(&parts.text), text::word_count(&whole.text));
            let ratio = pw as f64 / ww.max(1) as f64;
            let mut problems = check(c, &parts.text);
            if let Some(e) = &parts.error {
                problems.push(format!("error: {e}"));
            }
            if !(0.75..=1.25).contains(&ratio) {
                problems.push(format!("{pw} words vs {ww} for the whole text"));
            }
            let whole_problems = check(c, &whole.text);
            println!(
                "{} {} [{}] {} words, {}\n    wait after stop: parts {} ms, whole {} ms; background calls {}\n    parts: {}\n    whole: {}",
                if problems.is_empty() { "PASS" } else { "FAIL" },
                c.name,
                parts.mode_id,
                words,
                how,
                tail_wait,
                whole_wait,
                while_talking.calls,
                parts.text.replace('\n', "\\n"),
                whole.text.replace('\n', "\\n"),
            );
            if !problems.is_empty() {
                println!("    problems (parts): {problems:?}");
                failures.push(format!("{model}: {}: {problems:?}", c.name));
            }
            if !whole_problems.is_empty() {
                println!("    (for comparison) problems with the whole-text path: {whole_problems:?}");
            }
            println!();
        }
        summary.push(format!(
            "{model}: {}/{} used parts; wait after stop (those cases) median {} ms with parts vs {} ms for the whole text",
            used_parts,
            CASES.len(),
            median(&tail_waits),
            median(&whole_waits)
        ));
    }
    println!("===== summary =====");
    for line in &summary {
        println!("{line}");
    }
    assert!(failures.is_empty(), "failed: {failures:#?}");
}

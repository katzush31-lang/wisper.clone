//! The quick path: tiny dictations that the AI cleanup would leave exactly as they are.
//!
//! Most short dictations ("Sounds good, see you tomorrow.") come back from the AI unchanged
//! apart from capitalisation and the final full stop. Sending them costs time (the user waits
//! for a round trip) and money, and adds a small chance of the model doing something odd. The
//! quick path tidies those locally instead.
//!
//! The rule is that skipping the AI must never be a trade-off. So the checks below are
//! deliberately conservative: the text is only handled locally when nothing in it is
//! something the cleanup instructions (`prompts::BASE_RULES` and the mode instructions) would
//! change. Anything that might need the AI declines, and the dictation goes to the AI as usual:
//! fillers, self-corrections, Hebrew/Yiddish terms and their sound-alikes, snippet triggers,
//! a spoken mode switch, numbers, dates, money, email addresses, spoken formatting, lists,
//! email layout, uncommon words (names, terms and mishearings the AI might respell), grammar
//! slips, repetitions, questions without a question mark, and settings or apps where the
//! cleanup does more than tidy (Medium cleanup, Email, AI Prompt, List, Professional and custom
//! modes, edited instructions, email apps, AI assistants, code editors, continuing mid-sentence).
//!
//! `tests/quick_eval.rs` checks this against the real AI: for every dictation the quick path
//! accepts, the AI's result must be the same.
//!
//! The caller still runs `text::fit_to_context` on the result, exactly as for AI output.

use std::collections::HashSet;
use std::fmt;
use std::sync::OnceLock;

use regex::Regex;

use crate::apps::AppCategory;
use crate::common_words;
use crate::dictionary::{find_all, norm_apostrophes, DictEntry};
use crate::modes::{self, Mode};
use crate::prompts::CleanupLevel;
use crate::snippets::Snippet;

/// Longest dictation (in words) the quick path considers. Longer dictations are much more
/// likely to need commas, sentence breaks or paragraphing that only the AI can judge.
pub const DEFAULT_MAX_WORDS: usize = 12;

/// Everything `quick_clean` needs to know about the user's setup and where the text goes.
#[derive(Debug, Clone)]
pub struct QuickContext {
    /// Lower-case phrases that mean "this needs the dictionary-aware AI": every dictionary
    /// word, its automatic replacements and its sound-alikes. Always includes the built-in
    /// Hebrew/Yiddish list, even if the user's dictionary is empty.
    pub dictionary_phrases: Vec<String>,
    /// Spoken snippet triggers.
    pub snippet_triggers: Vec<String>,
    /// All modes (built-in and custom), to recognise a spoken mode switch.
    pub modes: Vec<Mode>,
    /// The mode the pipeline chose for this dictation.
    pub mode: Mode,
    /// The kind of app the text goes into.
    pub category: Option<AppCategory>,
    pub level: CleanupLevel,
    /// The text before the cursor, exactly as the AI would receive it (None when context
    /// awareness is off or the app is excluded).
    pub text_before: Option<String>,
    pub max_words: usize,
}

impl QuickContext {
    pub fn new(
        entries: &[DictEntry],
        snippets: &[Snippet],
        modes: &[Mode],
        mode: &Mode,
        category: Option<AppCategory>,
        level: CleanupLevel,
    ) -> Self {
        let mut seen = HashSet::new();
        let mut phrases = Vec::new();
        let mut add = |p: &str| {
            let p = norm_apostrophes(p.trim()).to_lowercase();
            if !p.is_empty() && seen.insert(p.clone()) {
                phrases.push(p);
            }
        };
        for t in crate::hebrew::STARTER {
            add(t.word);
            t.variants.iter().for_each(|v| add(v));
            t.sounds_like.iter().for_each(|v| add(v));
        }
        for e in entries {
            add(&e.word);
            e.replace.iter().for_each(|v| add(v));
            e.sounds_like.iter().for_each(|v| add(v));
        }
        Self {
            dictionary_phrases: phrases,
            snippet_triggers: snippets.iter().map(|s| s.trigger.trim().to_string()).filter(|t| !t.is_empty()).collect(),
            modes: modes.to_vec(),
            mode: mode.clone(),
            category,
            level,
            text_before: None,
            max_words: DEFAULT_MAX_WORDS,
        }
    }

    pub fn with_text_before(mut self, before: Option<&str>) -> Self {
        self.text_before = before.map(str::to_string);
        self
    }

    pub fn with_max_words(mut self, n: usize) -> Self {
        self.max_words = n;
        self
    }
}

/// Why a dictation was sent to the AI instead (for logs and the evaluation report).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declined {
    /// Short rule name, e.g. "filler".
    pub rule: &'static str,
    /// What triggered it, e.g. "um".
    pub detail: String,
}

impl fmt::Display for Declined {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.detail.is_empty() {
            write!(f, "{}", self.rule)
        } else {
            write!(f, "{}: {}", self.rule, self.detail)
        }
    }
}

fn no(rule: &'static str, detail: impl Into<String>) -> Declined {
    Declined { rule, detail: detail.into() }
}

/// Returns the tidied text when the AI cleanup would not change anything beyond
/// capitalisation and end punctuation; otherwise None (use the AI).
pub fn quick_clean(raw: &str, ctx: &QuickContext) -> Option<String> {
    quick_check(raw, ctx).ok()
}

// ---------------------------------------------------------------------------------------------
// Word lists. Each list says why its words need the AI.
// ---------------------------------------------------------------------------------------------

/// Fillers and verbal tics the cleanup removes ("Remove filler words and verbal tics").
const FILLERS: &[&str] = &[
    "um", "umm", "ummm", "uh", "uhh", "uhm", "er", "erm", "err", "ah", "ahh", "eh", "hmm", "hm", "mm",
    "mmm", "mhm", "huh", "basically", "literally", "anyway", "anyways", "whatever", "like",
];

/// Multi-word fillers.
const FILLER_PHRASES: &[&str] = &[
    "i mean", "sort of", "kind of", "or something", "and stuff", "or whatever", "and everything",
    "let me think", "how do i say", "what's it called", "if you know what i mean",
];

/// "you know" is a filler unless it is part of a question or a clause ("Do you know…",
/// "Let you know"); "you see" is a filler when set off by a comma ("You see, …").
const YOU_KNOW_OK_PREV: &[&str] = &["do", "did", "don't", "didn't", "would", "will", "if", "let", "to", "when", "how", "what"];

/// Self-correction signals: the cleanup keeps only the final version, so the AI must decide
/// what was retracted.
const CORRECTIONS: &[&str] = &[
    "actually", "sorry", "scratch", "correction", "oops", "whoops", "nevermind", "rephrase", "meant", "instead",
];

const CORRECTION_PHRASES: &[&str] = &[
    "no wait", "wait no", "no no", "scratch that", "let me rephrase", "i meant", "make that", "change that",
    "never mind", "hold on", "hang on", "or rather", "start again", "start over", "delete that",
    "let me start", "what i meant", "i said", "not that", "let me try again", "take that back",
];

/// Numbers, ordinals, dates, times and money: the cleanup decides between digits and words
/// ("3pm", "£20", "two or three people"), so the AI must see them.
const NUMBERS: &[&str] = &[
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven",
    "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen", "twenty",
    "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety", "hundred", "hundreds", "thousand",
    "thousands", "million", "millions", "billion", "trillion", "first", "second", "seconds", "third",
    "fourth", "fifth", "sixth", "seventh", "eighth", "ninth", "tenth", "eleventh", "twelfth", "twentieth",
    "thirtieth", "half", "halves", "quarter", "quarters", "dozen", "dozens", "double", "triple", "firstly",
    "secondly", "thirdly", "lastly", "number", "numbers", "o'clock", "percent", "pound", "pounds",
    "quid", "pence", "penny", "dollar", "dollars", "bucks", "cent", "cents", "euro", "euros", "shekel",
    "shekels", "grand", "january", "february", "march", "april", "june", "july", "august", "september",
    "october", "november", "december",
];

/// Prepositions that turn "may" into the month ("in May", "by May").
const MAY_MONTH_BEFORE: &[&str] = &[
    "in", "of", "since", "until", "till", "by", "early", "late", "mid", "next", "last", "this", "end", "from",
];

/// Spoken formatting commands and spoken addresses ("new line", "comma", "david at example
/// dot com"): the cleanup turns them into symbols.
const FORMATTING: &[&str] = &[
    "comma", "commas", "period", "colon", "semicolon", "quote", "unquote", "quotes", "paragraph", "hyphen",
    "dash", "ellipsis", "bracket", "brackets", "parenthesis", "parentheses", "asterisk", "apostrophe",
    "dot", "slash", "backslash", "underscore", "hashtag", "hash", "emoji", "smiley", "ampersand", "www",
    "http", "https", "com",
];

const FORMATTING_PHRASES: &[&str] = &[
    "new line", "next line", "line break", "new paragraph", "next paragraph", "full stop", "question mark",
    "exclamation mark", "exclamation point", "open quote", "close quote", "end quote", "at sign",
    "smiley face", "per cent",
];

/// Structure words: a list or an email layout is the AI's job.
const STRUCTURE: &[&str] = &[
    "list", "lists", "bullet", "bullets", "bulleted", "numbered", "checklist", "dear", "regards", "sincerely",
];

/// Slang and spoken forms the cleanup may write differently depending on the mode.
const SLANG: &[&str] = &[
    "ok", "alright", "gonna", "wanna", "gotta", "kinda", "sorta", "cause", "cuz", "coz", "ain't", "y'all",
    "dunno", "lemme", "gimme", "ya", "yep", "nope", "yup", "lol", "haha", "its", "outta", "til", "till",
];

/// Common speech grammar slips the cleanup fixes ("fix the small grammar slips of speech").
const GRAMMAR_PHRASES: &[&str] = &[
    "should of", "could of", "would of", "must of", "might of", "me and", "your welcome", "your right",
    "your wrong", "your going", "your coming", "there going", "their going", "less people", "more better",
    "he don't", "she don't", "it don't", "we was", "they was", "you was", "him and i", "between you and i",
    "there's lots", "there's loads", "there's some", "there's many",
];

/// Padding the casual mode may trim ("Keep it short by removing padding").
const PADDING_PHRASES: &[&str] = &[
    "just wanted", "wanted to let", "let you know", "just to let", "just to say", "just checking",
    "i was wondering", "just a quick", "quick question",
];

/// Lower-case words the cleanup would capitalise or treat as proper names.
const PROPER_LOWER: &[&str] = &[
    "god", "english", "british", "american", "jewish", "jew", "jews", "christmas", "easter", "israel",
    "england", "london", "america", "internet", "tv", "mr", "mrs", "ms", "dr",
];

const WEEKDAYS: &[&str] = &["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];

/// Everyday chat words missing from `common_words.txt`. None of them is close to a
/// Hebrew/Yiddish dictionary word; names (including "mom", "dad"), which the cleanup may
/// capitalise when used as a name, are left out on purpose.
const EXTRA_WORDS: &[&str] = &[
    "yeah", "hey", "hi", "awesome", "lovely", "perfect", "later", "mail", "couple", "kids", "kid", "forgot",
    "check", "checked", "checking", "sounds", "sounded", "done", "exactly", "appreciate", "appreciated",
    "friends", "paid", "bill", "bought", "photo", "photos", "video", "videos", "address", "rather",
    "thanks", "thank", "bye", "goodbye", "congrats", "congratulations", "fantastic", "wonderful",
    "brilliant", "amazing", "excellent", "definitely", "hopefully", "honestly", "totally", "absolutely",
    "tonight", "tomorrow", "yesterday", "today", "weekend", "morning", "evening", "afternoon", "night",
    "dinner", "lunch", "breakfast", "groceries", "pharmacy", "parking", "traffic", "stuck", "heading",
    "omw", "busy", "free", "ready", "almost", "nearly", "soon", "asap", "fun", "safe", "safely", "landed",
    "arrived", "anytime", "welcome", "cute", "sweet", "proud", "excited", "tired", "sick", "feel", "feeling",
    "hug", "hugs", "kiss", "miss", "missed", "missing", "sleep", "slept", "nap", "shower", "laundry",
    "dishes", "fridge", "oven", "pizza", "pasta", "salad", "chicken", "coffee", "tea", "cake", "snack",
    "snacks", "bread", "eggs", "cheese", "juice", "store", "shop", "shopping", "mall", "gym", "park",
    "doctor", "dentist", "appointment", "meeting", "meetings", "call", "calls", "calling", "called",
    "text", "texted", "texting", "email", "emailed", "message", "messages", "reply", "replied", "update",
    "updates", "updated", "deadline", "draft", "slides", "invoice", "receipt", "budget", "client",
    "clients", "project", "projects", "review", "reviewed", "approve", "approved", "sign", "signed",
    "schedule", "scheduled", "reschedule", "cancel", "cancelled", "canceled", "confirm", "confirmed",
    "sorted", "fixed", "broken", "charger", "laptop", "password", "wifi", "printer", "keys", "wallet",
    "jacket", "umbrella", "raining", "rain", "sunny", "cold", "hot", "warm", "drive", "driving", "drove",
    "ride", "uber", "taxi", "train", "bus", "flight", "airport", "hotel", "trip", "vacation", "holiday",
    "birthday", "anniversary", "party", "gift", "present", "card", "flowers", "cookies", "dessert",
    "tidy", "clean", "cleaned", "cleaning", "bed", "bedtime", "homework", "teacher", "class", "classes",
    "practice", "game", "match", "won", "lost", "score", "team", "coach", "pickup", "drop", "dropped",
    "grab", "grabbed", "grabbing", "discuss", "discussed", "approach", "sense", "poem", "ignore", "ignored",
    "previous", "plan", "plans", "screen", "maybe", "probably", "basically", "seriously", "literally", "anyway",
];

/// Contractions (the common-words check only handles stems of three letters or more).
const CONTRACTIONS: &[&str] = &[
    "i'm", "i'll", "i've", "i'd", "you're", "you'll", "you've", "you'd", "we're", "we'll", "we've", "we'd",
    "they're", "they'll", "they've", "they'd", "he's", "he'll", "he'd", "she's", "she'll", "she'd", "it's",
    "it'll", "that's", "that'll", "there's", "here's", "what's", "who's", "where's", "when's", "how's",
    "let's", "don't", "doesn't", "didn't", "can't", "won't", "wouldn't", "shouldn't", "couldn't", "isn't",
    "aren't", "wasn't", "weren't", "haven't", "hasn't", "hadn't", "mustn't", "needn't",
];

/// Words that open a question. A sentence starting with one and ending in a full stop (or
/// with no end mark) may be a question the AI would end with "?".
const QUESTION_OPENERS: &[&str] = &[
    "what", "when", "where", "why", "how", "who", "whom", "whose", "which", "any", "anyone", "anything",
    "want", "you",
];

/// Auxiliary verbs open a question only when a subject follows ("Have you eaten", but "Have a
/// safe flight").
const AUX_OPENERS: &[&str] = &[
    "can", "could", "would", "will", "do", "does", "did", "is", "are", "was", "were", "should", "shall", "have",
    "has", "had", "may", "might", "am", "must", "isn't", "aren't", "don't", "doesn't", "didn't", "won't",
    "can't", "wouldn't", "shouldn't", "couldn't", "haven't", "hasn't", "wasn't", "weren't",
];
const SUBJECTS: &[&str] = &[
    "you", "we", "they", "i", "he", "she", "it", "anyone", "anybody", "everyone", "everybody", "someone",
    "somebody", "the", "your", "my", "our", "their", "his", "her", "this", "that", "these", "those", "there",
];

/// Interjections that the cleanup separates with a comma when more words follow ("Yes,
/// please", "Okay, see you then"). They must already be followed by punctuation.
const INTERJECTIONS: &[&str] = &[
    "yes", "yeah", "no", "okay", "sure", "hi", "hello", "hey", "oh", "ah", "wow", "cheers", "bye", "goodbye",
    "morning", "absolutely", "definitely", "honestly", "anyway",
];

/// What may follow an interjection directly ("Hey there", "Sure thing", "No problem").
fn interjection_continuation_ok(first: &str, next: &str) -> bool {
    match first {
        "no" => matches!(
            next,
            "problem" | "problems" | "worries" | "thanks" | "idea" | "way" | "longer" | "more" | "need" | "rush"
                | "matter" | "doubt" | "big" | "pressure" | "hurry" | "clue" | "chance"
        ),
        "hi" | "hello" | "hey" => next == "there",
        "sure" => next == "thing",
        "bye" => next == "for",
        "definitely" | "absolutely" | "honestly" => !matches!(next, "i" | "i'll" | "i'm" | "we" | "we'll" | "you" | "it's" | "that's"),
        _ => false,
    }
}

/// Words that praise or react ("Great", "Perfect"). Followed directly by a new clause they
/// need a comma or full stop ("Great, see you then"), which the AI would add.
const REACTIONS: &[&str] = &[
    "great", "perfect", "cool", "awesome", "nice", "lovely", "excellent", "brilliant", "amazing", "good",
    "fantastic", "wonderful", "thanks", "thank", "congrats", "congratulations", "fine", "right",
];

/// Words that start a new clause; after a reaction word without punctuation, the AI would
/// insert a comma or a sentence break.
const CLAUSE_STARTERS: &[&str] = &[
    "i", "i'll", "i'm", "i've", "i'd", "we", "we'll", "we're", "we've", "you", "you're", "you'll", "he", "she",
    "they", "he's", "she's", "they're", "it's", "that's", "there's", "let's", "see", "talk", "speak", "call",
    "chat", "get", "have", "enjoy", "will", "can", "could", "do", "sounds", "so", "and", "but",
    "just", "looking", "glad", "happy", "done", "okay", "sure", "yes", "yeah", "no", "thanks", "thank",
    "great", "perfect", "cool", "nice", "lovely", "bye", "love", "please", "what", "when", "how", "is", "are",
    "was", "send", "let", "keep", "take", "drive", "text", "message", "go", "come", "be", "it", "this",
    "that", "the", "everyone", "everybody", "all", "guys", "mate", "again",
];

/// What may follow "thanks" / "thank you" directly ("Thanks so much", "Thank you for…").
const THANKS_CONTINUATIONS: &[&str] = &["so", "for", "a", "again", "very", "too", "heaps", "loads", "lots", "ever"];

/// Words the AI may set off with a comma at the end of a sentence ("See you later, everyone",
/// "Send it, please").
const END_TAGS: &[&str] = &[
    "please", "yeah", "yes", "thanks", "though", "everyone", "everybody", "guys", "folks", "mate", "buddy",
    "dude", "babe", "honey", "sweetie", "darling", "sir", "madam", "bro", "dear",
];

/// Words that can follow "like" when it is the verb or a comparison, not a filler.
const LIKE_AFTER_SUBJECT: &[&str] = &[
    "it", "that", "this", "the", "to", "your", "my", "our", "his", "her", "them", "him", "you", "a", "an", "how",
    "what", "when", "those", "these", "their",
];
const LIKE_OK_PREV: &[&str] = &[
    "would", "i'd", "we'd", "you'd", "they'd", "he'd", "she'd", "looks", "look", "looked", "sounds", "sound",
    "sounded", "seems", "seem", "seemed", "feels", "feel", "felt", "don't", "didn't", "doesn't", "really",
];

/// "wait" is a self-correction signal, except in phrases like "can't wait" or "wait for".
const WAIT_OK_PREV: &[&str] = &[
    "can't", "cannot", "to", "i'll", "we'll", "will", "don't", "won't", "not", "couldn't", "could", "should",
    "would", "must", "please",
];
const WAIT_OK_NEXT: &[&str] = &["for", "until", "till"];

/// Words the cleanup often drops when they open a sentence ("So, …", "And …", "Well, …").
const LEAD_INS: &[&str] = &["so", "well", "and"];

/// Words that make an exclamation likely, so when the transcript has no end mark the AI may
/// choose "!" rather than ".".
const EXCLAIMY: &[&str] = &[
    "congratulations", "congrats", "wow", "yay", "hooray", "amazing", "awesome", "fantastic", "brilliant",
    "wonderful", "happy", "welcome", "thanks", "thank", "love", "great", "perfect", "excellent", "lovely",
    "hi", "hello", "hey", "bye", "goodbye", "cheers", "enjoy", "can't",
];

fn set(words: &[&'static str]) -> HashSet<&'static str> {
    words.iter().copied().collect()
}

// ---------------------------------------------------------------------------------------------
// The check.
// ---------------------------------------------------------------------------------------------

struct Token {
    /// The word as spoken/transcribed (apostrophes normalised).
    word: String,
    lower: String,
    /// Punctuation that followed it: None or one of , . ? !
    punct: Option<char>,
}

/// Like `quick_clean`, but says which rule sent the dictation to the AI.
pub fn quick_check(raw: &str, ctx: &QuickContext) -> Result<String, Declined> {
    check_setup(ctx)?;
    let s = normalise(raw)?;
    let tokens = tokenise(&s)?;
    let words = tokens.len();
    if words == 0 {
        return Err(no("empty", ""));
    }
    if words > ctx.max_words {
        return Err(no("too long", format!("{words} words")));
    }
    if !s.contains([',', '.', '?', '!']) && words > 3 {
        // The recogniser left out all punctuation; where commas and sentence breaks go is a
        // judgement call for the AI ("thanks see you soon" → "Thanks, see you soon.").
        return Err(no("no punctuation", ""));
    }

    // Hebrew/Yiddish terms, their spellings and sound-alikes, and the user's own words.
    for p in &ctx.dictionary_phrases {
        if !find_all(&s, p).is_empty() {
            return Err(no("dictionary term or sound-alike", p.clone()));
        }
    }
    for t in &ctx.snippet_triggers {
        if !find_all(&s, &norm_apostrophes(t)).is_empty() {
            return Err(no("snippet trigger", t.clone()));
        }
    }

    let lower_spaced = format!(" {} ", tokens.iter().map(|t| t.lower.as_str()).collect::<Vec<_>>().join(" "));
    let has_phrase = |list: &[&'static str]| list.iter().find(|p| lower_spaced.contains(&format!(" {p} "))).copied();
    if let Some(p) = has_phrase(FILLER_PHRASES) {
        return Err(no("filler", p));
    }
    for (k, w) in tokens.windows(2).enumerate() {
        if w[0].lower == "you" && w[0].punct.is_none() && w[1].lower == "know" {
            let prev = if k > 0 { Some(&tokens[k - 1]) } else { None };
            let ok = prev.map(|p| p.punct.is_none() && YOU_KNOW_OK_PREV.contains(&p.lower.as_str())).unwrap_or(false);
            if !ok {
                return Err(no("filler", "you know"));
            }
        }
        if w[0].lower == "you" && w[0].punct.is_none() && w[1].lower == "see" && w[1].punct == Some(',') {
            return Err(no("filler", "you see"));
        }
    }
    if let Some(p) = has_phrase(CORRECTION_PHRASES) {
        return Err(no("self-correction", p));
    }
    if let Some(p) = has_phrase(FORMATTING_PHRASES) {
        return Err(no("spoken formatting", p));
    }
    if let Some(p) = has_phrase(GRAMMAR_PHRASES) {
        return Err(no("grammar slip", p));
    }
    if let Some(p) = has_phrase(PADDING_PHRASES) {
        return Err(no("padding", p));
    }

    check_modes(&s, ctx)?;
    check_repetition(&tokens)?;
    check_words(&tokens)?;
    check_sentences(&tokens)?;
    Ok(render(&tokens))
}

/// Only Light cleanup in the Auto or Casual mode with their built-in instructions, and never
/// where the cleanup reshapes short text (email layout, AI prompts, code) or continues a
/// sentence already in the field.
fn check_setup(ctx: &QuickContext) -> Result<(), Declined> {
    if ctx.level != CleanupLevel::Light {
        return Err(no("cleanup level", format!("{:?}", ctx.level)));
    }
    let m = &ctx.mode;
    let mode_ok = m.builtin
        && (m.id == modes::AUTO || m.id == "casual")
        && modes::default_instructions(&m.id).map(|d| d.trim() == m.instructions.trim()).unwrap_or(false);
    if !mode_ok {
        return Err(no("mode", m.id.clone()));
    }
    match ctx.category {
        Some(AppCategory::Email) | Some(AppCategory::AiAssistant) | Some(AppCategory::Code) => {
            return Err(no("app", format!("{:?}", ctx.category.unwrap())));
        }
        _ => {}
    }
    if let Some(b) = &ctx.text_before {
        let b = b.trim_end_matches([' ', '\t']);
        let ok = b.trim().is_empty() || b.ends_with(['\n', '\r', '.', '?', '!']);
        if !ok {
            return Err(no("continues a sentence", ""));
        }
    }
    Ok(())
}

/// Straightens apostrophes, rejects anything but plain words and , . ? ! and fixes spacing.
fn normalise(raw: &str) -> Result<String, Declined> {
    let s = norm_apostrophes(raw).trim().to_string();
    if s.is_empty() {
        return Err(no("empty", ""));
    }
    if s.contains(['\n', '\r']) {
        return Err(no("line breaks", ""));
    }
    if let Some(c) = s.chars().find(|c| c.is_ascii_digit()) {
        return Err(no("number", c.to_string()));
    }
    if let Some(c) = s.chars().find(|c| !(c.is_ascii_alphabetic() || c.is_whitespace() || ",.?!'".contains(*c))) {
        // Symbols (@ £ % & - : ; " … ⟦), accented or non-Latin letters (Hebrew script).
        return Err(no("symbol or non-English letter", c.to_string()));
    }
    static RES: OnceLock<[Regex; 3]> = OnceLock::new();
    let [space_before, no_space_after, repeated] = RES.get_or_init(|| {
        [Regex::new(r" ([,.?!])").unwrap(), Regex::new(r"([,.?!])([A-Za-z'])").unwrap(), Regex::new(r"[,.?!]{2,}").unwrap()]
    });
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = space_before.replace_all(&s, "$1").to_string();
    let s = no_space_after.replace_all(&s, "$1 $2").to_string();
    if repeated.is_match(&s) {
        return Err(no("repeated punctuation", ""));
    }
    if s.starts_with([',', '.', '?', '!']) {
        return Err(no("leading punctuation", ""));
    }
    if s.ends_with(',') {
        return Err(no("ends with a comma", ""));
    }
    Ok(s)
}

fn tokenise(s: &str) -> Result<Vec<Token>, Declined> {
    let mut out = Vec::new();
    for raw in s.split(' ') {
        let (word, punct) = match raw.chars().last() {
            Some(c @ (',' | '.' | '?' | '!')) => (&raw[..raw.len() - 1], Some(c)),
            _ => (raw, None),
        };
        let ok = !word.is_empty()
            && !word.starts_with('\'')
            && !word.ends_with('\'')
            && !word.contains("''")
            && word.matches('\'').count() <= 1
            && word.chars().all(|c| c.is_ascii_alphabetic() || c == '\'');
        if !ok {
            return Err(no("unusual word shape", raw));
        }
        out.push(Token { word: word.to_string(), lower: word.to_lowercase(), punct });
    }
    Ok(out)
}

/// A spoken mode switch, or a mode named the way a switch would be ("make it a list",
/// "email format"), means the AI must format the text.
fn check_modes(s: &str, ctx: &QuickContext) -> Result<(), Declined> {
    if let Some((id, _)) = modes::detect_spoken_switch(s, &ctx.modes) {
        return Err(no("spoken mode switch", id));
    }
    let mut names: Vec<String> = Vec::new();
    for m in &ctx.modes {
        names.push(m.name.to_lowercase());
        names.extend(m.aliases.iter().map(|a| a.to_lowercase()));
    }
    names.retain(|n| !n.trim().is_empty());
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    names.dedup();
    if names.is_empty() {
        return Ok(());
    }
    let alts = names.iter().map(|n| regex::escape(n).replace(' ', r"[\s\-]+")).collect::<Vec<_>>().join("|");
    let lead = r"(?:as|in|into|use|using|format(?:\s+(?:it|this))?(?:\s+as)?|make\s+(?:it|this)(?:\s+(?:into|as))?|turn\s+(?:it|this)\s+into|write\s+(?:it|this)\s+as|switch\s+to|change\s+to)";
    let re = Regex::new(&format!(r"(?i)\b{lead}\s+(?:an?\s+|the\s+)?(?:{alts})\b|\b(?:{alts})\s+(?:mode|format|style|version)\b"));
    match re {
        Ok(re) => match re.find(s) {
            Some(m) => Err(no("mode name", m.as_str().to_string())),
            None => Ok(()),
        },
        // A mode name that can't be turned into a pattern: be safe.
        Err(_) => Err(no("mode name", "unreadable mode name")),
    }
}

/// Repeated words or word pairs are usually false starts ("I I think", "we should, we
/// should go") or emphasis ("really really") that the AI handles.
fn check_repetition(tokens: &[Token]) -> Result<(), Declined> {
    for w in tokens.windows(2) {
        if w[0].lower == w[1].lower {
            return Err(no("repetition", w[0].lower.clone()));
        }
    }
    let mut pairs = HashSet::new();
    for w in tokens.windows(2) {
        let pair = format!("{} {}", w[0].lower, w[1].lower);
        if !pairs.insert(pair.clone()) {
            return Err(no("repetition", pair));
        }
    }
    Ok(())
}

fn check_words(tokens: &[Token]) -> Result<(), Declined> {
    let fillers = set(FILLERS);
    let corrections = set(CORRECTIONS);
    let numbers = set(NUMBERS);
    let formatting = set(FORMATTING);
    let structure = set(STRUCTURE);
    let slang = set(SLANG);
    let proper = set(PROPER_LOWER);
    let extra = set(EXTRA_WORDS);
    let contractions = set(CONTRACTIONS);

    for (i, t) in tokens.iter().enumerate() {
        let w = t.lower.as_str();
        let prev = if i > 0 { Some(&tokens[i - 1]) } else { None };
        let next = tokens.get(i + 1);
        // Directly adjacent: no punctuation between the two words.
        let prev_adj = prev.filter(|p| p.punct.is_none()).map(|p| p.lower.as_str());
        let next_adj = next.filter(|_| t.punct.is_none()).map(|n| n.lower.as_str());

        if w == "like" {
            let ok = match prev_adj {
                Some(p) if LIKE_OK_PREV.contains(&p) => true,
                Some("i" | "you" | "we" | "they") => next_adj.map(|n| LIKE_AFTER_SUBJECT.contains(&n)).unwrap_or(false),
                _ => false,
            };
            if !ok {
                return Err(no("filler", "like"));
            }
            continue;
        }
        if w == "wait" {
            let ok = prev_adj.map(|p| WAIT_OK_PREV.contains(&p)).unwrap_or(false)
                || next_adj.map(|n| WAIT_OK_NEXT.contains(&n)).unwrap_or(false);
            if !ok {
                return Err(no("self-correction", "wait"));
            }
            continue;
        }
        if w == "no" && i > 0 {
            // "Tuesday, no, Wednesday". At the start ("No, I can't") it's an answer, which the
            // interjection rule checks.
            let ok = next_adj.map(|n| interjection_continuation_ok("no", n) || n == "one").unwrap_or(false);
            if !ok {
                return Err(no("self-correction", "no"));
            }
        }
        if w == "may" && prev_adj.map(|p| MAY_MONTH_BEFORE.contains(&p)).unwrap_or(false) {
            return Err(no("date", "may"));
        }
        if fillers.contains(w) {
            return Err(no("filler", w));
        }
        if corrections.contains(w) {
            return Err(no("self-correction", w));
        }
        if numbers.contains(w) {
            return Err(no("number, date or money", w));
        }
        if formatting.contains(w) {
            return Err(no("spoken formatting", w));
        }
        if structure.contains(w) {
            return Err(no("list or email structure", w));
        }
        if slang.contains(w) {
            return Err(no("slang or spoken form", w));
        }
        if proper.contains(w) {
            return Err(no("proper noun", w));
        }
        if w.len() == 1 && w != "a" && w != "i" {
            return Err(no("single letter", w));
        }
        // "a apple" / "an car": the AI fixes the article.
        if let Some(n) = next_adj {
            if w == "a" && starts_with_vowel_sound(n) {
                return Err(no("grammar slip", format!("a {n}")));
            }
            if w == "an" && !starts_with_vowel_sound(n) && !n.starts_with('h') && !n.starts_with('u') {
                return Err(no("grammar slip", format!("an {n}")));
            }
        }
        // Vocabulary: only everyday English. Anything else could be a name, a Hebrew/Yiddish
        // word or a mishearing the AI would respell.
        let known = common_words::is_common(w) || extra.contains(w) || contractions.contains(w) || WEEKDAYS.contains(&w) || w == "may";
        if !known {
            return Err(no("uncommon word", t.word.clone()));
        }
        // Casing: lower case, or a capital first letter only.
        let caps = t.word.chars().filter(|c| c.is_ascii_uppercase()).count();
        let first_upper = t.word.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false);
        if caps > 1 || (caps == 1 && !first_upper) {
            return Err(no("unusual capitals", t.word.clone()));
        }
    }

    Ok(())
}

fn starts_with_vowel_sound(word: &str) -> bool {
    let silent_h = ["hour", "honest", "honour", "honor", "heir"].iter().any(|h| word.starts_with(h));
    let u_as_you = ["uni", "use", "usu", "uk", "ufo"].iter().any(|p| word.starts_with(p));
    let o_as_w = word.starts_with("one") || word.starts_with("once");
    silent_h || (word.starts_with(['a', 'e', 'i', 'o', 'u']) && !u_as_you && !o_as_w)
}

/// Sentence-level checks: capitals only where a sentence starts, interjections already set
/// off by punctuation, questions already marked, nothing the AI would punctuate differently.
fn check_sentences(tokens: &[Token]) -> Result<(), Declined> {
    let reactions = set(REACTIONS);
    let clause_starters = set(CLAUSE_STARTERS);
    let mut start = 0;
    let last_index = tokens.len() - 1;
    while start < tokens.len() {
        let mut end = start;
        while end < last_index && !matches!(tokens[end].punct, Some('.' | '?' | '!')) {
            end += 1;
        }
        let sentence = &tokens[start..=end];
        let first = &sentence[0];
        let end_mark = sentence.last().and_then(|t| t.punct).filter(|c| matches!(c, '.' | '?' | '!'));

        // Capitals: only the sentence start, "I" and weekdays may be capitalised.
        for (k, t) in sentence.iter().enumerate() {
            let upper = t.word.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false);
            let allowed = k == 0 || t.lower == "i" || t.lower.starts_with("i'") || WEEKDAYS.contains(&t.lower.as_str());
            if upper && !allowed {
                return Err(no("capitalised word mid-sentence", t.word.clone()));
            }
        }

        // Lead-ins the cleanup tends to drop ("So, …", "And then …"), at the start of a
        // sentence or straight after an opening interjection ("Okay, so …").
        for (k, t) in sentence.iter().enumerate().take(2) {
            let at_start = k == 0 || sentence[0].punct == Some(',');
            let well_done = t.lower == "well" && sentence.get(k + 1).map(|n| n.lower == "done").unwrap_or(false);
            if at_start && LEAD_INS.contains(&t.lower.as_str()) && sentence.len() > k + 1 && !well_done {
                return Err(no("lead-in", t.lower.clone()));
            }
        }

        // Interjections and reactions that run straight into more words.
        if sentence.len() > 1 && first.punct.is_none() {
            let next = sentence[1].lower.as_str();
            let f = first.lower.as_str();
            if INTERJECTIONS.contains(&f) && !interjection_continuation_ok(f, next) {
                return Err(no("needs a comma after the first word", format!("{f} {next}")));
            }
            if f == "thanks" && !THANKS_CONTINUATIONS.contains(&next) {
                return Err(no("needs a comma after the first word", format!("{f} {next}")));
            }
            if f == "thank" {
                if next != "you" {
                    return Err(no("needs a comma after the first word", format!("{f} {next}")));
                }
                if sentence.len() > 2 && sentence[1].punct.is_none() && !THANKS_CONTINUATIONS.contains(&sentence[2].lower.as_str()) {
                    return Err(no("needs a comma after the first word", format!("thank you {}", sentence[2].lower)));
                }
            } else if reactions.contains(f) && f != "thanks" && clause_starters.contains(next) {
                return Err(no("needs a comma after the first word", format!("{f} {next}")));
            }
        }
        // A word the AI would set off at the end ("See you later, everyone").
        if sentence.len() > 1 {
            let last = &sentence[sentence.len() - 1];
            let before_last = &sentence[sentence.len() - 2];
            if END_TAGS.contains(&last.lower.as_str()) && before_last.punct.is_none() {
                let set_phrase = last.lower == "thanks" && matches!(before_last.lower.as_str(), "many" | "no");
                if !set_phrase {
                    return Err(no("needs a comma before the last word", last.lower.clone()));
                }
            }
        }
        // Questions: a question opener must end with "?".
        let opener = QUESTION_OPENERS.contains(&first.lower.as_str())
            || (AUX_OPENERS.contains(&first.lower.as_str())
                && first.punct.is_none()
                && sentence.get(1).map(|n| SUBJECTS.contains(&n.lower.as_str())).unwrap_or(false));
        // "Okay. Sounds good.": the AI may join a one-word sentence to the next with a comma.
        if sentence.len() == 1 && end_mark == Some('.') && end < last_index {
            return Err(no("one-word sentence", first.lower.clone()));
        }

        // "I'm at the store, do you need anything?": the AI may split the statement from the
        // question.
        if end_mark == Some('?') {
            for k in 1..sentence.len() {
                let t = &sentence[k];
                let is_q = QUESTION_OPENERS.contains(&t.lower.as_str())
                    || (AUX_OPENERS.contains(&t.lower.as_str())
                        && t.punct.is_none()
                        && sentence.get(k + 1).map(|n| SUBJECTS.contains(&n.lower.as_str())).unwrap_or(false));
                if sentence[k - 1].punct == Some(',') && is_q {
                    return Err(no("question after a comma", t.lower.clone()));
                }
            }
        }
        match end_mark {
            Some('?') | Some('!') => {}
            Some(_) if opener => return Err(no("may be a question", first.lower.clone())),
            None if opener => return Err(no("may be a question", first.lower.clone())),
            None => {
                // We'd add a full stop; if the AI might choose "!" instead, let it decide.
                if let Some(t) = sentence.iter().find(|t| EXCLAIMY.contains(&t.lower.as_str())) {
                    return Err(no("may be an exclamation", t.lower.clone()));
                }
            }
            _ => {}
        }
        start = end + 1;
    }
    Ok(())
}

/// The local tidy: capital at each sentence start, "I" and weekdays capitalised, single
/// spaces, and a full stop at the end if there was no end mark.
fn render(tokens: &[Token]) -> String {
    let mut out = String::new();
    let mut sentence_start = true;
    for (i, t) in tokens.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut w = t.word.clone();
        if sentence_start || t.lower == "i" || t.lower.starts_with("i'") || WEEKDAYS.contains(&t.lower.as_str()) {
            let mut c = w.chars();
            if let Some(f) = c.next() {
                w = f.to_uppercase().collect::<String>() + c.as_str();
            }
        }
        out.push_str(&w);
        if let Some(p) = t.punct {
            out.push(p);
        }
        sentence_start = matches!(t.punct, Some('.' | '?' | '!'));
    }
    if !out.ends_with(['.', '?', '!']) {
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::starter_entries;
    use crate::modes::builtin_modes;

    fn mode(id: &str) -> Mode {
        builtin_modes().into_iter().find(|m| m.id == id).unwrap()
    }

    fn ctx_for(mode_id: &str, cat: Option<AppCategory>) -> QuickContext {
        let snippets = vec![Snippet { id: 1, trigger: "my address".into(), expansion: "1 High St".into(), uses: 0 }];
        let mut entries = starter_entries();
        let mut menachem = DictEntry::user("Menachem");
        menachem.sounds_like = vec!["men a hem".into()];
        entries.push(menachem);
        QuickContext::new(&entries, &snippets, &builtin_modes(), &mode(mode_id), cat, CleanupLevel::Light)
    }

    fn chat() -> QuickContext {
        ctx_for("casual", Some(AppCategory::PersonalChat))
    }

    fn doc() -> QuickContext {
        ctx_for("auto", Some(AppCategory::Document))
    }

    /// Accepted in a chat app (Casual) and a document (Auto), with exactly this output.
    fn accepts(raw: &str, want: &str) {
        for c in [chat(), doc()] {
            assert_eq!(quick_check(raw, &c), Ok(want.to_string()), "{raw:?} in {}", c.mode.id);
            assert_eq!(quick_clean(raw, &c).as_deref(), Some(want));
        }
    }

    /// Sent to the AI in both, for the given rule.
    fn declines(raw: &str, rule: &str) {
        for c in [chat(), doc()] {
            match quick_check(raw, &c) {
                Ok(t) => panic!("{raw:?} should go to the AI ({rule}) but the quick path gave {t:?}"),
                Err(d) => assert_eq!(d.rule, rule, "{raw:?}: {d}"),
            }
            assert_eq!(quick_clean(raw, &c), None);
        }
    }

    #[test]
    fn tidies_simple_sentences() {
        accepts("Sounds good, see you tomorrow.", "Sounds good, see you tomorrow.");
        accepts("sounds good", "Sounds good.");
        accepts("Thanks so much!", "Thanks so much!");
        accepts("see you soon", "See you soon.");
        accepts("Can you send me the file when you get a chance?", "Can you send me the file when you get a chance?");
        accepts("I'm on my way.", "I'm on my way.");
        accepts("That works for me.", "That works for me.");
        accepts("No problem.", "No problem.");
        accepts("No, I can't make it tonight.", "No, I can't make it tonight.");
        accepts("Great job.", "Great job.");
        accepts("Well done!", "Well done!");
        accepts("Can't wait to see you!", "Can't wait to see you!");
        accepts("I'd like to book a table.", "I'd like to book a table.");
        accepts("It looks like rain.", "It looks like rain.");
        accepts("Sure thing.", "Sure thing.");
        accepts("Yes, please.", "Yes, please.");
        accepts("Thank you for your help.", "Thank you for your help.");
        accepts("Where are you?", "Where are you?");
        accepts("Have a safe flight.", "Have a safe flight.");
        accepts("Did you see my message?", "Did you see my message?");
        accepts("Do you know where it is?", "Do you know where it is?");
        accepts("Good catch, I'll fix it.", "Good catch, I'll fix it.");
    }

    #[test]
    fn local_tidy_fixes_case_spacing_and_end() {
        // Capital first letter, "I", weekdays, single spaces, full stop added.
        accepts("i'm  on my way.", "I'm on my way.");
        accepts("on my way", "On my way.");
        accepts("let's meet on monday.", "Let's meet on Monday.");
        accepts("i think that works .", "I think that works.");
        accepts("Sounds good.see you then.", "Sounds good. See you then.");
        accepts("  I'll be home soon.  ", "I'll be home soon.");
        // Curly apostrophes are straightened, as the AI path does.
        accepts("I\u{2019}ll call you later.", "I'll call you later.");
        // ? and ! are kept.
        accepts("Are you home?", "Are you home?");
        accepts("That was fun!", "That was fun!");
    }

    #[test]
    fn fillers_need_the_ai() {
        declines("Um, sounds good.", "filler");
        declines("Sounds good, uh, see you then.", "filler");
        declines("It was like really good.", "filler");
        declines("I'm, you know, not sure.", "filler");
        declines("It was you know great.", "filler");
        declines("You see, it's late.", "filler");
        declines("It's sort of late.", "filler");
        declines("Basically we're done.", "filler");
        declines("So, see you tomorrow.", "lead-in");
        declines("And then we left.", "lead-in");
        declines("Okay, so I'm home.", "lead-in");
        declines("Well, it went fine.", "lead-in");
    }

    #[test]
    fn self_corrections_need_the_ai() {
        declines("Let's meet Tuesday, no wait, Wednesday.", "self-correction");
        declines("Wait, I'll call you.", "self-correction");
        declines("Send it to me, I mean to him.", "filler"); // "I mean" is both; caught first as a filler
        declines("Actually, I can make it.", "self-correction");
        declines("I'll bring cake, scratch that, bring fruit.", "self-correction");
        declines("Sorry, I meant tomorrow.", "self-correction");
        declines("Tuesday, no, the day after.", "self-correction");
        declines("Let's go home, or rather to the park.", "self-correction");
        declines("I I think so.", "repetition");
        declines("We should, we should go.", "repetition");
        declines("It's really really good.", "repetition");
    }

    #[test]
    fn hebrew_and_yiddish_terms_need_the_ai() {
        declines("Good Shabbos.", "dictionary term or sound-alike");
        declines("See you on shabbat.", "dictionary term or sound-alike");
        declines("Thank Hashem we're home.", "dictionary term or sound-alike");
        declines("Mazel tov!", "dictionary term or sound-alike");
        declines("It's a big zeus.", "dictionary term or sound-alike");
        declines("What a Zeus that was.", "dictionary term or sound-alike");
        declines("Such parnassus.", "dictionary term or sound-alike");
        declines("We had a good walk.", "dictionary term or sound-alike");
        declines("I'm going to shul.", "dictionary term or sound-alike");
        declines("He's at kollel.", "dictionary term or sound-alike");
        // The user's own words and their sound-alikes too.
        declines("Tell menachem I said hello.", "dictionary term or sound-alike");
        declines("Tell men a hem.", "dictionary term or sound-alike");
        // Unknown words could be Hebrew/Yiddish terms or names the AI would respell.
        declines("See you at the simchah.", "dictionary term or sound-alike");
        declines("The chazzan was great.", "uncommon word");
        declines("Thanks David.", "uncommon word");
    }

    #[test]
    fn numbers_dates_money_and_addresses_need_the_ai() {
        declines("See you at 3.", "number");
        declines("I'll be there in five minutes.", "number, date or money");
        declines("It costs twenty pounds.", "number, date or money");
        declines("Meet me at three o'clock.", "number, date or money");
        declines("See you in march.", "number, date or money");
        declines("See you in may.", "date");
        declines("It's half done.", "number, date or money");
        declines("Email me at home dot com.", "spoken formatting");
        declines("Email me at david at example dot com.", "uncommon word");
        declines("Costs £20.", "number");
        declines("Costs a few £.", "symbol or non-English letter");
        declines("It's 50% off.", "number");
        declines("See you on the fourth.", "number, date or money");
    }

    #[test]
    fn spoken_formatting_lists_and_mode_switches_need_the_ai() {
        declines("Hi new line how are you.", "spoken formatting");
        declines("Hello comma how are you.", "spoken formatting");
        declines("Hello full stop.", "spoken formatting");
        declines("As a list, eggs and milk.", "spoken mode switch");
        declines("Put it as a list.", "mode name");
        declines("Make it a list.", "spoken mode switch");
        declines("Here's the list.", "list or email structure");
        declines("Can you do it in list mode?", "mode name");
        declines("Write this as an email.", "spoken mode switch");
        declines("Can you write this as an email for me?", "mode name");
        declines("Casual message mode: see you later.", "symbol or non-English letter");
        declines("Casual style, see you later.", "spoken mode switch");
        declines("Dear Sir.", "list or email structure");
        declines("Kind regards.", "list or email structure");
    }

    #[test]
    fn snippet_triggers_need_the_ai() {
        declines("Send them my address.", "snippet trigger");
    }

    #[test]
    fn punctuation_the_ai_would_decide_goes_to_the_ai() {
        declines("thanks see you soon", "no punctuation");
        declines("Yes please.", "needs a comma after the first word");
        declines("Okay I'll call you.", "needs a comma after the first word");
        declines("Hey how are you?", "needs a comma after the first word");
        declines("Thanks see you soon.", "needs a comma after the first word");
        declines("Great see you then.", "needs a comma after the first word");
        declines("See you later everyone.", "needs a comma before the last word");
        declines("Send it please.", "needs a comma before the last word");
        declines("Can you call me.", "may be a question");
        declines("can you go", "may be a question");
        declines("Have you finished.", "may be a question");
        declines("I'm at the store, do you need anything?", "question after a comma");
        declines("What time is it.", "may be a question");
        declines("Thanks so much", "may be an exclamation");
        declines("Happy birthday", "may be an exclamation");
        declines("Really?!", "repeated punctuation");
        declines("Wait...", "repeated punctuation");
        declines("Okay,", "ends with a comma");
        declines("Okay. Sounds good.", "one-word sentence");
        declines("Sounds good, see you then\nBye.", "line breaks");
    }

    #[test]
    fn grammar_slips_slang_and_odd_casing_need_the_ai() {
        declines("I should of gone.", "grammar slip");
        declines("Your welcome.", "grammar slip");
        declines("It's a apple.", "grammar slip");
        declines("Its late.", "slang or spoken form");
        declines("I'm gonna go.", "slang or spoken form");
        declines("Ok see you.", "slang or spoken form");
        declines("I love the Internet.", "proper noun");
        declines("Thank god it's over.", "proper noun");
        declines("I'll Call You.", "capitalised word mid-sentence");
        declines("I'm HOME.", "unusual capitals");
        declines("Send it to b.", "single letter");
        declines("Just wanted to say I'm home.", "padding");
        declines("It's a well-known place.", "symbol or non-English letter");
    }

    #[test]
    fn settings_and_apps_where_cleanup_does_more() {
        let raw = "Sounds good, see you tomorrow.";
        assert!(quick_clean(raw, &chat()).is_some());
        assert!(quick_clean(raw, &ctx_for("auto", None)).is_some());
        assert!(quick_clean(raw, &ctx_for("auto", Some(AppCategory::Other))).is_some());
        assert!(quick_clean(raw, &ctx_for("casual", Some(AppCategory::WorkChat))).is_some());
        for id in ["email", "ai_prompt", "list", "professional"] {
            assert_eq!(quick_check(raw, &ctx_for(id, None)).unwrap_err().rule, "mode", "{id}");
        }
        for cat in [AppCategory::Email, AppCategory::AiAssistant, AppCategory::Code] {
            assert_eq!(quick_check(raw, &ctx_for("auto", Some(cat))).unwrap_err().rule, "app", "{cat:?}");
        }
        // Medium may reword; None never calls the AI anyway.
        for level in [CleanupLevel::Medium, CleanupLevel::None] {
            let mut c = chat();
            c.level = level;
            assert_eq!(quick_check(raw, &c).unwrap_err().rule, "cleanup level");
        }
        // Edited built-in instructions or a custom mode: the AI might do anything.
        let mut c = chat();
        c.mode.instructions = "Always end with an emoji.".into();
        assert_eq!(quick_check(raw, &c).unwrap_err().rule, "mode");
        let mut c = chat();
        c.mode.builtin = false;
        c.mode.id = "custom_family".into();
        assert_eq!(quick_check(raw, &c).unwrap_err().rule, "mode");
    }

    #[test]
    fn context_before_the_cursor() {
        let raw = "Sounds good.";
        // Empty field, or after a finished sentence / new line: fine.
        for before in [None, Some(""), Some("Are you coming tonight?"), Some("Done. "), Some("Hi,\n")] {
            assert!(quick_clean(raw, &chat().with_text_before(before)).is_some(), "{before:?}");
        }
        // Continuing a sentence: the AI decides on capitals and joining words.
        for before in ["I think it", "Well,", "and then"] {
            assert_eq!(quick_check(raw, &chat().with_text_before(Some(before))).unwrap_err().rule, "continues a sentence");
        }
    }

    #[test]
    fn length_limit() {
        let twelve = "I think we can leave the car at the station for now.";
        assert_eq!(crate::text::word_count(twelve), 12);
        assert!(quick_clean(twelve, &doc()).is_some());
        let thirteen = "I think we can leave the car at the station for now then.";
        assert_eq!(quick_check(thirteen, &doc()).unwrap_err().rule, "too long");
        assert!(quick_clean(thirteen, &doc().with_max_words(20)).is_some());
        assert_eq!(quick_check("", &doc()).unwrap_err().rule, "empty");
        assert_eq!(quick_check("   ", &doc()).unwrap_err().rule, "empty");
    }

    #[test]
    fn every_starter_term_and_sound_alike_is_declined() {
        let c = doc();
        for t in crate::hebrew::STARTER {
            for p in std::iter::once(&t.word).chain(t.variants.iter()).chain(t.sounds_like.iter()) {
                let raw = format!("I said {p} today.");
                assert!(quick_clean(&raw, &c).is_none(), "{raw:?} was accepted");
            }
        }
    }

    #[test]
    fn empty_user_dictionary_still_knows_the_starter_list() {
        let c = QuickContext::new(&[], &[], &builtin_modes(), &mode("casual"), None, CleanupLevel::Light);
        assert!(quick_clean("It's a big zeus.", &c).is_none());
        assert!(quick_clean("Gut Shabbos.", &c).is_none());
        assert!(quick_clean("Sounds good.", &c).is_some());
    }
}

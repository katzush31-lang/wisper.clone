//! The personal dictionary: preferred spellings, automatic corrections, hints for the speech
//! model and the cleanup model, and suggestions for words that keep coming out inconsistently.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntrySource {
    /// Pre-loaded Hebrew/Yiddish vocabulary.
    Builtin,
    /// Added by the user.
    User,
    /// Suggested by Wisper and approved by the user.
    Learned,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DictEntry {
    pub id: i64,
    /// The spelling to use, exactly as written (casing and punctuation preserved).
    pub word: String,
    /// Other spellings that are always replaced by `word`.
    pub replace: Vec<String>,
    /// Sound-alikes given to the AI as hints only.
    pub sounds_like: Vec<String>,
    pub source: EntrySource,
    pub starred: bool,
    /// Send to the speech model as a keyword hint.
    pub keyword: bool,
    /// How often the word appeared in finished dictations (for ranking hints).
    pub uses: i64,
}

impl DictEntry {
    pub fn user(word: &str) -> Self {
        Self {
            id: 0,
            word: word.trim().to_string(),
            replace: vec![],
            sounds_like: vec![],
            source: EntrySource::User,
            starred: false,
            keyword: true,
            uses: 0,
        }
    }
}

/// Builds the starter entries from the built-in Hebrew/Yiddish list.
pub fn starter_entries() -> Vec<DictEntry> {
    crate::hebrew::STARTER
        .iter()
        .map(|t| DictEntry {
            id: 0,
            word: t.word.to_string(),
            replace: t.variants.iter().map(|s| s.to_string()).collect(),
            sounds_like: t.sounds_like.iter().map(|s| s.to_string()).collect(),
            source: EntrySource::Builtin,
            starred: false,
            keyword: t.keyword,
            uses: 0,
        })
        .collect()
}

/// Validates a user-entered word or phrase. Returns a plain-English problem.
pub fn validate_word(word: &str) -> Result<String, String> {
    let w = word.trim();
    if w.is_empty() {
        return Err("Type a word or phrase.".into());
    }
    if w.chars().count() > 60 {
        return Err("Keep it under 60 characters.".into());
    }
    if w.contains(['<', '>', '\n', '\r']) {
        return Err("Words can't contain < > or line breaks.".into());
    }
    Ok(w.to_string())
}

pub(crate) fn norm_apostrophes(s: &str) -> String {
    s.replace(['\u{2019}', '\u{2018}', '`', '\u{02bc}'], "'")
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\''
}

/// Case-insensitive, whole-word (whole-phrase) search for `needle` in `hay`, treating runs of
/// whitespace as equal. Returns byte ranges in `hay`.
pub(crate) fn find_all(hay: &str, needle: &str) -> Vec<(usize, usize)> {
    let needle_words: Vec<String> = needle.split_whitespace().map(|w| w.to_lowercase()).collect();
    if needle_words.is_empty() {
        return vec![];
    }
    // Tokenise hay into words with their byte positions; punctuation other than apostrophes and
    // quotes ends a word.
    let mut tokens: Vec<(usize, usize, String)> = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in hay.char_indices() {
        let part_of_word = is_word_char(c) || c == '"' || c == '-';
        match (part_of_word, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                tokens.push((s, i, hay[s..i].to_lowercase()));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        tokens.push((s, hay.len(), hay[s..].to_lowercase()));
    }
    let mut out = Vec::new();
    let n = needle_words.len();
    let mut i = 0;
    while i + n <= tokens.len() {
        let matches = (0..n).all(|k| {
            let tok = tokens[i + k].2.trim_matches(|c| c == '"' || c == '-' );
            let tok = tok.trim_end_matches('\'');
            let want = needle_words[k].trim_end_matches('\'');
            // A trailing possessive "'s" still counts as the word for the last token.
            tok == want || (k == n - 1 && tok.strip_suffix("'s").map(|t| t == want).unwrap_or(false))
        });
        // Only allow separators between phrase words to be whitespace.
        let contiguous = (0..n.saturating_sub(1)).all(|k| hay[tokens[i + k].1..tokens[i + k + 1].0].trim().is_empty());
        if matches && contiguous {
            let s = tokens[i].0 + leading_trim(&hay[tokens[i].0..tokens[i].1]);
            let last = &tokens[i + n - 1];
            let word_end = last.0 + hay[last.0..last.1].trim_end_matches(|c| c == '"' || c == '-').len();
            // Keep a possessive "'s" outside the replacement.
            let e = if hay[s..word_end].to_lowercase().ends_with("'s")
                && !needle.to_lowercase().ends_with("'s")
            {
                word_end - 2
            } else {
                word_end
            };
            out.push((s, e));
            i += n;
        } else {
            i += 1;
        }
    }
    out
}

fn leading_trim(tok: &str) -> usize {
    tok.len() - tok.trim_start_matches(|c| c == '"' || c == '-').len()
}

/// Applies automatic corrections (each entry's `replace` spellings → `word`), longest phrases
/// first. Keeps a capital first letter where the original had one at a sentence start.
/// Returns the corrected text and the number of corrections made.
pub fn apply_replacements(text: &str, entries: &[DictEntry]) -> (String, usize) {
    let mut rules: Vec<(&str, &str)> = Vec::new();
    for e in entries {
        for r in &e.replace {
            if !r.trim().is_empty() && !r.trim().eq(e.word.trim()) {
                rules.push((r.as_str(), e.word.as_str()));
            }
        }
        // The word itself with the wrong case (e.g. "hashem" → "Hashem").
        rules.push((e.word.as_str(), e.word.as_str()));
    }
    rules.sort_by_key(|(from, _)| std::cmp::Reverse(from.split_whitespace().count() * 1000 + from.len()));

    let mut s = norm_apostrophes(text);
    let mut fixes = 0;
    // Longest rules run first. Text written by a rule is protected so a shorter rule can't
    // rewrite part of it (e.g. "Kiddush" must not touch "kiddush Hashem").
    let mut protected: Vec<(usize, usize)> = Vec::new();
    for (from, to) in rules {
        let ranges = find_all(&s, &norm_apostrophes(from));
        if ranges.is_empty() {
            continue;
        }
        let mut result = String::with_capacity(s.len());
        let mut last = 0;
        let mut edits: Vec<(usize, usize, usize)> = Vec::new(); // (old_start, old_end, new_len)
        for (a, b) in ranges {
            if protected.iter().any(|&(pa, pb)| a < pb && b > pa) {
                continue;
            }
            let original = &s[a..b];
            let replacement = match_case(original, to, at_sentence_start(&s, a));
            if original != replacement {
                fixes += 1;
            }
            result.push_str(&s[last..a]);
            result.push_str(&replacement);
            edits.push((a, b, replacement.len()));
            last = b;
        }
        result.push_str(&s[last..]);
        // Map old positions to new ones and protect the newly written spans.
        let map = |pos: usize| -> usize {
            let mut delta: isize = 0;
            for &(a, b, n) in &edits {
                if b <= pos {
                    delta += n as isize - (b - a) as isize;
                }
            }
            (pos as isize + delta) as usize
        };
        protected = protected.into_iter().map(|(pa, pb)| (map(pa), map(pb))).collect();
        for &(a, _, n) in &edits {
            let start = map(a);
            protected.push((start, start + n));
        }
        s = result;
    }
    (s, fixes)
}

fn at_sentence_start(s: &str, pos: usize) -> bool {
    let before = s[..pos].trim_end();
    before.is_empty() || before.ends_with(['.', '!', '?', '\n'])
}

fn match_case(original: &str, target: &str, sentence_start: bool) -> String {
    let orig_first_upper = original.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
    let target_first_lower = target.chars().next().map(|c| c.is_lowercase()).unwrap_or(false);
    if sentence_start && orig_first_upper && target_first_lower {
        let mut c = target.chars();
        let first = c.next().unwrap();
        first.to_uppercase().collect::<String>() + c.as_str()
    } else {
        target.to_string()
    }
}

/// Picks keyword hints for the speech model: the user's own words first, then built-in words
/// ranked by how often the user actually says them, then the default keyword set.
pub fn keywords(entries: &[DictEntry], limit: usize) -> Vec<String> {
    let mut picked: Vec<&DictEntry> = Vec::new();
    let mut user: Vec<&DictEntry> = entries
        .iter()
        .filter(|e| e.source != EntrySource::Builtin && e.keyword)
        .collect();
    user.sort_by_key(|e| (std::cmp::Reverse(e.starred), std::cmp::Reverse(e.uses)));
    picked.extend(user);
    let mut builtin_used: Vec<&DictEntry> = entries
        .iter()
        .filter(|e| e.source == EntrySource::Builtin && e.uses > 0)
        .collect();
    builtin_used.sort_by_key(|e| std::cmp::Reverse(e.uses));
    for e in builtin_used {
        if !picked.iter().any(|p| p.id == e.id && p.word == e.word) {
            picked.push(e);
        }
    }
    for e in entries.iter().filter(|e| e.source == EntrySource::Builtin && e.keyword) {
        if !picked.iter().any(|p| p.word == e.word) {
            picked.push(e);
        }
    }
    picked
        .into_iter()
        .map(|e| e.word.replace(['<', '>', '\n', '\r'], " ").trim().to_string())
        .filter(|w| !w.is_empty())
        .take(limit)
        .collect()
}

/// Dictionary listing for the cleanup model's instructions.
pub fn prompt_listing(entries: &[DictEntry]) -> String {
    let mut lines = Vec::new();
    for e in entries {
        let mut line = format!("- {}", e.word);
        let mut alts: Vec<&str> = e.sounds_like.iter().map(|s| s.as_str()).collect();
        alts.extend(e.replace.iter().take(3).map(|s| s.as_str()));
        if !alts.is_empty() {
            line.push_str(&format!(" (may come through as: {})", alts.join(", ")));
        }
        lines.push(line);
    }
    lines.join("\n")
}

/// Counts how often each dictionary word appears in a finished text (for ranking hints).
pub fn count_uses(text: &str, entries: &[DictEntry]) -> Vec<(i64, i64)> {
    let norm = norm_apostrophes(text);
    entries
        .iter()
        .filter_map(|e| {
            let n = find_all(&norm, &norm_apostrophes(&e.word)).len() as i64;
            (n > 0).then_some((e.id, n))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    /// The spelling we propose (the most frequent one).
    pub word: String,
    /// Other spellings seen for the same word.
    pub variants: Vec<String>,
    pub occurrences: usize,
}

/// Finds names/terms that keep appearing with different spellings across recent dictations,
/// e.g. "Menachem" / "Menahem" / "Menacham". Only uncommon words are considered.
pub fn find_suggestions(texts: &[String], entries: &[DictEntry]) -> Vec<Suggestion> {
    let known: Vec<String> = entries
        .iter()
        .flat_map(|e| std::iter::once(e.word.to_lowercase()).chain(e.replace.iter().map(|r| r.to_lowercase())))
        .collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for text in texts {
        let norm = norm_apostrophes(text);
        for raw in norm.split(|c: char| !is_word_char(c)) {
            let w = raw.trim_matches('\'');
            if w.chars().count() < 4 || w.chars().any(|c| c.is_ascii_digit()) {
                continue;
            }
            if crate::common_words::is_common(&w.to_lowercase()) {
                continue;
            }
            if known.contains(&w.to_lowercase()) {
                continue;
            }
            *counts.entry(w.to_string()).or_default() += 1;
        }
    }
    let mut words: Vec<(String, usize)> = counts.into_iter().collect();
    words.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut used = vec![false; words.len()];
    let mut out = Vec::new();
    for i in 0..words.len() {
        if used[i] {
            continue;
        }
        let mut group = vec![i];
        for j in (i + 1)..words.len() {
            if used[j] {
                continue;
            }
            let (a, b) = (words[i].0.to_lowercase(), words[j].0.to_lowercase());
            let similar = a != b
                && a.chars().next() == b.chars().next()
                && strsim::jaro_winkler(&a, &b) >= 0.88
                && strsim::levenshtein(&a, &b) <= 2;
            if similar {
                group.push(j);
            }
        }
        if group.len() >= 2 {
            let total: usize = group.iter().map(|&k| words[k].1).sum();
            if total >= 3 {
                for &k in &group {
                    used[k] = true;
                }
                out.push(Suggestion {
                    word: words[group[0]].0.clone(),
                    variants: group[1..].iter().map(|&k| words[k].0.clone()).collect(),
                    occurrences: total,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<DictEntry> {
        let mut v = starter_entries();
        for (i, e) in v.iter_mut().enumerate() {
            e.id = i as i64 + 1;
        }
        v
    }

    #[test]
    fn replaces_alternative_spellings() {
        let e = entries();
        let (s, n) = apply_replacements("We need parnassa and zchus, beezras hashem.", &e);
        assert_eq!(s, "We need parnasa and zechus, b'ezras Hashem.");
        assert_eq!(n, 3);
    }

    #[test]
    fn fixes_case_and_curly_apostrophes() {
        let e = entries();
        let (s, _) = apply_replacements("thank hashem, b\u{2019}ezras HaShem it works", &e);
        assert_eq!(s, "thank Hashem, b'ezras Hashem it works");
    }

    #[test]
    fn capitalises_at_sentence_start() {
        let e = entries();
        let (s, _) = apply_replacements("Zchus to be here. Parnassa is hard.", &e);
        assert_eq!(s, "Zechus to be here. Parnasa is hard.");
    }

    #[test]
    fn never_replaces_sound_alikes_or_partial_words() {
        let e = entries();
        let input = "Zeus is a Greek god. I went to school from home. Shabbatai and Shabbatical.";
        let (s, n) = apply_replacements(input, &e);
        assert_eq!(s, input);
        assert_eq!(n, 0);
    }

    #[test]
    fn handles_possessives_and_sephardi_forms() {
        let e = entries();
        let (s, _) = apply_replacements("Shabbat's meal and Sukkot", &e);
        assert_eq!(s, "Shabbos's meal and Sukkos");
    }

    #[test]
    fn longest_phrase_wins() {
        let e = entries();
        let (s, _) = apply_replacements("boruch HaShem we're fine", &e);
        assert_eq!(s, "baruch Hashem we're fine");
    }

    #[test]
    fn user_words_and_keywords() {
        let mut e = entries();
        let mut mine = DictEntry::user("Menachem Mendel");
        mine.id = 9999;
        e.push(mine);
        let k = keywords(&e, 10);
        assert_eq!(k[0], "Menachem Mendel");
        assert_eq!(k.len(), 10);
        assert!(keywords(&e, 500).iter().any(|w| w == "zechus"));
    }

    #[test]
    fn prompt_listing_includes_sound_alikes() {
        let e = entries();
        let p = prompt_listing(&e);
        assert!(p.contains("- zechus (may come through as: Zeus"));
    }

    #[test]
    fn validates_words() {
        assert!(validate_word("  ").is_err());
        assert!(validate_word("a<b").is_err());
        assert_eq!(validate_word(" Chaya Sura ").unwrap(), "Chaya Sura");
    }

    #[test]
    fn suggests_inconsistent_spellings() {
        let texts = vec![
            "Menachem called today about the deal.".to_string(),
            "I told Menahem we would meet.".to_string(),
            "Menachem agreed.".to_string(),
            "The weather is nice today and everything is good.".to_string(),
        ];
        let s = find_suggestions(&texts, &entries());
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].word, "Menachem");
        assert_eq!(s[0].variants, vec!["Menahem".to_string()]);
    }

    #[test]
    fn starter_rules_never_rewrite_inside_another_canonical_phrase() {
        let e = entries();
        for phrase in e.iter().filter(|x| x.word.contains(' ')) {
            let (out, _) = apply_replacements(&phrase.word, &e);
            assert_eq!(out, phrase.word, "rules changed canonical phrase {:?}", phrase.word);
        }
        for single in &e {
            let (out, _) = apply_replacements(&single.word, &e);
            assert_eq!(out, single.word, "rules changed canonical word {:?}", single.word);
        }
    }

    #[test]
    fn counts_uses() {
        let e = entries();
        let uses = count_uses("zechus zechus parnasa", &e);
        let z = e.iter().find(|x| x.word == "zechus").unwrap().id;
        assert!(uses.contains(&(z, 2)));
    }
}

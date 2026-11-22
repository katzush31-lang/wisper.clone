//! A list of very common English words, used to ignore ordinary words when looking for
//! names/terms that are transcribed inconsistently.

const WORDS: &str = include_str!("common_words.txt");

pub fn is_common(lower: &str) -> bool {
    use std::collections::HashSet;
    use std::sync::OnceLock;
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    let set = SET.get_or_init(|| WORDS.split_whitespace().collect());
    if set.contains(lower) {
        return true;
    }
    // Simple inflections of common words.
    for suffix in ["s", "es", "ed", "d", "ing", "ly", "er", "est", "'s", "n't", "'ll", "'re", "'ve", "'d"] {
        if let Some(stem) = lower.strip_suffix(suffix) {
            if stem.len() >= 3 && (set.contains(stem) || set.contains(format!("{stem}e").as_str())) {
                return true;
            }
        }
    }
    false
}

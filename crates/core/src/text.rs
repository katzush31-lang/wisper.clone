//! Text utilities: word counts, insertion spacing/casing, cleaning up model output.

/// Counts words (tokens containing at least one letter or digit).
pub fn word_count(s: &str) -> usize {
    s.split_whitespace()
        .filter(|w| w.chars().any(|c| c.is_alphanumeric()))
        .count()
}

/// Words that are safe to lowercase when continuing mid-sentence.
const LOWERCASABLE: &[&str] = &[
    "a", "about", "after", "again", "all", "also", "an", "and", "any", "are", "as", "at", "be",
    "because", "been", "before", "but", "by", "can", "could", "did", "do", "does", "for", "from",
    "had", "has", "have", "he", "her", "here", "him", "his", "how", "if", "in", "into", "is", "it",
    "its", "just", "let", "like", "may", "maybe", "me", "might", "more", "most", "my", "no", "not",
    "now", "of", "on", "once", "only", "or", "our", "out", "over", "please", "she", "should", "since",
    "so", "some", "such", "than", "thank", "thanks", "that", "the", "their", "them", "then", "there",
    "these", "they", "this", "those", "though", "to", "too", "under", "until", "up", "us", "very",
    "was", "we", "were", "what", "when", "where", "which", "while", "who", "why", "will", "with",
    "would", "yes", "yet", "you", "your", "okay", "ok", "well", "really", "actually", "which",
];

/// Adjusts text for where it lands: adds a leading space after a word, lowercases a common
/// first word when continuing a sentence, and drops a trailing period when the dictation
/// continues into existing text.
pub fn fit_to_context(text: &str, before: Option<&str>, after: Option<&str>) -> String {
    let mut out = text.trim().to_string();
    if out.is_empty() {
        return out;
    }
    if let Some(before) = before {
        let last = before.chars().last();
        let mid_sentence = before
            .trim_end()
            .chars()
            .last()
            .map(|c| c.is_alphanumeric() || c == ',' || c == ';' || c == ':' || c == '-')
            .unwrap_or(false);
        if mid_sentence {
            out = lowercase_first_word_if_common(&out);
        }
        let starts_with_punct = out.starts_with([',', '.', '!', '?', ';', ':', ')']);
        if let Some(c) = last {
            if !c.is_whitespace() && c != '(' && c != '"' && c != '\u{201c}' && !starts_with_punct {
                out.insert(0, ' ');
            }
        }
    }
    if let Some(after) = after {
        if let Some(first) = after.chars().next() {
            if first.is_alphanumeric() {
                if first.is_lowercase() && out.ends_with('.') && !out.ends_with("..") {
                    out.pop();
                }
                out.push(' ');
            }
        }
    }
    out
}

fn lowercase_first_word_if_common(s: &str) -> String {
    let first_word: String = s.chars().take_while(|c| c.is_alphabetic() || *c == '\'').collect();
    if first_word.len() < 1 {
        return s.to_string();
    }
    let lower = first_word.to_lowercase();
    let is_capitalised = first_word.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
        && first_word.chars().skip(1).all(|c| !c.is_uppercase());
    if is_capitalised && LOWERCASABLE.contains(&lower.as_str()) {
        let mut out = lower;
        out.push_str(&s[first_word.len()..]);
        out
    } else {
        s.to_string()
    }
}

/// Removes wrappers a model sometimes adds despite instructions: surrounding tags, a
/// "Here is the cleaned text:" preamble, or quotes around the whole answer.
pub fn sanitize_model_output(output: &str, input: &str) -> String {
    let mut s = output.trim().to_string();
    for tag in ["cleaned_text", "text", "output", "result", "transcript", "rewritten_text"] {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        if s.starts_with(&open) && s.ends_with(&close) {
            s = s[open.len()..s.len() - close.len()].trim().to_string();
        }
    }
    if let Some(first_line_end) = s.find('\n') {
        let first = s[..first_line_end].trim();
        let lower = first.to_lowercase();
        let looks_like_preamble = first.ends_with(':')
            && first.len() < 80
            && (lower.starts_with("here is")
                || lower.starts_with("here's")
                || lower.starts_with("sure")
                || lower.starts_with("cleaned")
                || lower.starts_with("the cleaned")
                || lower.starts_with("formatted"));
        if looks_like_preamble {
            s = s[first_line_end..].trim().to_string();
        }
    }
    let input_trim = input.trim();
    let wrapped = |o: &str, a: char, b: char| o.len() >= 2 && o.starts_with(a) && o.ends_with(b);
    if (wrapped(&s, '"', '"') || wrapped(&s, '\u{201c}', '\u{201d}'))
        && !(input_trim.starts_with('"') || input_trim.starts_with('\u{201c}'))
        && s.matches(['"', '\u{201c}', '\u{201d}']).count() == 2
    {
        let mut chars = s.chars();
        chars.next();
        chars.next_back();
        s = chars.as_str().trim().to_string();
    }
    s
}

/// Deterministic spoken formatting, used when AI cleanup is off.
pub fn apply_spoken_line_breaks(s: &str) -> String {
    let re_para = regex::Regex::new(r"(?i)[,.]?\s*\b(?:new paragraph|next paragraph)\b[,.]?\s*").unwrap();
    let re_line = regex::Regex::new(r"(?i)[,.]?\s*\b(?:new line|next line|line break)\b[,.]?\s*").unwrap();
    let s = re_para.replace_all(s, "\n\n");
    let s = re_line.replace_all(&s, "\n");
    s.trim().to_string()
}

/// Capitalises the first letter (used when cleanup is off).
pub fn capitalise_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Splits long text into chunks of roughly `max_words`, breaking at paragraph or sentence ends.
pub fn split_for_cleanup(text: &str, max_words: usize) -> Vec<String> {
    if word_count(text) <= max_words {
        return vec![text.to_string()];
    }
    let mut sentences: Vec<String> = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        cur.push(c);
        let end_sentence = matches!(c, '.' | '!' | '?' | '\n')
            && chars.get(i + 1).map(|n| n.is_whitespace()).unwrap_or(true);
        if end_sentence {
            sentences.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        sentences.push(cur);
    }
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    let mut words = 0;
    for s in sentences {
        let w = word_count(&s);
        if words + w > max_words && words > 0 {
            chunks.push(chunk.trim().to_string());
            chunk.clear();
            words = 0;
        }
        // A single run-on "sentence" longer than the limit is split by words.
        if w > max_words {
            let ws: Vec<&str> = s.split_whitespace().collect();
            for part in ws.chunks(max_words) {
                chunks.push(part.join(" "));
            }
            continue;
        }
        chunk.push_str(&s);
        words += w;
    }
    if !chunk.trim().is_empty() {
        chunks.push(chunk.trim().to_string());
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words() {
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count("Hello, world!"), 2);
        assert_eq!(word_count("b'ezras Hashem - be'H"), 3);
        assert_eq!(word_count("one\ntwo\n\n- three"), 3);
    }

    #[test]
    fn continuing_a_sentence() {
        assert_eq!(fit_to_context("The meeting is at 3.", Some("I think"), None), " the meeting is at 3.");
        // Proper names stay capitalised.
        assert_eq!(fit_to_context("David will come.", Some("I think"), None), " David will come.");
        // New sentence: unchanged apart from spacing.
        assert_eq!(fit_to_context("The end.", Some("Done."), None), " The end.");
        // After a newline or empty field: no space.
        assert_eq!(fit_to_context("Hi.", Some("Hello\n"), None), "Hi.");
        assert_eq!(fit_to_context("Hi.", None, None), "Hi.");
        // Continuing into following lowercase text drops the period and adds a space.
        assert_eq!(fit_to_context("Really good.", None, Some("and more")), "Really good ");
        // Punctuation-first output doesn't get a space.
        assert_eq!(fit_to_context(", thanks", Some("Yes"), None), ", thanks");
    }

    #[test]
    fn sanitizes_wrappers() {
        assert_eq!(sanitize_model_output("<text>Hello</text>", "hello"), "Hello");
        assert_eq!(sanitize_model_output("Here is the cleaned text:\nHello there.", "hello there"), "Hello there.");
        assert_eq!(sanitize_model_output("\"Hello there.\"", "hello there"), "Hello there.");
        // Quotes the user dictated are kept.
        assert_eq!(sanitize_model_output("\"Hi,\" she said. \"Bye.\"", "hi she said bye"), "\"Hi,\" she said. \"Bye.\"");
        // A real first line ending in a colon is kept.
        assert_eq!(sanitize_model_output("Agenda:\n- One", "agenda one"), "Agenda:\n- One");
    }

    #[test]
    fn spoken_line_breaks() {
        assert_eq!(apply_spoken_line_breaks("Hi David, new paragraph thanks for coming"), "Hi David\n\nthanks for coming");
        assert_eq!(apply_spoken_line_breaks("one new line two"), "one\ntwo");
    }

    #[test]
    fn splits_long_text_at_sentences() {
        let sentence = "This is a sentence with exactly ten words in it. ";
        let text = sentence.repeat(50);
        let chunks = split_for_cleanup(&text, 100);
        assert_eq!(chunks.len(), 5);
        assert!(chunks.iter().all(|c| word_count(c) <= 100));
        assert!(chunks.iter().all(|c| c.ends_with('.')));
        let rejoined: usize = chunks.iter().map(|c| word_count(c)).sum();
        assert_eq!(rejoined, 500);
        let runon = "word ".repeat(250);
        let chunks = split_for_cleanup(&runon, 100);
        assert_eq!(chunks.iter().map(|c| word_count(c)).sum::<usize>(), 250);
    }
}

//! Snippets: say a trigger phrase, get saved text. Triggers are swapped for placeholders
//! before AI cleanup so the saved text comes out exactly as written (addresses, links,
//! multi-line blocks), then restored afterwards.

use serde::{Deserialize, Serialize};

use crate::dictionary::{find_all, norm_apostrophes};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    pub id: i64,
    pub trigger: String,
    pub expansion: String,
    pub uses: i64,
}

pub fn validate(trigger: &str, expansion: &str) -> Result<(String, String), String> {
    let t = trigger.trim();
    if t.is_empty() {
        return Err("Type the phrase you'll say.".into());
    }
    if t.chars().count() > 60 {
        return Err("Keep the trigger phrase under 60 characters.".into());
    }
    if expansion.trim().is_empty() {
        return Err("Type the text the phrase should insert.".into());
    }
    if expansion.chars().count() > 8000 {
        return Err("Keep the saved text under 8,000 characters.".into());
    }
    Ok((t.to_string(), expansion.to_string()))
}

/// A placeholder token the cleanup model is told to keep untouched.
pub fn placeholder(n: usize) -> String {
    format!("\u{27E6}{n}\u{27E7}")
}

/// Replaces spoken triggers with placeholders. Returns the new text and (placeholder,
/// expansion, snippet id) for each replacement, in order.
pub fn insert_placeholders(text: &str, snippets: &[Snippet]) -> (String, Vec<(String, String, i64)>) {
    let mut order: Vec<&Snippet> = snippets.iter().filter(|s| !s.trigger.trim().is_empty()).collect();
    order.sort_by_key(|s| std::cmp::Reverse(s.trigger.len()));
    let mut s = norm_apostrophes(text);
    let mut map = Vec::new();
    for snip in order {
        loop {
            let ranges = find_all(&s, &norm_apostrophes(&snip.trigger));
            let Some(&(a, b)) = ranges.first() else { break };
            let ph = placeholder(map.len() + 1);
            // Swallow a trailing comma/period the recogniser put after a lone trigger.
            let mut end = b;
            let rest = &s[b..];
            if let Some(c) = rest.chars().next() {
                if (c == '.' || c == ',') && rest[1..].trim().is_empty() {
                    end = b + 1;
                }
            }
            s = format!("{}{}{}", &s[..a], ph, &s[end..]);
            map.push((ph, snip.expansion.clone(), snip.id));
        }
    }
    (s, map)
}

/// Puts the saved text back. Any placeholder the model dropped is appended at the end so
/// nothing is lost. Returns the text and whether every placeholder was found in place.
pub fn restore_placeholders(text: &str, map: &[(String, String, i64)]) -> (String, bool) {
    let mut s = text.to_string();
    let mut all_found = true;
    for (ph, expansion, _) in map {
        if s.contains(ph.as_str()) {
            s = s.replacen(ph.as_str(), expansion, 1);
            // Remove duplicates the model may have produced.
            s = s.replace(ph.as_str(), "");
        } else {
            all_found = false;
            if !s.is_empty() && !s.ends_with(char::is_whitespace) {
                s.push(' ');
            }
            s.push_str(expansion);
        }
    }
    (s, all_found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snips() -> Vec<Snippet> {
        vec![
            Snippet { id: 1, trigger: "my email address".into(), expansion: "me@example.com".into(), uses: 0 },
            Snippet { id: 2, trigger: "my address".into(), expansion: "1 High St\nLondon".into(), uses: 0 },
        ]
    }

    #[test]
    fn replaces_longest_trigger_first_and_restores() {
        let (t, map) = insert_placeholders("Send it to my email address, thanks", &snips());
        assert_eq!(t, "Send it to \u{27E6}1\u{27E7}, thanks");
        assert_eq!(map.len(), 1);
        let (out, ok) = restore_placeholders(&t, &map);
        assert!(ok);
        assert_eq!(out, "Send it to me@example.com, thanks");
    }

    #[test]
    fn case_insensitive_and_multiple() {
        let (t, map) = insert_placeholders("My Address. And my address again.", &snips());
        assert_eq!(map.len(), 2);
        let (out, _) = restore_placeholders(&t, &map);
        assert_eq!(out, "1 High St\nLondon. And 1 High St\nLondon again.");
    }

    #[test]
    fn lone_trigger_drops_trailing_period() {
        let (t, _) = insert_placeholders("My email address.", &snips());
        assert_eq!(t, "\u{27E6}1\u{27E7}");
    }

    #[test]
    fn dropped_placeholder_is_appended_not_lost() {
        let map = vec![(placeholder(1), "me@example.com".to_string(), 1)];
        let (out, ok) = restore_placeholders("Reach me any time.", &map);
        assert!(!ok);
        assert_eq!(out, "Reach me any time. me@example.com");
    }

    #[test]
    fn validation() {
        assert!(validate("", "x").is_err());
        assert!(validate("x", " ").is_err());
        assert!(validate("my email", "a@b.c").is_ok());
    }
}

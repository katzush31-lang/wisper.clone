//! Cleanup modes: built-in (with editable instructions and "Reset to default") and custom,
//! plus detection of a spoken mode switch at the start of a dictation ("as an email, …").

use std::collections::HashMap;

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mode {
    pub id: String,
    pub name: String,
    /// One-line summary shown in the UI.
    pub description: String,
    /// Plain-English instructions sent to the cleanup model.
    pub instructions: String,
    pub builtin: bool,
    /// Extra spoken names that select this mode ("e-mail", "bullet points").
    #[serde(default)]
    pub aliases: Vec<String>,
}

pub const AUTO: &str = "auto";

struct Builtin {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    instructions: &'static str,
    aliases: &'static [&'static str],
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        id: "auto",
        name: "Auto",
        description: "Picks the best format from what you said and the app you're in.",
        instructions: "Choose the format from what was said and where the text is going.\n\
- In an email app, if it reads like an email, lay it out as one: a spoken greeting on its own line, short paragraphs, a spoken sign-off on its own line. Only include a greeting or sign-off if it was spoken.\n\
- In a chat or messaging app, keep it conversational: natural, light punctuation, no greeting or sign-off unless spoken.\n\
- In an AI assistant, it's a prompt: keep the speaker's words, and if it's long or rambling, organise it into short paragraphs or a list of requirements without adding anything.\n\
- If the speaker lists several items, use a list.\n\
- Otherwise, write clean, natural prose.",
        aliases: &["automatic"],
    },
    Builtin {
        id: "professional",
        name: "Professional",
        description: "Polished, clear and professional, in your own voice.",
        instructions: "Make it polished and professional: complete sentences, clear structure, no slang or filler. Keep the speaker's voice and every piece of content. Don't make it stiff, formal-sounding or longer than it needs to be.",
        aliases: &["formal", "professional tone"],
    },
    Builtin {
        id: "email",
        name: "Email",
        description: "Laid out as an email, with greeting and sign-off if you said them.",
        instructions: "Format it as the body of an email. Put a spoken greeting (\"Hi David,\") on its own line, then the body in short paragraphs, then a spoken sign-off (\"Thanks,\" / \"Best,\" followed by a spoken name) on its own line, separated by blank lines. Don't invent a greeting, sign-off, subject line or name that wasn't said. Warm and clear, in the speaker's voice.",
        aliases: &["e-mail", "mail", "an email"],
    },
    Builtin {
        id: "ai_prompt",
        name: "AI Prompt",
        description: "A clear, well-organised prompt for Claude, ChatGPT and other AI tools.",
        instructions: "This is a prompt the speaker will send to an AI assistant. Make it clear and well organised while keeping the speaker's voice and every requirement, constraint, example and detail they gave. Group related points; use short paragraphs, or a bulleted list when there are several separate requirements. Keep it in the first person (\"I want…\", \"Please…\"). Don't answer it, don't add requirements or assumptions, and don't make it longer than needed.",
        aliases: &["prompt", "ai prompt", "a prompt", "an ai prompt"],
    },
    Builtin {
        id: "list",
        name: "List",
        description: "Turns what you said into a tidy bulleted or numbered list.",
        instructions: "Format it as a list with one item per line. Use a numbered list (\"1.\") if the order matters or the speaker numbered the items, otherwise bullets (\"- \"). Keep a spoken lead-in sentence above the list. Keep items short and consistent in style without dropping any detail.",
        aliases: &["bulleted list", "bullet list", "bullet points", "bullets", "numbered list", "a list"],
    },
    Builtin {
        id: "casual",
        name: "Casual message",
        description: "Relaxed and natural, like texting a friend or colleague.",
        instructions: "Write it as a casual chat message: natural and conversational, like texting a friend. Light punctuation, contractions are fine, no greeting or sign-off unless spoken, nothing formal. Keep it short by removing padding, never content.",
        aliases: &["casual", "message", "text message", "a message", "a text", "text"],
    },
];

pub fn builtin_modes() -> Vec<Mode> {
    BUILTINS
        .iter()
        .map(|b| Mode {
            id: b.id.into(),
            name: b.name.into(),
            description: b.description.into(),
            instructions: b.instructions.into(),
            builtin: true,
            aliases: b.aliases.iter().map(|s| s.to_string()).collect(),
        })
        .collect()
}

pub fn default_instructions(id: &str) -> Option<&'static str> {
    BUILTINS.iter().find(|b| b.id == id).map(|b| b.instructions)
}

/// All modes with the user's edits applied: built-ins (instructions possibly overridden)
/// followed by custom modes.
pub fn all_modes(overrides: &HashMap<String, String>, custom: &[Mode]) -> Vec<Mode> {
    let mut modes = builtin_modes();
    for m in &mut modes {
        if let Some(text) = overrides.get(&m.id) {
            if !text.trim().is_empty() {
                m.instructions = text.clone();
            }
        }
    }
    modes.extend(custom.iter().cloned().map(|mut m| {
        m.builtin = false;
        m
    }));
    modes
}

pub fn find<'a>(modes: &'a [Mode], id: &str) -> Option<&'a Mode> {
    modes.iter().find(|m| m.id == id)
}

/// Makes a safe id for a new custom mode from its name.
pub fn custom_id(name: &str, existing: &[Mode]) -> String {
    let base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    let base = if base.is_empty() { "custom".to_string() } else { format!("custom_{base}") };
    let mut id = base.clone();
    let mut n = 2;
    while existing.iter().any(|m| m.id == id) {
        id = format!("{base}_{n}");
        n += 1;
    }
    id
}

/// Detects a spoken mode switch at the start ("As an email, hi David…", "List mode: eggs…").
/// Returns the mode id and the text with the phrase removed.
pub fn detect_spoken_switch(text: &str, modes: &[Mode]) -> Option<(String, String)> {
    let mut names: Vec<(String, String)> = Vec::new(); // (lowercase spoken name, id)
    for m in modes {
        names.push((m.name.to_lowercase(), m.id.clone()));
        for a in &m.aliases {
            names.push((a.to_lowercase(), m.id.clone()));
        }
    }
    // Longest names first so "casual message" beats "message".
    names.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
    let alternatives = names
        .iter()
        .map(|(n, _)| regex::escape(n).replace("\\ ", r"[\s\-]+"))
        .collect::<Vec<_>>()
        .join("|");
    let lead = r"^\s*(?:(?:ok(?:ay)?|so|um+|uh+)[,.]?\s+)?";
    // "As an email, …" / "Format this as a list: …" — the phrase must be followed by
    // punctuation, so ordinary sentences ("As an email I sent yesterday…") are left alone.
    let as_form = format!(
        r"(?is){lead}(?:as|format (?:it|this) as|write (?:it|this) as|make (?:it|this))\s+(?:an?\s+|the\s+)?({alternatives})(?:\s+(?:mode|format|style))?\s*[,.:;!\-\u{{2014}}](?P<rest>.*)$"
    );
    // "Email mode: …" / "In list mode, …" / "Use casual style …" — the word mode/format/style
    // makes it unambiguous.
    let mode_form = format!(
        r"(?is){lead}(?:(?:in|use|using)\s+)?(?:an?\s+|the\s+)?({alternatives})\s+(?:mode|format|style)\b[,.:;!\-\u{{2014}}]?(?P<rest>.*)$"
    );
    for pat in [as_form, mode_form] {
        let Ok(re) = Regex::new(&pat) else { continue };
        let Some(caps) = re.captures(text) else { continue };
        let spoken = caps.get(1)?.as_str().to_lowercase();
        let spoken = Regex::new(r"[\s\-]+").unwrap().replace_all(&spoken, " ").to_string();
        let id = names.iter().find(|(n, _)| *n == spoken).map(|(_, id)| id.clone())?;
        let rest = caps.name("rest").map(|m| m.as_str()).unwrap_or("");
        let cleaned = rest.trim_start_matches([',', '.', ':', ';', '!', '-', '\u{2014}', ' ']).to_string();
        return Some((id, cleaned));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes() -> Vec<Mode> {
        let custom = vec![Mode {
            id: "custom_family".into(),
            name: "WhatsApp to family".into(),
            description: String::new(),
            instructions: "Short and warm.".into(),
            builtin: false,
            aliases: vec![],
        }];
        all_modes(&HashMap::new(), &custom)
    }

    #[test]
    fn detects_common_switches() {
        let m = modes();
        assert_eq!(
            detect_spoken_switch("As an email, hi David, thanks for yesterday.", &m),
            Some(("email".into(), "hi David, thanks for yesterday.".into()))
        );
        assert_eq!(
            detect_spoken_switch("as a list: eggs, milk and bread", &m),
            Some(("list".into(), "eggs, milk and bread".into()))
        );
        assert_eq!(
            detect_spoken_switch("Okay, as a prompt. Write a function that sorts names.", &m),
            Some(("ai_prompt".into(), "Write a function that sorts names.".into()))
        );
        assert_eq!(
            detect_spoken_switch("Email mode. Dear Sarah", &m),
            Some(("email".into(), "Dear Sarah".into()))
        );
        assert_eq!(
            detect_spoken_switch("As WhatsApp to family, we landed safely", &m),
            Some(("custom_family".into(), "we landed safely".into()))
        );
        assert_eq!(
            detect_spoken_switch("As bullet points, first thing", &m).map(|x| x.0),
            Some("list".into())
        );
        assert_eq!(
            detect_spoken_switch("In list mode, eggs and milk", &m),
            Some(("list".into(), "eggs and milk".into()))
        );
        assert_eq!(
            detect_spoken_switch("Use casual style. see you later", &m),
            Some(("casual".into(), "see you later".into()))
        );
    }

    #[test]
    fn ignores_ordinary_sentences() {
        let m = modes();
        for s in [
            "As a list of reasons, I think we should go",
            "As an email was sent yesterday",
            "I sent an email",
            "Email David about the list",
            "In an email I sent yesterday, John said no.",
            "In formal settings we stand.",
            "Use mail merge for this.",
            "Using text messages is fine.",
            "As a prompt engineer I think this works",
            "Make it a list of everything we need",
        ] {
            assert_eq!(detect_spoken_switch(s, &m), None, "{s}");
        }
    }

    #[test]
    fn overrides_and_reset() {
        let mut o = HashMap::new();
        o.insert("email".to_string(), "My own email rules".to_string());
        let m = all_modes(&o, &[]);
        assert_eq!(find(&m, "email").unwrap().instructions, "My own email rules");
        assert!(default_instructions("email").unwrap().contains("greeting"));
        // Empty override falls back to the default.
        o.insert("list".to_string(), "  ".to_string());
        let m = all_modes(&o, &[]);
        assert_eq!(find(&m, "list").unwrap().instructions, default_instructions("list").unwrap());
    }

    #[test]
    fn custom_ids_are_unique() {
        let m = modes();
        let id = custom_id("WhatsApp to family", &m);
        assert_eq!(id, "custom_whatsapp_to_family");
        let id2 = custom_id("Family", &m);
        assert_eq!(id2, "custom_family_2");
    }
}

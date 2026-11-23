//! Curated model lists with one-line notes, used for the settings dropdowns. The live model
//! list from each provider is merged in when a key is available.

use super::ModelInfo;

pub const DEFAULT_TRANSCRIPTION: &str = "gpt-transcribe";
pub const DEFAULT_ANTHROPIC: &str = "claude-sonnet-5-5";
pub const DEFAULT_OPENAI_CLEANUP: &str = "gpt-6.2-sol";
pub const DEFAULT_FAST_ANTHROPIC: &str = "claude-haiku-5-5";
pub const DEFAULT_FAST_OPENAI: &str = "gpt-6-luna";

fn m(id: &str, name: &str, note: &str, recommended: bool) -> ModelInfo {
    ModelInfo { id: id.into(), name: name.into(), note: note.into(), recommended }
}

pub fn transcription_models() -> Vec<ModelInfo> {
    vec![
        m("gpt-transcribe", "GPT-Transcribe", "Most accurate, recommended ($0.0045/min)", true),
        m("gpt-4o-transcribe", "GPT-4o Transcribe", "Older; retired by OpenAI in Feb 2027", false),
        m("gpt-4o-mini-transcribe", "GPT-4o mini Transcribe", "Older and cheaper, less accurate; retired Feb 2027", false),
        m("whisper-1", "Whisper", "Oldest model; least accurate with names", false),
    ]
}

pub fn anthropic_models() -> Vec<ModelInfo> {
    vec![
        m("claude-sonnet-5-5", "Claude Sonnet 5.5", "Best balance of speed and quality (recommended)", true),
        m("claude-haiku-5-5", "Claude Haiku 5.5", "Fastest and cheapest; good for short everyday dictation", false),
        m("claude-opus-5-5", "Claude Opus 5.5", "Best quality, slower and about 2× the cost", false),
    ]
}

pub fn openai_cleanup_models() -> Vec<ModelInfo> {
    vec![
        m("gpt-6.2-sol", "GPT-6.2 Sol", "OpenAI's balanced everyday model", true),
        m("gpt-6-luna", "GPT-6 Luna", "Fastest and cheapest OpenAI option", false),
        m("gpt-6.1-terra", "GPT-6.1 Terra", "Fast, with clear natural writing", false),
        m("gpt-6.2-astra", "GPT-6.2 Astra", "Most capable, slowest and most expensive", false),
    ]
}

/// Merges live model ids into a curated list: curated entries first (only those the account
/// has, if the live list is non-empty), then any other live ids that look relevant.
pub fn merge_live(curated: Vec<ModelInfo>, live: &[String], relevant: impl Fn(&str) -> bool) -> Vec<ModelInfo> {
    if live.is_empty() {
        return curated;
    }
    let mut out: Vec<ModelInfo> = curated.into_iter().filter(|c| live.iter().any(|l| l == &c.id)).collect();
    for id in live {
        if relevant(id) && !out.iter().any(|o| &o.id == id) {
            out.push(ModelInfo { id: id.clone(), name: id.clone(), note: String::new(), recommended: false });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_in_lists() {
        assert!(transcription_models().iter().any(|m| m.id == DEFAULT_TRANSCRIPTION && m.recommended));
        assert!(anthropic_models().iter().any(|m| m.id == DEFAULT_ANTHROPIC && m.recommended));
        assert!(openai_cleanup_models().iter().any(|m| m.id == DEFAULT_OPENAI_CLEANUP));
        assert!(anthropic_models().iter().any(|m| m.id == DEFAULT_FAST_ANTHROPIC));
        assert!(openai_cleanup_models().iter().any(|m| m.id == DEFAULT_FAST_OPENAI));
    }

    #[test]
    fn merge_keeps_curated_order_and_adds_new() {
        let live = vec!["claude-opus-5-5".to_string(), "claude-sonnet-5-5".into(), "claude-sonnet-6".into()];
        let merged = merge_live(anthropic_models(), &live, |id| id.starts_with("claude-"));
        let ids: Vec<_> = merged.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["claude-sonnet-5-5", "claude-opus-5-5", "claude-sonnet-6"]);
        assert_eq!(merge_live(anthropic_models(), &[], |_| true).len(), 3);
    }
}

//! "AI instructions": shows exactly what the cleanup AI is sent. Nothing is written out
//! again here; every block comes from the same functions the pipeline uses.

use serde::{Deserialize, Serialize};

use crate::apps::{AppCategory, AppInfo};
use crate::dictionary::DictEntry;
use crate::modes::{self, Mode};
use crate::pipeline::{natural_mode, cleanup_context_for, cleanup_system_blocks, command_system_blocks, CaptureContext};
use crate::prompts::{self, CleanupLevel};
use crate::settings::Settings;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRequest {
    pub mode_id: String,
    pub app_category: Option<AppCategory>,
    pub cleanup_level: Option<CleanupLevel>,
}

/// The made-up dictation the example message is built from.
#[derive(Debug, Clone)]
pub struct PreviewSample {
    pub transcript: String,
    pub ctx: CaptureContext,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppTypeInfo {
    pub category: AppCategory,
    pub label: String,
    /// The sentence added to the message for this kind of app.
    pub sentence: String,
    /// The style Auto picks for this kind of app (when no per-app style is set).
    pub auto_mode_id: String,
    pub auto_mode_name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TransformPreview {
    pub name: String,
    pub instructions: String,
    pub user_message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PromptPreview {
    pub mode_id: String,
    pub mode_name: String,
    /// True when the chosen style was Auto and the app type made Wisper use another style;
    /// `mode_id` / `mode_name` then name the style actually sent.
    pub auto_picked: bool,
    /// The style that was chosen in the picker.
    pub chosen_mode_name: String,
    /// False when the cleanup level is "no AI edits": then none of this is sent.
    pub ai_used: bool,
    pub level: CleanupLevel,
    pub level_text: String,
    /// System block 1: the shared rules and the dictionary.
    pub base_system: String,
    /// System block 2: the chosen mode and the cleanup level.
    pub mode_system: String,
    /// The per-dictation message: context, then the transcript.
    pub user_message: String,
    pub app_types: Vec<AppTypeInfo>,
    pub command_system: String,
    pub command_user: String,
    pub transforms: Vec<TransformPreview>,
}

/// An example app for each kind, so the message shows the real wording.
fn example_app(cat: AppCategory) -> (&'static str, &'static str, &'static str) {
    // (name, exe, window title)
    match cat {
        AppCategory::Email => ("Gmail", "chrome.exe", "Inbox - Gmail"),
        AppCategory::WorkChat => ("Slack", "slack.exe", "general - Slack"),
        AppCategory::PersonalChat => ("WhatsApp", "whatsapp.exe", "WhatsApp"),
        AppCategory::AiAssistant => ("Claude", "claude.exe", "Claude"),
        AppCategory::Document => ("Word", "winword.exe", "Document1 - Word"),
        AppCategory::Code => ("VS Code", "code.exe", "main.rs - VS Code"),
        AppCategory::Other => ("Notepad", "notepad.exe", "Untitled - Notepad"),
    }
}

pub const SAMPLE_TRANSCRIPT: &str = "um so I wanted to check if Thursday still works for the meeting, actually no, Friday morning is better for me";
const SAMPLE_BEFORE_CURSOR: &str = "Hi David,";
const SAMPLE_SELECTION: &str = "Their is a meeting on thursday, please come.";
const SAMPLE_INSTRUCTION: &str = "make it friendlier";

/// A sample dictation for a kind of app (None = no particular app).
pub fn sample_for(cat: Option<AppCategory>) -> PreviewSample {
    let app = match cat {
        Some(c) => {
            let (name, exe, title) = example_app(c);
            AppInfo { key: exe.into(), name: name.into(), exe: exe.into(), title: title.into(), category: Some(c) }
        }
        None => AppInfo::default(),
    };
    PreviewSample {
        transcript: SAMPLE_TRANSCRIPT.into(),
        ctx: CaptureContext { app, text_before: Some(SAMPLE_BEFORE_CURSOR.into()), text_after: None, selection: None },
    }
}

/// Builds the preview from the same functions `Engine::llm_cleanup` and `run_instruction` use.
pub fn build(settings: &Settings, modes_list: &[Mode], entries: &[DictEntry], req: &PreviewRequest, sample: &PreviewSample) -> Option<PromptPreview> {
    let chosen = modes::find(modes_list, &req.mode_id)?;
    // Same rule as the pipeline's choose_mode: with Auto as the default style, the kind of
    // app picks its natural style.
    let swapped = (chosen.id == modes::AUTO && settings.default_mode == modes::AUTO)
        .then(|| natural_mode(modes_list, sample.ctx.app.category))
        .flatten()
        .filter(|m| m.id != chosen.id);
    let mode = swapped.unwrap_or(chosen);
    let level = req.cleanup_level.unwrap_or(settings.cleanup_level);
    let excluded = settings.context_excluded_apps.iter().any(|a| a.eq_ignore_ascii_case(&sample.ctx.app.exe));
    let use_ctx = settings.context_awareness && !excluded;
    let before = if use_ctx { sample.ctx.text_before.clone() } else { None };

    let system = cleanup_system_blocks(entries, mode, level);
    let user_message = prompts::cleanup_user_message(&sample.transcript, &cleanup_context_for(&sample.ctx, before, use_ctx));

    let app_types = AppCategory::ALL
        .iter()
        .map(|&c| {
            let auto = modes::find(modes_list, c.default_mode());
            let (name, _, _) = example_app(c);
            AppTypeInfo {
                category: c,
                label: c.label().to_string(),
                sentence: format!("The text will be inserted into {name}, {}.", c.describe()),
                auto_mode_id: c.default_mode().to_string(),
                auto_mode_name: auto.map(|m| m.name.clone()).unwrap_or_default(),
            }
        })
        .collect();

    let cmd_ctx = cleanup_context_for(&sample.ctx, None, false);
    let command_system = command_system_blocks(entries).into_iter().map(|b| b.text).collect::<Vec<_>>().join("\n");
    let command_user = prompts::command_user_message(Some(SAMPLE_SELECTION), SAMPLE_INSTRUCTION, &cmd_ctx);
    let transforms = settings
        .transforms
        .iter()
        .map(|t| TransformPreview {
            name: t.name.clone(),
            instructions: t.instructions.clone(),
            user_message: prompts::command_user_message(Some(SAMPLE_SELECTION), &t.instructions, &cmd_ctx),
        })
        .collect();

    Some(PromptPreview {
        mode_id: mode.id.clone(),
        mode_name: mode.name.clone(),
        auto_picked: swapped.is_some(),
        chosen_mode_name: chosen.name.clone(),
        ai_used: level != CleanupLevel::None,
        level,
        level_text: prompts::level_text(level).to_string(),
        base_system: system[0].text.clone(),
        mode_system: system[1].text.clone(),
        user_message,
        app_types,
        command_system,
        command_user,
        transforms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modes::builtin_modes;

    fn req(mode: &str, cat: Option<AppCategory>) -> PreviewRequest {
        PreviewRequest { mode_id: mode.into(), app_category: cat, cleanup_level: None }
    }

    #[test]
    fn unknown_mode_gives_nothing() {
        let s = Settings::default();
        assert!(build(&s, &builtin_modes(), &[], &req("nope", None), &sample_for(None)).is_none());
    }

    #[test]
    fn app_type_changes_only_the_message_context() {
        let s = Settings::default();
        let m = builtin_modes();
        let none = build(&s, &m, &[], &req("auto", None), &sample_for(None)).unwrap();
        let email = build(&s, &m, &[], &req("auto", Some(AppCategory::Email)), &sample_for(Some(AppCategory::Email))).unwrap();
        assert_eq!(none.base_system, email.base_system);
        // Auto in an email app is replaced by the Email style (as the pipeline does).
        assert_ne!(none.mode_system, email.mode_system);
        assert!(!none.user_message.contains("will be inserted into"));
        assert!(email.user_message.contains("Gmail, an email app"));
        let email_row = email.app_types.iter().find(|a| a.category == AppCategory::Email).unwrap();
        assert_eq!(email_row.auto_mode_name, "Email");
        // Auto in an email app sends the Email style, and says so.
        assert!(email.auto_picked);
        assert_eq!(email.mode_id, "email");
        assert!(email.mode_system.contains("Format for this dictation: Email"));
        assert!(!none.auto_picked);
    }

    #[test]
    fn edited_and_custom_modes_and_level_are_applied() {
        let s = Settings::default();
        let mut over = std::collections::HashMap::new();
        over.insert("casual".to_string(), "Always write in capital letters.".to_string());
        let custom = Mode { id: "custom_x".into(), name: "Shouty".into(), description: String::new(), instructions: "Use exclamation marks.".into(), builtin: false, aliases: vec![] };
        let m = modes::all_modes(&over, &[custom]);
        let p = build(&s, &m, &[], &PreviewRequest { mode_id: "casual".into(), app_category: None, cleanup_level: Some(CleanupLevel::Medium) }, &sample_for(None)).unwrap();
        assert!(p.mode_system.contains("Always write in capital letters."));
        assert!(p.mode_system.contains("reword sentences"));
        let c = build(&s, &m, &[], &req("custom_x", None), &sample_for(None)).unwrap();
        assert!(c.mode_system.contains("Format for this dictation: Shouty"));
        let none = build(&s, &m, &[], &PreviewRequest { mode_id: "auto".into(), app_category: None, cleanup_level: Some(CleanupLevel::None) }, &sample_for(None)).unwrap();
        assert!(!none.ai_used);
    }

    #[test]
    fn context_off_hides_window_title_and_text_before_cursor() {
        let mut s = Settings::default();
        s.context_awareness = false;
        let p = build(&s, &builtin_modes(), &[], &req("auto", Some(AppCategory::Email)), &sample_for(Some(AppCategory::Email))).unwrap();
        assert!(!p.user_message.contains("Window title"));
        assert!(!p.user_message.contains("Hi David,"));
    }

    #[test]
    fn transforms_and_command_prompt_are_included() {
        let s = Settings::default();
        let p = build(&s, &builtin_modes(), &[], &req("auto", None), &sample_for(None)).unwrap();
        assert_eq!(p.transforms.len(), s.transforms.len());
        assert!(p.command_system.contains("voice-command step"));
        assert!(p.transforms[0].user_message.contains(&p.transforms[0].instructions));
    }
}

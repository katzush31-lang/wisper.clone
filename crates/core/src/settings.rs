//! User settings, stored as JSON in the app data folder. Every field has a default so old
//! settings files keep working when new options are added. API keys are NOT stored here
//! (they live in Windows Credential Manager).

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::hotkey::HotkeyConfig;
use crate::modes::Mode;
use crate::prompts::CleanupLevel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum OtherAudio {
    /// Leave other audio alone.
    Off,
    /// Pause music/videos that are playing, resume afterwards.
    #[default]
    Pause,
    /// Mute apps that are playing sound, unmute afterwards.
    Mute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum CleanupProvider {
    #[default]
    Anthropic,
    OpenAi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum BarPosition {
    #[default]
    Bottom,
    Top,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SoundSettings {
    pub enabled: bool,
    /// 0.0 – 1.0
    pub volume: f32,
}

impl Default for SoundSettings {
    fn default() -> Self {
        Self { enabled: true, volume: 0.5 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotificationSettings {
    pub dictation_failed: bool,
    pub waiting_offline: bool,
    pub api_key_problem: bool,
    pub dictionary_suggestion: bool,
    pub update_ready: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            dictation_failed: true,
            waiting_offline: true,
            api_key_problem: true,
            dictionary_suggestion: true,
            update_ready: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderSettings {
    pub transcription_model: String,
    pub cleanup_provider: CleanupProvider,
    pub anthropic_model: String,
    pub openai_cleanup_model: String,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self {
            transcription_model: crate::providers::models::DEFAULT_TRANSCRIPTION.into(),
            cleanup_provider: CleanupProvider::Anthropic,
            anthropic_model: crate::providers::models::DEFAULT_ANTHROPIC.into(),
            openai_cleanup_model: crate::providers::models::DEFAULT_OPENAI_CLEANUP.into(),
        }
    }
}

/// A per-app style: dictations into this app use this mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStyle {
    pub app_key: String,
    pub app_name: String,
    pub mode_id: String,
    /// True if the user set it; false if Wisper learned it from their choices.
    pub user_set: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transform {
    pub id: String,
    pub name: String,
    pub instructions: String,
    #[serde(default)]
    pub hotkey: Option<crate::hotkey::KeyCombo>,
    #[serde(default)]
    pub builtin: bool,
}

pub fn default_transforms() -> Vec<Transform> {
    vec![
        Transform {
            id: "polish".into(),
            name: "Polish".into(),
            instructions: "Polish this text: fix grammar and punctuation, make it clearer and more concise, and improve the flow, while keeping the meaning, every fact and the writer's tone.".into(),
            hotkey: Some(crate::hotkey::KeyCombo::new(&[crate::hotkey::vk::WIN, crate::hotkey::vk::ALT, 0x31])),
            builtin: true,
        },
        Transform {
            id: "prompt_engineer".into(),
            name: "Prompt Engineer".into(),
            instructions: "Rewrite this as a clear, well-structured prompt for an AI assistant: state the goal first, then context, requirements and constraints as short paragraphs or bullets. Keep every requirement and detail; don't add new ones.".into(),
            hotkey: Some(crate::hotkey::KeyCombo::new(&[crate::hotkey::vk::WIN, crate::hotkey::vk::ALT, 0x32])),
            builtin: true,
        },
        Transform {
            id: "to_list".into(),
            name: "Turn into a list".into(),
            instructions: "Turn this into a tidy bulleted list, one point per line, keeping every detail.".into(),
            hotkey: None,
            builtin: true,
        },
        Transform {
            id: "shorter".into(),
            name: "Make it shorter".into(),
            instructions: "Make this noticeably shorter while keeping all key facts, requests and the tone.".into(),
            hotkey: None,
            builtin: true,
        },
    ]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub version: u32,
    /// Shown in the greeting ("Welcome back, …").
    pub user_name: String,
    pub onboarded: bool,
    pub hotkeys: HotkeyConfig,
    /// Master switch: when false, Wisper ignores all its keyboard/mouse shortcuts.
    pub shortcuts_enabled: bool,
    /// Microphone id; None = Windows default.
    pub microphone: Option<String>,
    /// Keep the microphone open in the background so the very first syllable is captured.
    pub keep_mic_warm: bool,
    /// Spoken languages: ["en"] by default; empty = auto-detect.
    pub languages: Vec<String>,
    pub theme: Theme,
    pub launch_at_login: bool,
    pub always_show_bar: bool,
    pub bar_position: BarPosition,
    pub sounds: SoundSettings,
    pub other_audio: OtherAudio,
    pub notifications: NotificationSettings,
    pub default_mode: String,
    pub cleanup_level: CleanupLevel,
    /// Send the text around the cursor and the window title to the AI as context.
    pub context_awareness: bool,
    /// Apps where nothing on screen is ever sent as context.
    pub context_excluded_apps: Vec<String>,
    pub app_styles: Vec<AppStyle>,
    /// Learn per-app styles from the modes you pick.
    pub learn_app_styles: bool,
    /// Edited instructions for built-in modes (id → text).
    pub mode_overrides: HashMap<String, String>,
    pub custom_modes: Vec<Mode>,
    pub transforms: Vec<Transform>,
    /// Days to keep audio recordings; None = forever.
    pub keep_audio_days: Option<u32>,
    /// Days to keep transcripts; None = forever.
    pub keep_history_days: Option<u32>,
    pub providers: ProviderSettings,
    /// Typing speed used to estimate time saved (words per minute).
    pub typing_wpm: u32,
    /// Spoken "new line" etc. handled even when AI cleanup is off.
    pub smart_formatting: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            user_name: String::new(),
            onboarded: false,
            hotkeys: HotkeyConfig::defaults(),
            shortcuts_enabled: true,
            microphone: None,
            keep_mic_warm: false,
            languages: vec!["en".into()],
            theme: Theme::System,
            launch_at_login: true,
            always_show_bar: false,
            bar_position: BarPosition::Bottom,
            sounds: SoundSettings::default(),
            other_audio: OtherAudio::Pause,
            notifications: NotificationSettings::default(),
            default_mode: "auto".into(),
            cleanup_level: CleanupLevel::Light,
            context_awareness: true,
            context_excluded_apps: vec!["1password.exe".into(), "keepass.exe".into(), "bitwarden.exe".into()],
            app_styles: Vec::new(),
            learn_app_styles: true,
            mode_overrides: HashMap::new(),
            custom_modes: Vec::new(),
            transforms: default_transforms(),
            keep_audio_days: Some(30),
            keep_history_days: None,
            providers: ProviderSettings::default(),
            typing_wpm: 40,
            smart_formatting: true,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Settings {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Settings>(&text) {
                Ok(s) => s.normalised(),
                Err(e) => {
                    tracing::warn!("settings file unreadable ({e}); keeping a backup and using defaults");
                    let _ = std::fs::copy(path, path.with_extension("json.bad"));
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        }
    }

    /// Writes atomically (temp file + rename) so a crash can't leave a half-written file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).expect("settings serialise"))?;
        std::fs::rename(&tmp, path)
    }

    /// Fixes values that would break the app (e.g. a hotkey list emptied by hand).
    pub fn normalised(mut self) -> Self {
        let d = HotkeyConfig::defaults();
        if self.hotkeys.dictate.iter().all(|c| c.is_empty()) {
            self.hotkeys.dictate = d.dictate;
        }
        self.sounds.volume = self.sounds.volume.clamp(0.0, 1.0);
        if self.typing_wpm < 5 {
            self.typing_wpm = 40;
        }
        if self.default_mode.trim().is_empty() {
            self.default_mode = "auto".into();
        }
        if self.transforms.is_empty() {
            self.transforms = default_transforms();
        }
        self
    }

    /// The mode to use for an app: user rule, then learned rule, then the default mode.
    pub fn mode_for_app(&self, app_key: &str) -> Option<&AppStyle> {
        self.app_styles
            .iter()
            .filter(|s| s.app_key == app_key)
            .max_by_key(|s| s.user_set)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        let mut s = Settings::default();
        s.user_name = "Test".into();
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p), s);
        // An old/partial file still loads with defaults for missing fields.
        std::fs::write(&p, r#"{"userName":"X","sounds":{"volume":3.0}}"#).unwrap();
        let s = Settings::load(&p);
        assert_eq!(s.user_name, "X");
        assert_eq!(s.sounds.volume, 1.0);
        assert!(s.sounds.enabled);
        assert_eq!(s.default_mode, "auto");
        assert!(!s.hotkeys.dictate.is_empty());
    }

    #[test]
    fn shortcuts_enabled_defaults_on_and_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        // Old files without the field load as enabled.
        std::fs::write(&p, r#"{"userName":"X"}"#).unwrap();
        assert!(Settings::load(&p).shortcuts_enabled);
        let mut s = Settings::default();
        assert!(s.shortcuts_enabled);
        s.shortcuts_enabled = false;
        s.save(&p).unwrap();
        assert!(!Settings::load(&p).shortcuts_enabled);
    }

    #[test]
    fn corrupt_file_falls_back_and_keeps_backup() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(&p, "{not json").unwrap();
        let s = Settings::load(&p);
        assert_eq!(s, Settings::default());
        assert!(dir.path().join("settings.json.bad").exists());
    }

    #[test]
    fn user_set_app_style_beats_learned() {
        let mut s = Settings::default();
        s.app_styles.push(AppStyle { app_key: "slack.exe".into(), app_name: "Slack".into(), mode_id: "casual".into(), user_set: false });
        s.app_styles.push(AppStyle { app_key: "slack.exe".into(), app_name: "Slack".into(), mode_id: "professional".into(), user_set: true });
        assert_eq!(s.mode_for_app("slack.exe").unwrap().mode_id, "professional");
        assert!(s.mode_for_app("x.exe").is_none());
    }
}

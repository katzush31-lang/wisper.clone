//! User settings, stored as JSON in the app data folder. Every field has a default so old
//! settings files keep working when new options are added. API keys are NOT stored here
//! (they live in Windows Credential Manager).

use std::collections::{BTreeMap, HashMap};
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

/// Where API keys are kept once signed in to a Wisper account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum KeyStorage {
    /// Only on this computer, never uploaded.
    Local,
    /// Encrypted in the user's account and shared with every computer they sign in on.
    #[default]
    Cloud,
}

impl KeyStorage {
    pub fn as_str(self) -> &'static str {
        match self {
            KeyStorage::Local => "local",
            KeyStorage::Cloud => "cloud",
        }
    }
    pub fn parse(s: &str) -> Option<KeyStorage> {
        match s {
            "local" => Some(KeyStorage::Local),
            "cloud" => Some(KeyStorage::Cloud),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountSettings {
    pub key_storage: KeyStorage,
    /// Master switch for syncing while signed in.
    pub sync_enabled: bool,
}

impl Default for AccountSettings {
    fn default() -> Self {
        Self { key_storage: KeyStorage::Cloud, sync_enabled: true }
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

/// Settings sections that sync between computers (JSON field names). Anything not listed
/// here (microphone, keepMicWarm, launchAtLogin, onboarded, version, sectionTimes and the whole
/// `account` block, including where API keys are kept) never leaves this computer.
pub const SYNCED_SECTIONS: &[&str] = &[
    "userName", "hotkeys", "shortcutsEnabled", "languages", "theme", "alwaysShowBar", "barPosition", "sounds",
    "otherAudio", "notifications", "defaultMode", "cleanupLevel", "contextAwareness", "contextExcludedApps",
    "appStyles", "learnAppStyles", "modeOverrides", "customModes", "transforms", "keepAudioDays",
    "keepHistoryDays", "providers", "typingWpm", "smartFormatting",
];

/// A section value received from the account, with the time it was last changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteSection {
    pub value: serde_json::Value,
    pub updated_at: i64,
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
    /// Optional Wisper account preferences.
    pub account: AccountSettings,
    /// When each synced section last changed on this computer (unix ms), used to decide which
    /// side is newer when syncing. Managed by the app, not by the UI.
    pub section_times: HashMap<String, i64>,
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
            account: AccountSettings::default(),
            section_times: HashMap::new(),
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

    /// The synced sections as JSON values keyed by their camelCase field name.
    pub fn synced_sections(&self) -> BTreeMap<String, serde_json::Value> {
        let root = serde_json::to_value(self).expect("settings serialise");
        let mut out = BTreeMap::new();
        for name in SYNCED_SECTIONS {
            if let Some(v) = root.get(*name) {
                out.insert((*name).to_string(), v.clone());
            }
        }
        out
    }

    /// Applies sections received from the account. Only the listed synced sections can change;
    /// every never-synced field is left exactly as it was. A section whose value doesn't fit
    /// (e.g. from a newer app version) is skipped. Returns the names whose value changed.
    pub fn apply_remote_sections(&mut self, remote: &BTreeMap<String, RemoteSection>) -> Vec<String> {
        let mut changed = Vec::new();
        for (name, sec) in remote {
            if !SYNCED_SECTIONS.contains(&name.as_str()) {
                continue;
            }
            let mut root = serde_json::to_value(&*self).expect("settings serialise");
            root[name.as_str()] = sec.value.clone();
            let Ok(candidate) = serde_json::from_value::<Settings>(root) else { continue };
            let candidate = candidate.normalised();
            if candidate != *self {
                *self = candidate;
                changed.push(name.clone());
            }
            self.section_times.insert(name.clone(), sec.updated_at);
        }
        changed
    }

    /// True if the keyboard shortcuts (the main ones or any transform's) differ from `other`'s.
    /// Used to avoid resetting the shortcut engine (and a held key) when only other settings changed.
    pub fn shortcuts_differ(&self, other: &Settings) -> bool {
        self.hotkeys != other.hotkeys
            || self.transforms.len() != other.transforms.len()
            || self.transforms.iter().zip(&other.transforms).any(|(a, b)| a.hotkey != b.hotkey)
    }

    /// Records `now` as the change time of every synced section whose value differs from `old`.
    pub fn stamp_changed_sections(&mut self, old: &Settings, now: i64) {
        let before = old.synced_sections();
        for (name, value) in self.synced_sections() {
            if before.get(&name) != Some(&value) {
                self.section_times.insert(name, now);
            }
        }
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
    fn old_settings_files_load_with_account_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(&p, r#"{"version":1,"userName":"Old","onboarded":true,"theme":"dark"}"#).unwrap();
        let s = Settings::load(&p);
        assert_eq!(s.user_name, "Old");
        assert!(s.onboarded);
        assert_eq!(s.account.key_storage, KeyStorage::Cloud);
        assert!(s.account.sync_enabled);
        assert!(s.section_times.is_empty());
        // And the new fields round-trip with their documented names.
        let mut s2 = Settings::default();
        s2.account.key_storage = KeyStorage::Local;
        s2.section_times.insert("userName".into(), 5);
        let json = serde_json::to_value(&s2).unwrap();
        assert_eq!(json["account"]["keyStorage"], "local");
        assert_eq!(json["account"]["syncEnabled"], true);
        assert_eq!(json["sectionTimes"]["userName"], 5);
    }

    #[test]
    fn synced_sections_cover_the_documented_list_only() {
        let s = Settings::default();
        let sections = s.synced_sections();
        assert_eq!(sections.len(), SYNCED_SECTIONS.len());
        for never in ["microphone", "keepMicWarm", "launchAtLogin", "onboarded", "version", "sectionTimes", "account"] {
            assert!(!sections.contains_key(never), "{never} must not sync");
        }
        assert!(!sections.contains_key("account.keyStorage"), "key storage is per computer");
        assert_eq!(sections["theme"], "system");
    }

    #[test]
    fn applying_remote_sections_leaves_never_synced_fields_alone() {
        let mut s = Settings::default();
        s.microphone = Some("mic-1".into());
        s.keep_mic_warm = true;
        s.launch_at_login = false;
        s.onboarded = true;
        let mut remote = BTreeMap::new();
        remote.insert("userName".to_string(), RemoteSection { value: "Sarah".into(), updated_at: 50 });
        remote.insert("theme".to_string(), RemoteSection { value: "dark".into(), updated_at: 60 });
        // Key storage is per computer: a remote value for it is ignored.
        remote.insert("account.keyStorage".to_string(), RemoteSection { value: "local".into(), updated_at: 70 });
        // Unknown and never-synced names are ignored; bad values are skipped.
        remote.insert("microphone".to_string(), RemoteSection { value: "evil".into(), updated_at: 80 });
        remote.insert("typingWpm".to_string(), RemoteSection { value: "fast".into(), updated_at: 80 });
        let changed = s.apply_remote_sections(&remote);
        assert_eq!(changed, vec!["theme", "userName"]);
        assert_eq!(s.user_name, "Sarah");
        assert_eq!(s.theme, Theme::Dark);
        assert_eq!(s.account.key_storage, KeyStorage::Cloud);
        assert!(!s.section_times.contains_key("account.keyStorage"));
        assert_eq!(s.typing_wpm, 40);
        assert_eq!(s.microphone.as_deref(), Some("mic-1"));
        assert!(s.keep_mic_warm && s.onboarded && !s.launch_at_login);
        assert_eq!(s.section_times["theme"], 60);
        assert!(!s.section_times.contains_key("microphone"));
    }

    #[test]
    fn shortcut_changes_are_detected_but_other_settings_are_not() {
        let a = Settings::default();
        let mut b = a.clone();
        b.user_name = "X".into();
        b.theme = Theme::Dark;
        assert!(!a.shortcuts_differ(&b));
        b.transforms[0].hotkey = None;
        assert!(a.shortcuts_differ(&b));
        let mut c = a.clone();
        c.hotkeys.dictate.clear();
        assert!(a.shortcuts_differ(&c));
    }

    #[test]
    fn changed_sections_get_stamped() {
        let old = Settings::default();
        let mut new = old.clone();
        new.user_name = "X".into();
        new.microphone = Some("m".into()); // never synced → not stamped
        new.sounds.volume = 0.9;
        new.account.key_storage = KeyStorage::Local; // per computer → not stamped
        new.stamp_changed_sections(&old, 1234);
        let mut keys: Vec<_> = new.section_times.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, vec!["sounds", "userName"]);
        assert_eq!(new.section_times["userName"], 1234);
        // A second stamp with no further change keeps the old times.
        let snapshot = new.clone();
        new.stamp_changed_sections(&snapshot, 9999);
        assert_eq!(new.section_times["userName"], 1234);
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

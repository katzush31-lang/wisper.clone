//! Recognises which app (or website) the user is dictating into, for per-app styles.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase")]
pub enum AppCategory {
    Email,
    WorkChat,
    PersonalChat,
    AiAssistant,
    Document,
    Code,
    Other,
}

impl AppCategory {
    pub const ALL: [AppCategory; 7] = [
        AppCategory::Email,
        AppCategory::WorkChat,
        AppCategory::PersonalChat,
        AppCategory::AiAssistant,
        AppCategory::Document,
        AppCategory::Code,
        AppCategory::Other,
    ];

    /// Short plain name for the UI.
    pub fn label(self) -> &'static str {
        match self {
            AppCategory::Email => "Email apps",
            AppCategory::WorkChat => "Work chat (Slack, Teams)",
            AppCategory::PersonalChat => "Personal chat (WhatsApp, Telegram)",
            AppCategory::AiAssistant => "AI assistants (Claude, ChatGPT)",
            AppCategory::Document => "Documents and notes",
            AppCategory::Code => "Code editors and terminals",
            AppCategory::Other => "Other apps",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            AppCategory::Email => "an email app",
            AppCategory::WorkChat => "a work chat app",
            AppCategory::PersonalChat => "a personal messaging app",
            AppCategory::AiAssistant => "an AI assistant (the text is a prompt for an AI)",
            AppCategory::Document => "a document editor",
            AppCategory::Code => "a code editor or terminal",
            AppCategory::Other => "an app",
        }
    }

    /// The built-in mode Auto leans towards for this kind of app.
    pub fn default_mode(self) -> &'static str {
        match self {
            AppCategory::Email => "email",
            AppCategory::WorkChat | AppCategory::PersonalChat => "casual",
            AppCategory::AiAssistant => "ai_prompt",
            AppCategory::Document | AppCategory::Code | AppCategory::Other => "auto",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    /// Stable key used for per-app settings: "web:gmail" or "whatsapp.exe".
    pub key: String,
    /// Friendly name: "Gmail", "WhatsApp".
    pub name: String,
    pub exe: String,
    pub title: String,
    pub category: Option<AppCategory>,
}

const BROWSERS: &[&str] = &[
    "chrome.exe", "msedge.exe", "firefox.exe", "brave.exe", "opera.exe", "opera_gx.exe",
    "vivaldi.exe", "arc.exe", "zen.exe", "librewolf.exe", "waterfox.exe", "comet.exe", "dia.exe",
];

struct Known {
    needle: &'static str,
    key: &'static str,
    name: &'static str,
    cat: AppCategory,
}

const fn k(needle: &'static str, key: &'static str, name: &'static str, cat: AppCategory) -> Known {
    Known { needle, key, name, cat }
}

use AppCategory::*;

/// Desktop apps by executable name.
const EXES: &[Known] = &[
    k("outlook.exe", "outlook.exe", "Outlook", Email),
    k("olk.exe", "outlook.exe", "Outlook", Email),
    k("thunderbird.exe", "thunderbird.exe", "Thunderbird", Email),
    k("hxoutlook.exe", "mail.exe", "Mail", Email),
    k("whatsapp", "whatsapp.exe", "WhatsApp", PersonalChat),
    k("telegram.exe", "telegram.exe", "Telegram", PersonalChat),
    k("signal.exe", "signal.exe", "Signal", PersonalChat),
    k("messenger.exe", "messenger.exe", "Messenger", PersonalChat),
    k("discord.exe", "discord.exe", "Discord", PersonalChat),
    k("slack.exe", "slack.exe", "Slack", WorkChat),
    k("ms-teams.exe", "teams.exe", "Microsoft Teams", WorkChat),
    k("teams.exe", "teams.exe", "Microsoft Teams", WorkChat),
    k("zoom.exe", "zoom.exe", "Zoom", WorkChat),
    k("claude.exe", "claude.exe", "Claude", AiAssistant),
    k("chatgpt.exe", "chatgpt.exe", "ChatGPT", AiAssistant),
    k("perplexity.exe", "perplexity.exe", "Perplexity", AiAssistant),
    k("winword.exe", "winword.exe", "Word", Document),
    k("notepad.exe", "notepad.exe", "Notepad", Document),
    k("onenote.exe", "onenote.exe", "OneNote", Document),
    k("notion.exe", "notion.exe", "Notion", Document),
    k("obsidian.exe", "obsidian.exe", "Obsidian", Document),
    k("powerpnt.exe", "powerpnt.exe", "PowerPoint", Document),
    k("excel.exe", "excel.exe", "Excel", Document),
    k("code.exe", "code.exe", "VS Code", Code),
    k("cursor.exe", "cursor.exe", "Cursor", Code),
    k("windsurf.exe", "windsurf.exe", "Windsurf", Code),
    k("windowsterminal.exe", "windowsterminal.exe", "Terminal", Code),
    k("powershell.exe", "powershell.exe", "PowerShell", Code),
    k("cmd.exe", "cmd.exe", "Command Prompt", Code),
    k("wisper.exe", "wisper.exe", "Wisper", Document),
];

/// Websites by a distinctive part of the browser window title.
const SITES: &[Known] = &[
    k("gmail", "web:gmail", "Gmail", Email),
    k("outlook", "web:outlook", "Outlook", Email),
    k("yahoo mail", "web:yahoomail", "Yahoo Mail", Email),
    k("proton mail", "web:protonmail", "Proton Mail", Email),
    k("whatsapp", "web:whatsapp", "WhatsApp", PersonalChat),
    k("messenger", "web:messenger", "Messenger", PersonalChat),
    k("telegram", "web:telegram", "Telegram", PersonalChat),
    k("discord", "web:discord", "Discord", PersonalChat),
    k("slack", "web:slack", "Slack", WorkChat),
    k("microsoft teams", "web:teams", "Microsoft Teams", WorkChat),
    k("linkedin", "web:linkedin", "LinkedIn", WorkChat),
    k("claude", "web:claude", "Claude", AiAssistant),
    k("chatgpt", "web:chatgpt", "ChatGPT", AiAssistant),
    k("gemini", "web:gemini", "Gemini", AiAssistant),
    k("perplexity", "web:perplexity", "Perplexity", AiAssistant),
    k("copilot", "web:copilot", "Copilot", AiAssistant),
    k("grok", "web:grok", "Grok", AiAssistant),
    k("google docs", "web:googledocs", "Google Docs", Document),
    k("- word", "web:word", "Word", Document),
    k("notion", "web:notion", "Notion", Document),
];

/// Identifies the app from its executable name and window title.
pub fn identify(exe: &str, title: &str) -> AppInfo {
    let exe_l = exe.to_lowercase();
    let exe_l = exe_l.rsplit(['\\', '/']).next().unwrap_or(&exe_l).to_string();
    let title_l = title.to_lowercase();
    if BROWSERS.contains(&exe_l.as_str()) {
        // Browser titles look like "Inbox (3) - me@x.com - Gmail - Google Chrome".
        if let Some(site) = SITES.iter().find(|s| title_l.contains(s.needle)) {
            return AppInfo {
                key: site.key.into(),
                name: site.name.into(),
                exe: exe_l,
                title: title.into(),
                category: Some(site.cat),
            };
        }
        let browser = browser_name(&exe_l);
        return AppInfo { key: exe_l.clone(), name: browser.into(), exe: exe_l, title: title.into(), category: None };
    }
    if let Some(app) = EXES.iter().find(|a| exe_l.contains(a.needle)) {
        return AppInfo {
            key: app.key.into(),
            name: app.name.into(),
            exe: exe_l,
            title: title.into(),
            category: Some(app.cat),
        };
    }
    let name = exe_l.trim_end_matches(".exe");
    let mut pretty = String::new();
    for (i, c) in name.chars().enumerate() {
        if i == 0 {
            pretty.extend(c.to_uppercase());
        } else {
            pretty.push(c);
        }
    }
    AppInfo { key: exe_l.clone(), name: pretty, exe: exe_l, title: title.into(), category: None }
}

fn browser_name(exe: &str) -> &'static str {
    match exe {
        "chrome.exe" => "Chrome",
        "msedge.exe" => "Edge",
        "firefox.exe" => "Firefox",
        "brave.exe" => "Brave",
        "arc.exe" => "Arc",
        _ => "Browser",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_desktop_apps() {
        let a = identify(r"C:\Program Files\WindowsApps\WhatsApp.Root.exe", "WhatsApp");
        assert_eq!(a.key, "whatsapp.exe");
        assert_eq!(a.category, Some(PersonalChat));
        assert_eq!(identify("OUTLOOK.EXE", "Inbox").category, Some(Email));
        assert_eq!(identify("claude.exe", "Claude").category.unwrap().default_mode(), "ai_prompt");
    }

    #[test]
    fn identifies_websites_in_browsers() {
        let a = identify("chrome.exe", "Inbox (3) - someone@example.com - Gmail - Google Chrome");
        assert_eq!(a.key, "web:gmail");
        assert_eq!(a.category, Some(Email));
        assert_eq!(a.category.unwrap().default_mode(), "email");
        let a = identify("msedge.exe", "New chat - Claude - Microsoft Edge");
        assert_eq!(a.key, "web:claude");
        let a = identify("chrome.exe", "BBC News - Google Chrome");
        assert_eq!(a.key, "chrome.exe");
        assert_eq!(a.category, None);
    }

    #[test]
    fn unknown_apps_get_a_tidy_name() {
        let a = identify("myeditor.exe", "x");
        assert_eq!(a.name, "Myeditor");
        assert_eq!(a.category, None);
    }
}

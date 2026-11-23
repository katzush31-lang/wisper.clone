// One place that lists every page and settings section, so the rail, the settings page
// and the "Jump to" search always agree.
import {
  BarChart3,
  Bell,
  BookText,
  Cloud,
  Database,
  FileText,
  Info,
  Keyboard,
  KeyRound,
  Mic,
  Scissors,
  Settings as Cog,
  Type,
  Volume2,
  Wand2,
} from "lucide-react";
import type { ReactNode } from "react";

export type PageId =
  | "home"
  | "insights"
  | "dictionary"
  | "snippets"
  | "style"
  | "transforms"
  | "scratchpad"
  | "settings";

export interface NavPage {
  id: PageId;
  label: string;
  /** what the page is for, shown in Jump to */
  hint: string;
  icon: (size: number) => ReactNode;
  group: "main" | "teach";
  /** extra words people might type to find it */
  keywords?: string;
}

export const PAGES: NavPage[] = [
  { id: "home", label: "Home", hint: "Start here and see what you said", icon: (s) => <Mic size={s} />, group: "main", keywords: "history dictation recent" },
  { id: "scratchpad", label: "Scratchpad", hint: "Dictate into a note inside Wisper", icon: (s) => <FileText size={s} />, group: "main", keywords: "notes draft" },
  { id: "insights", label: "Insights", hint: "Words, speed and streaks", icon: (s) => <BarChart3 size={s} />, group: "main", keywords: "stats usage numbers" },
  { id: "dictionary", label: "Dictionary", hint: "Names and words Wisper should spell right", icon: (s) => <BookText size={s} />, group: "teach", keywords: "words spelling vocabulary replacements" },
  { id: "snippets", label: "Snippets", hint: "Say a short phrase, get a longer text", icon: (s) => <Scissors size={s} />, group: "teach", keywords: "shortcuts expand templates" },
  { id: "style", label: "Style", hint: "How your writing should sound", icon: (s) => <Type size={s} />, group: "teach", keywords: "tone formal casual modes" },
  { id: "transforms", label: "Transforms", hint: "Rewrite selected text with your voice", icon: (s) => <Wand2 size={s} />, group: "teach", keywords: "rewrite command edit" },
];

export const SETTINGS_PAGE: NavPage = {
  id: "settings",
  label: "Settings",
  hint: "Everything you can change",
  icon: (s) => <Cog size={s} />,
  group: "main",
  keywords: "preferences options",
};

export interface SettingsSectionInfo {
  id: string;
  label: string;
  hint: string;
  icon: ReactNode;
  keywords?: string;
}

export const SETTINGS_SECTIONS: SettingsSectionInfo[] = [
  { id: "general", label: "General", hint: "Name, theme, microphone, languages", icon: <Cog size={17} />, keywords: "theme dark light startup login name language microphone mic" },
  { id: "shortcuts", label: "Shortcuts", hint: "Keys that start and stop dictation", icon: <Keyboard size={17} />, keywords: "hotkey keys keyboard" },
  { id: "dictation", label: "Dictation", hint: "How your words get tidied up", icon: <Wand2 size={17} />, keywords: "mode cleanup formatting context excluded apps" },
  { id: "audio", label: "Sounds & audio", hint: "Sounds and the floating bar", icon: <Volume2 size={17} />, keywords: "sound volume mute beep bar position" },
  { id: "notifications", label: "Notifications", hint: "When Wisper tells you things", icon: <Bell size={17} />, keywords: "alerts" },
  { id: "providers", label: "AI providers", hint: "Your OpenAI and Anthropic keys", icon: <KeyRound size={17} />, keywords: "api key openai anthropic claude model" },
  { id: "data", label: "Data & privacy", hint: "What is kept and for how long", icon: <Database size={17} />, keywords: "history delete export privacy" },
  { id: "account", label: "Account", hint: "Sync, backup and your computers", icon: <Cloud size={17} />, keywords: "sign in sync cloud backup restore" },
  { id: "about", label: "About & updates", hint: "Version, updates, release notes", icon: <Info size={17} />, keywords: "version update" },
];

export const validSection = (id?: string) =>
  SETTINGS_SECTIONS.some((s) => s.id === id) ? (id as string) : "general";

/** Ctrl+1 … Ctrl+7 go to pages in this order. */
export const PAGE_ORDER: PageId[] = PAGES.map((p) => p.id);

export const pageLabel = (id: PageId) => (id === "settings" ? "Settings" : PAGES.find((p) => p.id === id)?.label ?? "");

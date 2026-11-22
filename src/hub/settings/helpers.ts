// Pure helpers for the Settings window (kept free of React so they can be unit-tested).
import type { HotkeyConfig, KeyCombo } from "../../shared/api";

export interface Option<T> {
  value: T;
  label: string;
}

export const AUDIO_RETENTION: Option<number | null>[] = [
  { value: 7, label: "7 days" },
  { value: 30, label: "30 days" },
  { value: 90, label: "90 days" },
  { value: 365, label: "1 year" },
  { value: null, label: "Forever" },
];

export const HISTORY_RETENTION: Option<number | null>[] = [
  { value: null, label: "Forever" },
  { value: 30, label: "30 days" },
  { value: 90, label: "90 days" },
  { value: 365, label: "1 year" },
];

/** Dropdown <option> values are strings; "forever" stands for null. */
export function retentionToString(v: number | null): string {
  return v === null ? "forever" : String(v);
}

export function retentionFromString(s: string): number | null {
  if (s === "forever") return null;
  const n = Number(s);
  return Number.isFinite(n) && n > 0 ? n : null;
}

/** If the saved value isn't one of the presets (e.g. hand-edited), keep it visible as an extra option. */
export function withCurrent(options: Option<number | null>[], current: number | null): Option<number | null>[] {
  if (options.some((o) => o.value === current)) return options;
  return [...options, { value: current, label: `${current} days` }];
}

export type LanguageChoice = "en" | "en-he" | "auto";

export function languageChoice(languages: string[]): LanguageChoice {
  if (languages.length === 0) return "auto";
  if (languages.includes("he")) return "en-he";
  return "en";
}

export function languagesFor(choice: LanguageChoice): string[] {
  return choice === "en" ? ["en"] : choice === "en-he" ? ["en", "he"] : [];
}

/** Splits on commas/semicolons/new lines; lowercases, trims and removes duplicates. */
export function parseExcludedApps(text: string): string[] {
  const out: string[] = [];
  for (const part of text.split(/[,;\n]/)) {
    const v = part.trim().toLowerCase();
    if (v && !out.includes(v)) out.push(v);
  }
  return out;
}

export function formatExcludedApps(apps: string[]): string {
  return apps.join(", ");
}

export function clampWpm(n: number): number {
  if (!Number.isFinite(n)) return 40;
  return Math.min(250, Math.max(10, Math.round(n)));
}

export const HOTKEY_ACTIONS: { id: keyof HotkeyConfig; title: string; description: string }[] = [
  {
    id: "dictate",
    title: "Dictate",
    description: "Hold to talk, release to paste your words. Double-tap to keep recording hands-free.",
  },
  { id: "handsFree", title: "Hands-free", description: "Starts recording without holding any keys. Press again to finish." },
  {
    id: "command",
    title: "Command mode",
    description: "Select some text, hold this and say what to do with it, like \"make this shorter\".",
  },
  { id: "pasteLast", title: "Paste last dictation", description: "Types your most recent dictation again at the cursor." },
  { id: "copyLast", title: "Copy last dictation", description: "Copies your most recent dictation so you can paste it yourself." },
  {
    id: "undoCleanup",
    title: "Undo cleanup",
    description: "Swaps the tidied-up text for exactly what you said (press again to swap back).",
  },
  {
    id: "rerunLast",
    title: "Redo last in another mode",
    description: "Runs your last dictation again using a different writing style.",
  },
  { id: "openScratchpad", title: "Open Scratchpad", description: "Opens Wisper's notepad and starts dictating into it." },
];

const k = (...keys: number[]): KeyCombo[] => [{ keys }];
const CTRL = 0x11, WIN = 0x5b, ALT = 0x12, SHIFT = 0x10, SPACE = 0x20;

export const DEFAULT_HOTKEYS: HotkeyConfig = {
  dictate: k(CTRL, WIN),
  handsFree: k(CTRL, WIN, SPACE),
  command: k(CTRL, WIN, ALT),
  pasteLast: k(ALT, SHIFT, 0x5a),
  copyLast: k(ALT, SHIFT, 0x58),
  undoCleanup: k(ALT, SHIFT, 0x55),
  rerunLast: k(ALT, SHIFT, 0x52),
  openScratchpad: k(WIN, ALT, 0x53),
};

export function setCombo(hotkeys: HotkeyConfig, action: keyof HotkeyConfig, keys: number[]): HotkeyConfig {
  return { ...hotkeys, [action]: [{ keys }] };
}

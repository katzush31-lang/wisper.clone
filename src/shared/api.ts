// Typed contract between the UI and the Rust backend (Tauri commands + events).
// Field names are camelCase, matching the Rust serde output.
import { call, listen } from "./tauri";

// ---------- shared types ----------

export interface KeyCombo {
  keys: number[];
}

export interface HotkeyConfig {
  dictate: KeyCombo[];
  handsFree: KeyCombo[];
  command: KeyCombo[];
  pasteLast: KeyCombo[];
  copyLast: KeyCombo[];
  undoCleanup: KeyCombo[];
  rerunLast: KeyCombo[];
  openScratchpad: KeyCombo[];
}

export type Theme = "system" | "light" | "dark";
export type OtherAudio = "off" | "pause" | "mute";
export type CleanupProvider = "anthropic" | "openAi";
export type CleanupLevel = "none" | "light" | "medium";
export type BarPosition = "bottom" | "top";

export interface Mode {
  id: string;
  name: string;
  description: string;
  instructions: string;
  builtin: boolean;
  aliases: string[];
}

export type AppCategory = "email" | "workChat" | "personalChat" | "aiAssistant" | "document" | "code" | "other";

export interface PromptPreview {
  modeId: string;
  modeName: string;
  /** false when cleanup is set to "no AI edits": none of this is sent */
  aiUsed: boolean;
  level: CleanupLevel;
  levelText: string;
  baseSystem: string;
  modeSystem: string;
  userMessage: string;
  appTypes: { category: AppCategory; label: string; sentence: string; autoModeId: string; autoModeName: string }[];
  commandSystem: string;
  commandUser: string;
  transforms: { name: string; instructions: string; userMessage: string }[];
}

export interface AppStyle {
  appKey: string;
  appName: string;
  modeId: string;
  /** true if set by the user; false if learned */
  userSet: boolean;
}

export interface Transform {
  id: string;
  name: string;
  instructions: string;
  hotkey: KeyCombo | null;
  builtin: boolean;
}

export interface FastCleanup {
  enabled: boolean;
  /** "short" means a recording shorter than this many seconds */
  maxSeconds: number;
  anthropicModel: string;
  openaiModel: string;
  /** tidy tiny dictations on this computer when there's nothing for the AI to fix */
  skipAiForTiny?: boolean;
}

export interface Settings {
  version: number;
  userName: string;
  onboarded: boolean;
  hotkeys: HotkeyConfig;
  shortcutsEnabled: boolean;
  microphone: string | null;
  keepMicWarm: boolean;
  /** ISO codes; empty array = auto-detect */
  languages: string[];
  theme: Theme;
  launchAtLogin: boolean;
  alwaysShowBar: boolean;
  barPosition: BarPosition;
  sounds: { enabled: boolean; volume: number };
  otherAudio: OtherAudio;
  notifications: {
    dictationFailed: boolean;
    waitingOffline: boolean;
    apiKeyProblem: boolean;
    dictionarySuggestion: boolean;
    updateReady: boolean;
  };
  defaultMode: string;
  cleanupLevel: CleanupLevel;
  contextAwareness: boolean;
  contextExcludedApps: string[];
  appStyles: AppStyle[];
  learnAppStyles: boolean;
  /** edited instructions for built-in modes: id → text */
  modeOverrides: Record<string, string>;
  customModes: Mode[];
  transforms: Transform[];
  /** null = keep forever */
  keepAudioDays: number | null;
  keepHistoryDays: number | null;
  providers: {
    transcriptionModel: string;
    cleanupProvider: CleanupProvider;
    anthropicModel: string;
    openaiCleanupModel: string;
    /** a quicker model for short dictations */
    fastCleanup: FastCleanup;
    /** transcribe while the user talks (OpenAI Realtime); the recording is still saved */
    liveTranscription: boolean;
  };
  typingWpm: number;
  smartFormatting: boolean;
  /** optional account preferences (absent on older settings files) */
  account?: { keyStorage: KeyStorage; syncEnabled: boolean };
}

export type KeyStorage = "local" | "cloud";

export interface AccountStatus {
  /** false when this build has no cloud address, so accounts are not offered */
  available: boolean;
  signedIn: boolean;
  email?: string;
  name?: string;
  deviceId?: string;
  keyStorage: KeyStorage;
  /** false right after the first sign-in on this computer, until the user picks where keys live */
  keyStorageChosen?: boolean;
  /** unix ms of the last successful sync */
  lastSync?: number;
  syncError?: string;
  /** A one-off message to show, e.g. keys were removed from the account on another computer */
  notice?: string;
}

export interface AccountDevice {
  id: string;
  name: string;
  createdAt: number;
  lastSeen: number;
  /** true for the computer you are using now */
  current: boolean;
}

export type HistoryStatus =
  | "recording"
  | "transcribing"
  | "cleaning"
  | "done"
  | "queued"
  | "failed"
  | "cancelled"
  | "empty"
  /** no speech heard: hidden from History, kept 24 hours */
  | "silent";

export interface HistoryItem {
  id: string;
  /** unix ms */
  createdAt: number;
  kind: "dictation" | "command" | "transform";
  status: HistoryStatus;
  modeId: string | null;
  appKey: string | null;
  appName: string | null;
  durationMs: number;
  rawText: string | null;
  cleanText: string | null;
  finalText: string | null;
  selectedText: string | null;
  wordCount: number;
  /** local file path (.flac or .pcm); null if deleted by retention */
  audioPath: string | null;
  error: string | null;
  errorKind: string | null;
  inserted: boolean;
  attempts: number;
  timings: {
    recordingMs?: number;
    transcribeWaitMs?: number;
    cleanupMs?: number;
    totalAfterStopMs?: number;
    /** the AI model that cleaned the text up, and whether it was the fast one */
    cleanupModel?: string | null;
    cleanupFast?: boolean;
    /** live transcription: used (some text came from it), tried, and how many pieces came from it vs. the normal way */
    live?: boolean;
    liveTried?: boolean;
    liveConnected?: boolean;
    livePieces?: number;
    fallbackPieces?: number;
  } | null;
  meta: Record<string, unknown> | null;
}

export interface HistoryQuery {
  text?: string;
  from?: number;
  to?: number;
  status?: HistoryStatus;
  /** leave out items with any of these statuses */
  excludeStatus?: HistoryStatus[];
  limit?: number;
  offset?: number;
}

export interface Processed {
  id: string;
  rawText: string;
  finalText: string;
  modeId: string;
  modeName: string;
  cleanupWarning: string | null;
}

export type EntrySource = "builtin" | "user" | "learned";

export interface DictEntry {
  id: number;
  word: string;
  /** spellings always auto-corrected to `word` */
  replace: string[];
  /** sound-alikes given to the AI as hints */
  soundsLike: string[];
  source: EntrySource;
  starred: boolean;
  keyword: boolean;
  uses: number;
}

export interface Suggestion {
  word: string;
  variants: string[];
  occurrences: number;
}

export interface Snippet {
  id: number;
  trigger: string;
  expansion: string;
  uses: number;
}

export interface Note {
  id: number;
  title: string;
  body: string;
  pinned: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface Stats {
  totalWords: number;
  wordsThisWeek: number;
  wordsToday: number;
  totalDictations: number;
  wpm: number;
  minutesSaved: number;
  minutesSavedThisWeek: number;
  streakDays: number;
  longestStreak: number;
  /** last 365 days, oldest first; date = "YYYY-MM-DD" */
  daily: { date: string; words: number }[];
  topApps: { name: string; words: number }[];
  fixes: number;
  /** "all-computers" when signed in and combined with your other computers */
  scope?: "this-computer" | "all-computers";
  /** waiting time after you stop talking, over the last 50 dictations on this computer */
  speed?: Speed;
}

export interface Speed {
  samples: number;
  transcribeWaitMedianMs: number;
  cleanupMedianMs: number;
  totalWaitMedianMs: number;
  /** 0 to 1 */
  fastModelShare: number;
}

export interface ModelInfo {
  id: string;
  name: string;
  note: string;
  recommended: boolean;
}

export interface Microphone {
  id: string;
  name: string;
  isDefault: boolean;
  /** true for Bluetooth headsets (lower quality, slower to start) */
  bluetooth: boolean;
}

export interface KeyTestResult {
  ok: boolean;
  message: string;
}

export interface ImportResult {
  added: number;
  skipped: number;
}

export interface AppInfoDto {
  version: string;
  dataDir: string;
}

export type Provider = "openai" | "anthropic";
export type ModelList = "transcription" | "anthropic" | "openai";

// ---------- floating bar ----------

export type BarPhase = "hidden" | "idle" | "listening" | "processing" | "done" | "error" | "notice";

export interface BarState {
  phase: BarPhase;
  /** 0..1 live microphone level while listening */
  level: number;
  elapsedMs: number;
  handsFree: boolean;
  kind: "dictation" | "command";
  /** short status line, e.g. "Transcribing…", "Couldn't paste — copy it below" */
  message: string | null;
  /** longer plain-English explanation for errors */
  detail: string | null;
  /** last result text (for Copy) */
  text: string | null;
  dictationId: string | null;
  /** which buttons to offer: "copy" | "undo" | "rerun" | "retry" | "settings" | "history" */
  actions: string[];
  modeName: string | null;
  modes: { id: string; name: string }[];
  /** label of the dictation shortcut, for hints ("Ctrl + Win") */
  hotkeyLabel: string;
}

// ---------- commands ----------

export const api = {
  getSettings: () => call<Settings>("get_settings"),
  saveSettings: (settings: Settings) => call<Settings>("save_settings", { settings }),
  appInfo: () => call<AppInfoDto>("app_info"),

  listHistory: (query: HistoryQuery) => call<HistoryItem[]>("list_history", { query }),
  getHistoryItem: (id: string) => call<HistoryItem | null>("get_history_item", { id }),
  deleteHistoryItem: (id: string) => call<boolean>("delete_history_item", { id }),
  /** Pastes the item's text into the app that was focused before Wisper. raw = original transcript */
  reinsertHistoryItem: (id: string, raw: boolean) => call<void>("reinsert_history_item", { id, raw }),
  reprocessHistoryItem: (id: string, modeId: string) =>
    call<Processed>("reprocess_history_item", { id, modeId }),
  retryHistoryItem: (id: string) => call<Processed | null>("retry_history_item", { id }),
  /** Tries every waiting (queued or failed) recording again, one after another. */
  retryWaiting: () => call<{ done: number; stillWaiting: number }>("retry_waiting"),
  copyText: (text: string) => call<void>("copy_text", { text }),

  listDictionary: () => call<DictEntry[]>("list_dictionary"),
  addDictionaryEntry: (entry: DictEntry) => call<number>("add_dictionary_entry", { entry }),
  updateDictionaryEntry: (entry: DictEntry) => call<void>("update_dictionary_entry", { entry }),
  deleteDictionaryEntry: (id: number) => call<void>("delete_dictionary_entry", { id }),
  listSuggestions: () => call<[number, Suggestion][]>("list_suggestions"),
  resolveSuggestion: (id: number, accept: boolean) => call<void>("resolve_suggestion", { id, accept }),
  /** CSV: one word per line, or "misspelling,correction" */
  importDictionaryCsv: (text: string) => call<ImportResult>("import_dictionary_csv", { text }),

  listSnippets: () => call<Snippet[]>("list_snippets"),
  saveSnippet: (snippet: Snippet) => call<number>("save_snippet", { snippet }),
  deleteSnippet: (id: number) => call<void>("delete_snippet", { id }),
  /** JSON array of {name, text} (Wispr Flow's format) or {trigger, expansion} */
  importSnippetsJson: (text: string) => call<ImportResult>("import_snippets_json", { text }),

  listModes: () => call<Mode[]>("list_modes"),
  promptPreview: (modeId: string, appCategory?: AppCategory | null, cleanupLevel?: CleanupLevel | null) =>
    call<PromptPreview>("prompt_preview", { modeId, appCategory: appCategory ?? null, cleanupLevel: cleanupLevel ?? null }),
  defaultModeInstructions: (id: string) => call<string | null>("default_mode_instructions", { id }),

  getStats: () => call<Stats>("get_stats"),

  keyStatus: () => call<{ openai: boolean; anthropic: boolean }>("key_status"),
  setApiKey: (provider: Provider, key: string) => call<void>("set_api_key", { provider, key }),
  /** Tests the given key, or the saved one if key is omitted */
  testApiKey: (provider: Provider, key?: string) => call<KeyTestResult>("test_api_key", { provider, key }),
  listModels: (list: ModelList) => call<ModelInfo[]>("list_models", { list }),

  listMicrophones: () => call<Microphone[]>("list_microphones"),
  startMicTest: (deviceId: string | null) => call<void>("start_mic_test", { deviceId }),
  stopMicTest: () => call<void>("stop_mic_test"),

  /** Starts listening for a key combination; result arrives as a "hotkey-captured" event */
  /** Where the bar's visible pill/card is (CSS px); the rest of the bar window lets clicks through */
  barHitArea: (area: { x: number; y: number; width: number; height: number } | null) => call<void>("bar_hit_area", { area }),
  startHotkeyCapture: () => call<void>("start_hotkey_capture"),
  stopHotkeyCapture: () => call<void>("stop_hotkey_capture"),
  describeCombo: (keys: number[]) => call<{ label: string; error: string | null }>("describe_combo", { keys }),

  listNotes: (search?: string) => call<Note[]>("list_notes", { search }),
  saveNote: (note: Note) => call<number>("save_note", { note }),
  deleteNote: (id: number) => call<void>("delete_note", { id }),
  /** Start/stop a dictation whose text goes into the Scratchpad note instead of another app */
  dictateIntoScratchpad: () => call<void>("dictate_into_scratchpad"),

  runTransform: (id: string) => call<void>("run_transform", { id }),

  openDataFolder: () => call<void>("open_data_folder"),
  checkForUpdates: () => call<{ available: boolean; version: string | null; message: string }>("check_for_updates"),
  exportBackup: () => call<string>("export_backup"),
  importBackup: () => call<string>("import_backup"),

  accountStatus: () => call<AccountStatus>("account_status"),
  /** Stops waiting for the browser sign-in */
  accountCancelSignIn: () => call<void>("account_cancel_sign_in"),
  /** Deletes the API keys stored in the account (every computer keeps its own copy) */
  accountRemoveCloudKeys: () => call<AccountStatus>("account_remove_cloud_keys"),
  /** Opens the browser to sign in; resolves when done (plain-English error after 5 minutes) */
  accountSignIn: () => call<AccountStatus>("account_sign_in"),
  accountSignOut: () => call<AccountStatus>("account_sign_out"),
  accountSyncNow: () => call<AccountStatus>("account_sync_now"),
  accountSetKeyStorage: (storage: KeyStorage) => call<AccountStatus>("account_set_key_storage", { storage }),
  accountDevices: () => call<AccountDevice[]>("account_devices"),
  accountRemoveDevice: (id: string) => call<void>("account_remove_device", { id }),
  /** Deletes everything in the cloud and signs out; data on this PC stays */
  accountDelete: () => call<void>("account_delete"),

  // floating bar buttons
  barCancel: () => call<void>("bar_cancel"),
  barFinish: () => call<void>("bar_finish"),
  barStart: () => call<void>("bar_start"),
  barCopy: () => call<void>("bar_copy"),
  barUndo: () => call<void>("bar_undo"),
  barRerun: (modeId: string) => call<void>("bar_rerun", { modeId }),
  barRetry: () => call<void>("bar_retry"),
  barOpenHub: (page?: string) => call<void>("bar_open_hub", { page }),
  barDismiss: () => call<void>("bar_dismiss"),
};

// ---------- events ----------

export const events = {
  barState: (cb: (s: BarState) => void) => listen<BarState>("bar-state", (e) => cb(e.payload)),
  historyChanged: (cb: () => void) => listen<string>("history-changed", () => cb()),
  /** API keys changed (e.g. synced from another computer) */
  keysChanged: (cb: () => void) => listen<string>("keys-changed", () => cb()),
  dictionaryChanged: (cb: () => void) => listen<string>("dictionary-changed", () => cb()),
  snippetsChanged: (cb: () => void) => listen<string>("snippets-changed", () => cb()),
  settingsChanged: (cb: (s: Settings) => void) => listen<Settings>("settings-changed", (e) => cb(e.payload)),
  suggestionsChanged: (cb: () => void) => listen<null>("suggestions-changed", () => cb()),
  micLevel: (cb: (level: number) => void) => listen<number>("mic-level", (e) => cb(e.payload)),
  hotkeyCaptured: (cb: (r: { keys: number[]; label: string; done: boolean }) => void) =>
    listen<{ keys: number[]; label: string; done: boolean }>("hotkey-captured", (e) => cb(e.payload)),
  navigate: (cb: (page: string) => void) => listen<string>("navigate", (e) => cb(e.payload)),
  /** true while a Scratchpad dictation is recording */
  dictationState: (cb: (recording: boolean) => void) => listen<boolean>("dictation-state", (e) => cb(e.payload)),
  /** a dictation targeted the Scratchpad; payload is the text to insert */
  accountChanged: (cb: (s: AccountStatus) => void) => listen<AccountStatus>("account-changed", (e) => cb(e.payload)),
  scratchpadText: (cb: (text: string) => void) => listen<string>("scratchpad-text", (e) => cb(e.payload)),
};

// ---------- helpers ----------

const KEY_NAMES: Record<number, string> = {
  0x10: "Shift",
  0x11: "Ctrl",
  0x12: "Alt",
  0x5b: "Win",
  0x20: "Space",
  0x1b: "Esc",
  0x0d: "Enter",
  0x09: "Tab",
  0x08: "Backspace",
  0x14: "Caps Lock",
  0x04: "Middle mouse",
  0x05: "Mouse 4",
  0x06: "Mouse 5",
  0x25: "Left",
  0x26: "Up",
  0x27: "Right",
  0x28: "Down",
};

/** "Ctrl + Win + Space" — same ordering as the Rust side. */
export function comboLabel(combo: KeyCombo | null | undefined): string {
  if (!combo || combo.keys.length === 0) return "Not set";
  const order = (k: number) => ({ 0x11: 0, 0x5b: 1, 0x12: 2, 0x10: 3 })[k] ?? 4;
  return [...combo.keys]
    .sort((a, b) => order(a) - order(b) || a - b)
    .map((k) => {
      if (KEY_NAMES[k]) return KEY_NAMES[k];
      if (k >= 0x70 && k <= 0x87) return `F${k - 0x6f}`;
      if ((k >= 0x30 && k <= 0x39) || (k >= 0x41 && k <= 0x5a)) return String.fromCharCode(k);
      return `Key ${k}`;
    })
    .join(" + ");
}

export function formatDuration(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${String(sec).padStart(2, "0")}`;
  return `${m}:${String(sec).padStart(2, "0")}`;
}

export function compactNumber(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
  if (n >= 10_000) return `${(n / 1000).toFixed(1).replace(/\.0$/, "")}K`;
  return n.toLocaleString("en-GB");
}

# Wisper — build plan

Status: **proposal, awaiting approval** (written 22 Nov 2026).
The full spec lives in the original request; this file records the decisions and the phase plan so work can resume from any session.

## 1. Product decisions

### What Wispr Flow does that the spec didn't list (recommended additions)
Source: docs.wisprflow.ai (all desktop articles read 22 Nov 2026), reviews, /reference screenshots.

| Addition | Why | Phase |
|---|---|---|
| Cancel a dictation (Esc or ✕ on the bar) — cancelled audio still kept in history | Accidental starts are common; nothing should be lost | 1 |
| "Paste last" (Alt+Shift+Z) and "Copy last" (Alt+Shift+X) hotkeys, as in Wispr | Recovers text that went into the wrong window | 2 |
| Cleanup strength: None / Light / Medium (Wispr "Auto Cleanup") | Sometimes you want your words exactly as spoken | 2 |
| Context-aware capitalisation and spacing (continue mid-sentence in lowercase, no double spaces) | Inserted text should read as if typed in place | 2 |
| Context awareness (text around the cursor, app name, page title sent as hints) | Spells names you can see on screen; lets Auto pick the right format. Privacy toggle; password fields and chosen apps excluded | 2 |
| Spoken formatting ("new paragraph", "new line", "full stop"), spoken lists ("one… two…"), smart numbers/dates | Wispr "Smart Formatting" | 2 |
| Learn from corrections | When you fix a word right after insertion, suggest it for the dictionary | 3 |
| Dictionary CSV / snippets JSON import in Wispr's formats | Bring your existing words across | 3 |
| Microphone ranking + warnings ("no audio received", "is your mic muted?", Bluetooth call-quality) | Common cause of poor accuracy and missing first words | 3 |
| Bar: drag/dock to an edge, follows the monitor you're using, "hide for 1 hour" | Wispr behaviour you already know | 4 |
| Transforms: saved one-click rewrites on selected text (Polish, Prompt Engineer, your own) | Command mode without speaking | 2 (optional) |
| Scratchpad: notes page you can dictate into (Win+Alt+S) | Somewhere to dictate when there's no text box | 4 (optional) |
| Backup / restore of settings, dictionary, snippets, modes | Protects personalisation; moving PCs | 5 |
| Not recommended now: Notetaker (meeting recording), IDE file-tagging | Separate products; large scope | — |

### Where this app should beat Wispr Flow
- No time limit (Wispr stops at 20 min), and crash-safe: audio is written to disk while you speak, before any network call; recovered automatically after a crash.
- Hebrew/Yiddish-aware from day one (pre-loaded dictionary + cleanup rules).
- First words never cut: recording starts on the first key-down, with an optional "keep mic warm" pre-roll (Wispr's own advice is "pause after pressing the hotkey").
- Simple key rules: hold = talk, double-tap = hands-free, Esc = cancel. No silent cancels from Wispr's overlapping 0.5 s / 1 s rules.
- Clipboard restore keeps every format (Wispr loses files/rich text), and dictations stay out of Windows clipboard history.
- "Mute other audio" only touches apps that are actually playing, and is undone even after a crash (Wispr can leave apps muted).
- Offline is not failure: dictations queue and process when the connection returns; retries reapply dictionary and formatting.
- Light: Rust + system WebView instead of Electron (Wispr's Windows app is reported at ~800 MB RAM).
- One-click undo / re-run in another mode from the bar.
- Per-step timings recorded in history, so speed problems are visible, not guessed.

## 2. Tech stack
**Tauri 2** — Rust back end + React/TypeScript UI rendered by Windows' built-in WebView2.
- Installer ~10 MB (NSIS, per-user, no admin prompt). WebView2 ships with Windows 10/11; the installer embeds its bootstrapper for the rare PC without it. Static C runtime, so no Visual C++ redistributable needed.
- Rust gives direct access to Win32/WinRT for the low-level keyboard hook (hold, double-tap, modifier-only combos like Ctrl+Win), WASAPI audio, clipboard, SendInput, UI Automation (focused text field / selected text), Core Audio + media transport controls (pause/mute other audio), Credential Manager (`keyring`).
- Official updater plugin (signed updates from GitHub Releases) and GitHub Action.
- Testability: all logic lives in a pure-Rust `core` crate (no Windows or Tauri deps) that runs `cargo test` on Linux; Windows code is thin, behind traits, compiled and smoke-tested on the Windows runner. UI logic tested with Vitest.

Rejected: **Electron** (100+ MB installer, high RAM, still needs native modules for hooks). **.NET/WPF** (good Windows access, but 70–150 MB self-contained, harder to match Wispr's web-style UI, WPF can't build/test on Linux).

## 3. AI providers and models
| Job | Default | Alternatives in settings |
|---|---|---|
| Speech-to-text | OpenAI **gpt-transcribe** (Jul 2026; OpenAI's recommended, most accurate; $0.0045/min). Uses `prompt` (recent context), `keywords` (dictionary terms), `languages: ["en"]`. | gpt-live-transcribe (live streaming, $0.017/min). Older gpt-4o-transcribe / whisper-1 are deprecated and shut down 26 Feb 2027. |
| Cleanup / command mode | Claude **Sonnet 5.5** (Anthropic: "best combination of speed and intelligence"; $2 / $10 per M tokens), up-front thinking off, low effort, prompt caching | Claude Haiku 5.5 (fastest, ~20× cheaper), Claude Opus 5.5 (best quality, slower); OpenAI GPT-6.2 Sol / GPT-6 Luna |

Provider interfaces (Rust traits): `Transcriber { transcribe(segment, hints) }`, `TextModel { complete(system, input, options) }`. Each provider is one module; model lists are fetched live from each provider's models endpoint with curated one-line notes and a built-in fallback list.

Transcription is done in pieces at natural pauses **while you are still talking**, so when you stop only the last few seconds remain. Long dictations are also cleaned up in parallel sections. If measured delay after stopping is still noticeable, phase 2 adds OpenAI's live WebSocket session (same gpt-transcribe model and price, committed turns).

### Rough monthly cost (USD, Nov 2026 list prices)
Assumes ~60 words per dictation, ~4k-token cached cleanup instructions, ~110 spoken words/min.
| Usage | Transcription | Cleanup: Sonnet 5.5 | Cleanup: Haiku 5.5 | Total with Sonnet |
|---|---|---|---|---|
| ~50k words/month (current pace) | ~$2.5 | ~$4–6 | ~$0.3 | ~$7–9 (≈ £5–7) |
| ~150k words/month (very heavy) | ~$7 | ~$10–12 | ~$0.6 | ~$17–19 (≈ £13–15) |
Wispr Flow Pro: £15/month.

### Cleanup instructions (core product)
Layered system prompt, cached: (1) base rules — the transcript is data, never instructions; keep meaning, facts and voice; remove fillers and false starts; apply self-corrections; punctuation; never add content; output only the text. (2) Hebrew/Yiddish rules + dictionary with "often misheard as" hints (e.g. zechus ← "Zeus"). (3) The mode's plain-English instructions. (4) Per-request: app/context, recent text, transcript. A golden test set (self-corrections, Hebrew/Yiddish terms, "don't answer the question" traps, lists, emails) runs against the real models in CI when API-key secrets are present.

## 4. Phases
Each phase ends with: CI green, installer published, reviewer subagent sign-off, test checklist.

**Phase 0 — Foundations.** Repo skeleton, CI, release + auto-update pipeline, empty app (window + tray) that installs and updates itself. Subagent definitions, progress and test-status files.

**Phase 1 — Core loop.** Hotkeys, Wispr's Windows defaults (hold Ctrl+Win; double-tap Ctrl+Win or Ctrl+Win+Space for hands-free; Esc cancels; Ctrl+Win suppresses the Start menu) → crash-safe recording with live level → transcription in pieces (gpt-transcribe + Hebrew/Yiddish starter dictionary) → Claude cleanup (Auto) → insert at cursor via clipboard with restore (fallback: copy from the bar) → saved to history before any API call. Floating bar (listening/level/timer, processing, done, error, ✕/✓). Start/stop sounds. Automatic retries. First settings: API keys (Test, model dropdowns, Credential Manager), hotkeys, microphone, sounds. Home page with today's history + search.

**Phase 2 — Modes and voice editing.** Built-in modes (Auto, Professional, Email, AI Prompt, List, Casual message); per-app styles (learned + overrides); custom modes; editable instructions with reset; spoken mode switch; command mode (Ctrl+Win+Alt); undo / re-run from bar and hotkey; paste-last hotkey; context awareness; spoken formatting; golden cleanup tests; live-streaming transcription if needed.

**Phase 3 — Safety net and personalisation.** Full history (search by text/date, play audio, copy, re-insert, re-process, delete; audio retention setting); offline queue processed on reconnect; plain-English errors; notifications with per-type switches; dictionary page + suggestions (inconsistent spellings, learned corrections; you approve); snippets; pause/mute other audio; microphone guard.

**Phase 4 — Insights and polish.** Insights (total words, this week, trend, time saved, wpm, streak), complete grouped settings with descriptions, theme, launch at login, always-show bar, language incl. auto-detect, optional Scratchpad/Transforms, visual pass against /reference.

**Phase 5 — Account (proposal, built last).** No online account or password for a single user. Instead: profile (name for the greeting), one-click backup/restore of everything to a file or a OneDrive/Google Drive folder, optional automatic daily backup.

## 5. Pipeline
- Every push, any branch: Rust tests (Linux + Windows), UI tests, lint, Windows installer built and attached as a build artifact.
- Push to `main`: version bumped, GitHub Release published with the installer + signed update manifest; permanent link `releases/latest/download/Wisper-Setup.exe`.
- App checks for updates at start and every few hours, downloads in the background, installs when idle.
- Needs: repo public (or a separate public releases repo), and one GitHub secret `TAURI_SIGNING_PRIVATE_KEY` for update signing.
- Installer is not code-signed: first install shows SmartScreen ("More info → Run anyway"); updates don't.

## 6. Subagents (`.claude/agents/`)
| Agent | Model | Effort | Job |
|---|---|---|---|
| architect | opus | high | Architecture, cross-cutting design, tricky debugging |
| audio-pipeline | opus | high | Recording, segmentation, transcription, retries, queue |
| prompt-engineer | opus | high | Cleanup/command instructions and golden tests |
| spec-reviewer | opus | high | Read-only review of each phase against the spec |
| feature-dev | sonnet | medium | Well-scoped features (Rust or TS) |
| ui-dev | sonnet | medium | UI components matching /reference |
| test-writer | sonnet | medium | Unit tests |
| test-runner | haiku | low | Run suites, lint fixes, concise failure reports |
| researcher | haiku | low | Docs lookups and searches |

## 7. Open questions (answers change the design)
Recommended answer first.
1. **Repo visibility** — make `wisper.clone` public (free Windows builds, auto-update and the download link work without tokens; no secrets or personal details are ever committed) / keep private and publish installers to a separate public "releases" repo (needs a token you create).
2. **Hebrew/Yiddish spelling** — Ashkenazi/Yeshivish transliteration (zechus, Shabbos, Hashem, parnasa, be'H), abbreviations kept as spoken / Modern-Sephardi (zechut, Shabbat).
3. **Shortcuts** — same as Wispr (quit Wispr while trying this app) / different keys until Wispr is cancelled.
4. **Context awareness** — on, with password fields and chosen apps excluded / off.
5. **Optional extras** — Transforms and Scratchpad: include / skip. Notetaker: skip.
6. **Releases** — Claude merges each finished phase into `main` (publishes the installer) after CI + review / you merge it yourself.

## 8. Needed from the user
- Phase 0: add one GitHub secret for update signing (exact steps provided then).
- Phase 1: OpenAI and Anthropic API keys with prepaid credit, entered in the app.
- Optional: the same keys as GitHub secrets so CI can run real-API quality tests (pennies per run).

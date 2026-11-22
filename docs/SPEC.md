# Owner's spec (condensed faithfully from the original request, 22 Nov 2026)

## Goal
A personal Windows voice-dictation app to replace Wispr Flow (£15/month): as fast, polished and reliable as a commercial product. Owner is not a developer.

## Key context
- English with Hebrew/Yiddish words ("zechus", "parnasa", "b'ezras Hashem", "be'H") — must be spelled correctly in English letters, never turned into similar English words ("zechus" ≠ "Zeus"). The single most important quality requirement. Ashkenazi transliteration.
- Dictations can exceed 20 minutes; losing a long dictation is the worst failure.
- Match Wispr Flow's look/layout/flow (screenshots in /reference); improve where clearly better.

## AI providers
- Speech-to-text: OpenAI, most accurate model by default; pass dictionary words and recent context as hints.
- Cleanup, formatting, command mode: Claude by default (balance speed/quality/cost); switchable to OpenAI in settings.
- Providers behind a simple interface; cleanup instructions treated as core product.

## Development setup
- Code pushed to GitHub; GitHub Actions builds the Windows installer on every push to main; each successful build published to GitHub Releases with a permanent "latest" link.
- Fully self-contained installer; no developer tools on the owner's PC.
- App updates itself from GitHub Releases.
- Windows-specific code thin and behind interfaces; everything else unit-tested in CI. A test checklist per phase.
- No secrets in the repo; keys entered in app settings and stored in Windows Credential Manager.

## Features
1. Hotkeys: hold to talk / release to finish; double-tap for hands-free until pressed again. Recording starts within a fraction of a second of the keypress. (Owner: hotkeys customisable.)
2. Insert anywhere at the cursor; restore the previous clipboard; if insertion impossible, keep the text and allow copying from the floating bar.
3. Unlimited length; long sessions chunked/streamed so 30+ min finishes quickly and nothing is lost if a chunk fails.
4. Accurate transcription incl. accent and Hebrew/Yiddish words, using the dictionary and context.
5. Cleanup modes: Professional, Email, AI Prompt, List, Auto (format from what was said + which app). Keep meaning, facts, voice.
6. Self-corrections and filler removal; keep intended emphasis.
7. Command mode: highlight text + command hotkey → speech is an instruction; replace the highlighted text with the result.
8. Per-app styles: Auto learns which mode suits each app; user can set/override per app.
9. Undo and re-run: swap inserted text for the raw transcript, or re-run in a different mode, from the bar or a hotkey.
10. Custom modes; edit instructions of any mode incl. built-ins; built-ins have "Reset to default".
11. Spoken mode switch: "as an email", "as a list", or a custom mode name at the start → that mode for this dictation; phrase removed.
12. History: every recording + transcript saved locally (audio + text), searchable by text and date; copy, re-insert, re-process, delete. Saved before any API call. Configurable audio retention.
13. Offline and errors: keep the recording, show the problem on the bar, auto-retry temporary errors, process waiting items when back online; plain-English errors.
14. Dictionary: add words with preferred spelling; app spots inconsistently transcribed words/names and suggests adding them (user approves). Pre-loaded common Hebrew/Yiddish words.
15. Snippets: trigger phrases that expand to saved text; managed in settings.
16. Stats: total words, words this week, usage trend, time saved vs typing.
17. Floating bar: bottom centre; states listening (live level), processing, done, error; recording time on long dictations; fades away unless "always show". Polished.
18. Sounds on start/stop; volume + on/off.
19. Pause or mute other audio while dictating, restore exactly; setting with pause vs mute.
20. Windows notifications only when attention is needed (failed/waiting offline, API key stopped working, dictionary suggestion, update ready); each type switchable.
21. Settings: everything customisable, each option with a plain-English description. At least: hotkeys (dictation + command), microphone, language (incl. auto-detect), theme, launch at login, always-show bar, sounds, other-audio behaviour, notifications, default mode, per-app styles, custom modes, snippets, dictionary, history retention, API keys ("OpenAI API key (used for speech-to-text)", "Claude API key (used for cleanup)") each with Test button, link to get a key, and a Model dropdown with one-line notes and a sensible default.
22. Account and login: build last; propose how it should work for a single user.

## Owner's answers (22 Nov 2026)
Repo public: yes. Spelling: Ashkenazi. Hotkeys: customisable. Context awareness: a setting (on/off). Transforms and Scratchpad: include. Claude may push to main at any time and should build everything; API-dependent tests wait until keys are added as secrets.

## Engineering standards
Speed, reliability, polish over architecture; code that works for all real inputs; never weaken tests; short progress + test-status files; commit often; ask before destructive actions; remove scratch files.

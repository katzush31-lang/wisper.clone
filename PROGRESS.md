# Progress

## Current state (keep this section up to date)
- Latest release: see GitHub Releases (auto-published from `main`). Owner uses it daily on two PCs.
- Working: dictation everywhere, modes, voice commands, history, dictionary (Hebrew/Yiddish),
  snippets, insights, scratchpad, transforms, optional accounts + sync, live transcription,
  fast model for short dictations, Speed panel, shortcuts on/off switch.
- In progress: see "Open items".

## Open items (owner requests, newest first)
- 2026-11-23 batch: (1) hide "no speech"/failed clutter in History, (2) keys must sync on a second
  computer without toggling key storage, (3) mouse shortcuts (e.g. double-click the scroll wheel),
  (4) shortcuts must start/stop recording regardless of focus/text box, (5) copy button on each
  History row, (6) Settings page showing the exact prompts sent to the AI, (7) "Set up AI keys"
  shown on Home although keys exist. Status: being built (see git log / TEST_STATUS.md).
- Speed: owner finds Sonnet cleanup (~3 s on a long dictation) slow vs Wispr Flow. Built:
  "Tidy up while you talk" (long dictations cleaned in parts while talking; only the tail at
  stop; whole-text fallback rules in `docs/PLAN.md`). Needs: spec-reviewer pass, the CI real-API
  comparison (`--test chunked_eval`), and the owner's checklist (section 10).
- Not run yet: real-API live transcription check (needs `OPENAI_API_KEY` GitHub secret).
- Not built: learning from manual corrections; dragging/docking the floating bar.

## History
| Date | Phase | State |
|---|---|---|
| 2026-11-22 | Planning | Research, plan (`docs/PLAN.md`) approved. Repo public; updater signing secret added. |
| 2026-11-22 | 0 Foundations | CI builds + publishes installer on every push to `main`. |
| 2026-11-22 | 1–5 | Core engine, Windows layer, all UI pages. Independent review: 3 rounds, signed off. |
| 2026-11-22 | Fixes | Pages scroll, shortcut recorder fallback, floating bar click-through (window region). Real-API quality tests 12/12. |
| 2026-11-23 | Accounts (optional) | Google Sign-In, encrypted cloud API keys (per-computer choice), sync of settings/dictionary/snippets/word counts via Cloudflare Worker + D1 (`cloud/`). Design: `docs/ACCOUNTS.md`. Security review: 4 rounds, signed off. |
| 2026-11-23 | Speed | Live transcription (OpenAI Realtime, per-piece fallback to the saved recording), fast model for short dictations (Haiku 12/12 golden, ~0.63 s vs Sonnet 1.31 s), shared HTTP client + prewarm, Speed panel, optional local "skip the AI" path (off; CI eval 70/73 identical). Review signed off. |

## Decisions (keep)
- Spoken mode switch needs a pause/punctuation after the phrase ("As an email, …") or a lead-in
  with "mode/style/format" ("In list mode, …"), so ordinary sentences are never cut.
- Undo/redo of an inserted dictation (Ctrl+Z + paste) only in the same window, within 2 minutes,
  and only if nothing was typed since.
- A dictation that takes longer than ~45 s (+ a little per minute of audio) moves to the
  background queue and appears in History when done.
- Key storage is chosen per computer; "Remove keys from your account" is separate and every
  computer keeps its own copy. See `docs/ACCOUNTS.md` → Review decisions.
- Live transcription streams 24 kHz audio to OpenAI and commits at the same pauses as the saved
  recording; any problem → that piece is transcribed from the recording. After a network failure,
  live pauses for 10 minutes.

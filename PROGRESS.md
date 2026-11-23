# Progress

## Current state (keep this section up to date)
- Latest release: see GitHub Releases (auto-published from `main`). Owner uses it daily on two PCs.
- Working: dictation everywhere, modes, voice commands, history, dictionary (Hebrew/Yiddish),
  snippets, insights, scratchpad, transforms, optional accounts + sync, live transcription,
  fast model for short dictations, Speed panel, shortcuts on/off switch.
- In progress: see "Open items".

## Open items (owner requests, newest first)
- "Clean up while you talk" (tidy finished sentences of long dictations in the background so
  only the tail is left at stop): owner approved; being built in a separate worktree, then
  review + CI eval (`chunked_eval`) before release.
- Needs a real Windows check: shortcuts in Remote Desktop/Hyper-V full screen (hold 5 s+), mouse
  shortcuts, admin-window notice (checklist section 11).
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
| 2026-11-24 | Owner batch | History hides no-speech recordings (kept hidden 24 h, retryable) and groups waiting ones; copy button per row; Settings → AI instructions (exact prompts); Home keys card fixed; second computer picks synced keys automatically; shortcuts always work (non-blocking hook, watchdog, stuck-key recovery, Remote-Desktop-safe holds, admin-window notice); mouse-button shortcuts (double-click the wheel). Review: 2 rounds, signed off. |
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

# Progress

| Date | Phase | State |
|---|---|---|
| 2026-11-22 | Planning | Research, plan (`docs/PLAN.md`) approved. Repo made public; updater signing secret added. |
| 2026-11-22 | 0 Foundations | Done. CI builds + publishes installer on every push to `main` (v0.1.2 first release). |
| 2026-11-22 | 1–5 (built together) | Core engine, Windows layer, all UI pages built. Real-API behaviour not yet verified (no keys in CI). Awaiting owner testing — see `docs/TEST_CHECKLIST.md`. |

## What exists
- **Core (`crates/core`)**: hotkey gestures, audio DSP (resample, VAD, pause-based segments, FLAC, crash-safe PCM), Hebrew/Yiddish starter dictionary, dictionary/suggestions, snippets, modes + spoken switch, prompts, OpenAI/Claude providers with retry, SQLite history/stats, pipeline (transcribe while recording, cleanup with fallback, offline queue, crash recovery, retention), sounds.
- **App (`src-tauri`)**: keyboard/mouse hook with Start-menu masking, cpal capture (+warm pre-roll), clipboard paste with full restore, UI Automation context/selection, media pause/mute with crash-safe restore, controller (bar states, undo/redo-as, paste/copy last, transforms, Scratchpad dictation, queue worker, notifications), Credential Manager keys, backup/restore, auto-update.
- **UI (`src`)**: home + history detail, insights, dictionary (+suggestions, CSV import), snippets (+JSON import), style (modes, custom modes, per-app styles), transforms, scratchpad, full settings, floating bar.

## Next
- Owner tests on Windows (checklist), then fix what they find.
- Add OPENAI/ANTHROPIC API keys as GitHub secrets to run the real-API quality tests.
- Possible later: live WebSocket transcription if the wait after stopping feels slow; learning from manual corrections; bar drag/dock.

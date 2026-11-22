# Test status

| Suite | Where | Status |
|---|---|---|
| Rust core unit + pipeline tests (`cargo test -p wisper-core`): 123 tests (UI: 29) | CI Linux + Windows | passing |
| UI typecheck + Vitest (24 tests) | CI Linux | passing |
| Windows compile of the app (`cargo xwin check`, and the real build on CI) | local + CI Windows | passing |
| Windows installer build + release | CI Windows | passing (latest release published automatically; signed updates) |
| Real-API cleanup quality tests (`crates/core/tests/golden.rs`, 12 cases) | CI (uses the `ANTHROPIC_API_KEY` secret) | 12/12 passing with Claude Sonnet 5.5 (2026-11-22) |
| Visual check of every page, settings section and bar state (light + dark) | local Playwright with mocked backend | done; issues fixed |
| On-device checks (hotkeys, mic, paste, media) | owner's PC | not yet — see `docs/TEST_CHECKLIST.md` |
| Independent code review against the spec | spec-reviewer subagent | signed off (round 3); its low-priority notes fixed |

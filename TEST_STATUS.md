# Test status

| Suite | Where | Status |
|---|---|---|
| Rust core unit + pipeline tests (`cargo test -p wisper-core`): 123 tests | CI Linux + Windows | passing |
| UI typecheck + Vitest (24 tests) | CI Linux | passing |
| Windows compile of the app (`cargo xwin check`, and the real build on CI) | local + CI Windows | passing |
| Windows installer build + release | CI Windows | passing (latest release published automatically; signed updates) |
| Real-API cleanup quality tests (`crates/core/tests/golden.rs`, 12 cases) | CI, when `ANTHROPIC_API_KEY` secret exists | skipped — no key yet |
| Visual check of every page, settings section and bar state (light + dark) | local Playwright with mocked backend | done; issues fixed |
| On-device checks (hotkeys, mic, paste, media) | owner's PC | not yet — see `docs/TEST_CHECKLIST.md` |
| Independent code review against the spec | spec-reviewer subagent | rounds 1–2 fixed; round 3 pending |

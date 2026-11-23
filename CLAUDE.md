# Wisper — notes for Claude sessions

Personal Windows voice-dictation app (a Wispr Flow replacement). Owner is not a developer: explain in plain language, keep updates short.

## Layout
- `crates/core` — all platform-independent logic (pipeline, audio DSP/VAD/encoding, providers, prompts, modes, dictionary, snippets, history DB, stats). Must build and test on Linux: `cargo test -p wisper-core`.
- `src-tauri` — Tauri 2 app: windows (hub `main`, floating `bar`), tray, updater, Tauri commands, and `src/platform/` (Windows-only code behind traits, with no-op stubs on other OSes).
- `src/` — React 19 + TypeScript UI. `src/hub` (main window), `src/bar` (floating bar), `src/shared` (theme, typed API wrappers).
- `docs/SPEC.md` (the owner's spec), `docs/PLAN.md` (decisions + phases), `PROGRESS.md`, `TEST_STATUS.md`.
- `reference/` — Wispr Flow screenshots, for feature reference only. The UI deliberately does NOT copy Flow's look (see "UI design" below).

## Rules
- Never lose a dictation: audio is on disk before any network call; failures keep everything for retry.
- No secrets in the repo. API keys live in Windows Credential Manager (keyring). Updater signing key is the GitHub secret `TAURI_SIGNING_PRIVATE_KEY`.
- Hebrew/Yiddish terms use Ashkenazi transliteration (zechus, Shabbos, Hashem, be'H).
- Every setting in the UI has a one-line plain-English description.
- Don't weaken or delete tests to make them pass.
- Pushing to `main` publishes a release (GitHub Actions `build.yml`); the owner has given permission to push to `main`.

## UI design (since the Nov 2026 redesign)
- Own identity: light content under a dark spruce header band, one spruce-green accent, Inter only (no serif), sentence-case labels (no ALL-CAPS eyebrows).
- Layout: no sidebar. A dark header band holds the brand, Ctrl+K "Jump to", theme/help and window buttons, plus every page as a tab (the open tab joins the page). Settings is a full page with its sections as pill tabs along the top. Pages are left-aligned under the tabs.
- Home: warning (if keys missing) → one wide strip (shortcut as keys | tips | numbers) → history as one continuous list (time | text | app | copy).
- Pages and settings sections are listed once in `src/hub/nav.tsx`; add new ones there so the page tabs, Settings and Jump-to stay in sync.
- Keyboard: Ctrl+K jump to, Ctrl+1–7 pages, Ctrl+, settings. Keep visible focus, AA contrast, and reduced-motion support.
- Design skills in `.claude/skills/` (frontend-design, accessibility-review) — use them for UI work.

## Commands
- `npm run typecheck`, `npm test`, `npm run build`
- `cargo test -p wisper-core`, `cargo build -p wisper` (Linux needs webkit2gtk dev libs)
- Release build on Windows CI: `npx tauri build`

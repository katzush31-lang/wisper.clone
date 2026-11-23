# Accounts, cloud keys and sync — design and contracts

Approved by the owner on 2026-11-23. This is the single source of truth for the three parts
(Cloudflare Worker, desktop Rust, desktop UI). Change it here first if a contract must change.

## Owner requirements
- Signing in is **optional**. Without an account Wisper works exactly as before (keys in Windows
  Credential Manager, settings file, local stats).
- Sign-in: **Google Sign-In** only (no passwords stored anywhere).
- When signed in, the user chooses **where API keys live**: `local` (this computer only, never
  uploaded) or `cloud` (encrypted on our Worker, synced to every computer they sign in on).
- When signed in, these sync across computers: API keys (if `cloud`), synced settings sections,
  user dictionary entries, snippets, and **usage stats** (numbers only, never dictated text).
- History text, audio, Scratchpad notes stay on each computer.
- One user can never read another user's data. No secrets are compiled into the app.
- Never lose or overwrite existing local data: back up before the first sync.

## Infrastructure
- Cloudflare account (owner's). Worker name: `wisper-api`. D1 database `wisper`,
  id `6e1cd762-f510-49e3-8235-835156ea314d` (WEUR). Public URL: `https://wisper-api.<subdomain>.workers.dev`.
- Worker secrets (never in git, set by CI): `MASTER_KEY` (32 random bytes, base64; generated once
  in CI, never printed), `TOKEN_SECRET` (32 random bytes, base64; generated once),
  `GOOGLE_CLIENT_SECRET` (from GitHub secret). Var: `GOOGLE_CLIENT_ID` (from GitHub secret).
- GitHub secrets the owner adds: `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`,
  `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`.
- The desktop app learns the Worker URL at build time: env `WISPER_CLOUD_URL` read with
  `option_env!`. If unset, account features show "Accounts aren't available in this build".

## Security design
- **Key encryption (Worker):** AES-256-GCM via WebCrypto. Per-user key =
  HKDF-SHA256(ikm = MASTER_KEY, salt = "wisper-keys-v1", info = user_id) → 256-bit AES key.
  Random 12-byte IV per write. AAD = `"{user_id}:{provider}"` (a ciphertext copied to another user
  or provider fails to decrypt). Store `ciphertext`, `iv`, `key_version` (=1). Plain keys are
  never logged or stored.
- **Access tokens:** compact HMAC-SHA256 signed token (JWT format, HS256, `TOKEN_SECRET`),
  claims `{sub: user_id, did: device_id, exp: now+15min, iat}`. Verified on every `/v1` call;
  device must still exist and not be revoked.
- **Refresh tokens:** 32 random bytes, base64url. Only SHA-256 hash stored (devices table).
  Rotated on every refresh. Presenting an already-rotated token revokes that device (theft
  detection). Lifetime 180 days since last use.
- **Isolation:** every data query takes `user_id` ONLY from the verified token, via one helper
  (`requireUser(req, env)`) and SQL that always includes `WHERE user_id = ?`. No endpoint accepts a
  user id from the client. Tests must prove user B cannot read/modify user A's keys, settings,
  items, stats or devices (including by guessing ids).
- **Google login (server-side OAuth + PKCE, loopback):**
  1. App picks a random free port, starts a one-shot HTTP listener on `127.0.0.1:{port}`,
     generates `verifier` (43–128 chars) and `challenge = base64url(SHA256(verifier))`.
  2. App opens browser: `GET /auth/google/start?challenge=…&port=…`. Worker stores a
     login_request `{id, challenge, port, created_at}` (expires 10 min) and redirects to Google
     with `state = id`, scope `openid email profile`, `prompt=select_account`,
     redirect_uri = `{worker}/auth/google/callback`.
  3. Google → `/auth/google/callback?code&state`. Worker exchanges code at Google's token endpoint
     with client secret (server-to-server over TLS), reads the `id_token` claims and checks
     `aud == GOOGLE_CLIENT_ID`, `iss` is accounts.google.com, `exp` in future, `email_verified`.
     Upserts user by Google `sub`. Creates a one-time `login_code` (random 32 bytes; store hash,
     expires 2 min, single use) tied to the login_request, then redirects to
     `http://127.0.0.1:{port}/callback?code=…`.
  4. App's listener replies with a small "You're signed in — you can close this tab" page, then
     calls `POST /auth/token {code, verifier, device_name}`. Worker checks code hash, expiry,
     unused, and `base64url(SHA256(verifier)) == challenge`; creates a device; returns tokens.
- **Rate limits:** `/auth/*` limited per IP (Workers rate-limiting binding, 20/min).
- **Desktop storage:** refresh token in Windows Credential Manager (service `Wisper`, account
  `cloud-refresh-token`). Access token in memory only. Cloud-mode API keys are cached in
  Credential Manager exactly as local keys are today (same entries), so dictation never waits on
  the network and works offline.
- **Sign out:** revoke device on server, delete refresh token; if key storage is `cloud`, delete the
  locally cached keys too. Settings/dictionary/snippets stay (they are the user's on this PC).
- **Revoked/expired:** refresh fails with 401 → app becomes signed-out, shows "Please sign in
  again"; if it was a revocation (`{"error":"revoked"}`) and storage was `cloud`, delete cached keys.

## D1 schema (`cloud/migrations/0001_init.sql`)
```
users(id TEXT PK, google_sub TEXT UNIQUE NOT NULL, email TEXT NOT NULL, name TEXT, created_at INTEGER NOT NULL)
devices(id TEXT PK, user_id TEXT NOT NULL, name TEXT NOT NULL, refresh_hash TEXT NOT NULL,
        prev_refresh_hash TEXT, created_at INTEGER, last_seen INTEGER, revoked INTEGER DEFAULT 0)
login_requests(id TEXT PK, challenge TEXT NOT NULL, port INTEGER NOT NULL, created_at INTEGER NOT NULL,
               code_hash TEXT, code_expires INTEGER, user_id TEXT, used INTEGER DEFAULT 0)
api_keys(user_id TEXT, provider TEXT, ciphertext TEXT, iv TEXT, key_version INTEGER, updated_at INTEGER,
         PRIMARY KEY(user_id, provider))
settings(user_id TEXT, section TEXT, value TEXT NOT NULL, updated_at INTEGER NOT NULL,
         PRIMARY KEY(user_id, section))
items(user_id TEXT, kind TEXT, uid TEXT, data TEXT, updated_at INTEGER NOT NULL, deleted INTEGER DEFAULT 0,
      PRIMARY KEY(user_id, kind, uid))
stats(user_id TEXT, uid TEXT, device_id TEXT, created_at INTEGER, words INTEGER, duration_ms INTEGER,
      app TEXT, fixes INTEGER, PRIMARY KEY(user_id, uid))
```
Indexes on `devices(user_id)`, `items(user_id, updated_at)`, `stats(user_id, created_at)`.

## HTTP API (JSON; errors `{"error": "<code>", "message": "<plain English>"}`)
Auth: `Authorization: Bearer <access>` on all `/v1/*`.
- `GET /auth/google/start?challenge&port` → 302. `GET /auth/google/callback` → 302 to loopback.
- `POST /auth/token {code, verifier, device_name}` → `{access_token, refresh_token, expires_in, user:{id,email,name}, device_id}`
- `POST /auth/refresh {refresh_token}` → same shape (new refresh token). 401 `invalid`/`revoked`.
- `POST /auth/logout` (bearer) → 204, revokes this device.
- `GET /v1/me` → `{user:{id,email,name}, device_id}`
- `GET /v1/devices` → `[{id,name,created_at,last_seen,current}]`; `DELETE /v1/devices/:id` → 204
- `DELETE /v1/account` → 204 (deletes every row for the user).
- `GET /v1/keys` → `{openai: {key, updated_at} | null, anthropic: … | null}`
- `PUT /v1/keys/:provider {key}` (provider ∈ openai|anthropic; empty key = delete) → `{updated_at}`
- `DELETE /v1/keys` → 204 (all providers; used when switching to `local`).
- `GET /v1/settings` → `{sections: {name: {value: <json>, updated_at}}}`
- `PUT /v1/settings {sections: {name: {value, updated_at}}}` → per section, keep the newer
  `updated_at` (last write wins); returns the merged full `sections`.
- `GET /v1/items?since=<ms>` → `{items:[{kind,uid,data,updated_at,deleted}], now}`
- `POST /v1/items {items:[…]}` → upsert where incoming `updated_at` > stored. Kinds: `dictionary`, `snippet`. Max 500 per call.
- `POST /v1/stats {rows:[{uid,created_at,words,duration_ms,app,fixes}]}` → insert-or-ignore (device_id from token). Max 1000.
- `GET /v1/stats?since=<created_at ms>` → `{rows:[{uid,device_id,created_at,words,duration_ms,app,fixes}]}`
Body size limit 1 MB. Timestamps are client ms since epoch.
**Sync cursors use the server's clock** (so a computer with a wrong clock can't cause missed
changes): `items` and `stats` rows store `server_at` (Worker time when written). `GET /v1/items?since`
and `GET /v1/stats?since` filter on `server_at >= since` and return `now` (server time); the client
stores that `now` as its next cursor. `updated_at`/`created_at` stay client times and are used only
for last-write-wins and for displaying stats. `GET /v1/stats` also returns `now`; if it returned
the maximum row count (`more: true`), the client calls again with `since` = the last row's `server_at`
(rows include `server_at`).

## Desktop (Rust)
- `Settings` gains `account: AccountSettings { key_storage: KeyStorage (Local|Cloud, default Cloud),
  sync_enabled: bool (default true) }`. Default Cloud only matters once signed in; the first-sign-in
  screen asks explicitly.
- **Synced settings sections** (each a top-level settings field, tracked with a local
  `updated_at` per section in `section_times: HashMap<String,i64>` stored in settings.json):
  `userName, hotkeys, shortcutsEnabled, languages, theme, alwaysShowBar, barPosition, sounds,
  otherAudio, notifications, defaultMode, cleanupLevel, contextAwareness, contextExcludedApps,
  appStyles, learnAppStyles, modeOverrides, customModes, transforms, keepAudioDays,
  keepHistoryDays, providers, typingWpm, smartFormatting`.
  **Never synced:** `microphone, keepMicWarm, launchAtLogin, onboarded, version, account` —
  key storage is a **per-computer** choice (decided after review): choosing "This computer only"
  on one PC can never be overridden by another PC.
- SQLite migration: `dictionary` and `snippets` gain `uid TEXT` (random UUID, backfilled),
  `updated_at INTEGER`, `deleted INTEGER DEFAULT 0` (soft delete; UI/pipeline ignore deleted rows).
  Builtin dictionary rows are not synced unless the user edited them (source becomes user).
- `crates/core/src/cloud/` (pure, Linux-tested): API client (reqwest), token handling, merge
  logic (`merge_sections`, `merge_items`, stats union by uid), sync engine with a trait for token
  storage so tests use fakes and a local mock server (e.g. `tokio` + `axum`-free hyper or a tiny
  handwritten server; prefer `wiremock` dev-dependency).
- Sync triggers: on sign-in, on app start, when the hub window gains focus, every 5 minutes, and
  ~2 s after a local change (debounced). Offline → retry later; never blocks dictation.
- First sign-in on a PC: save `settings.before-signin.json` + DB backup (existing backup code) →
  if the account has no settings, upload local; else take account sections, keep local
  never-synced fields; dictionary/snippets merged (union by word/trigger, newest wins).
  Keys: if storage is `cloud` and account has keys → use them; if account has none and the PC has
  keys → upload. If storage is `local` → keys never uploaded.
- Switching storage Cloud→Local: confirm, then `DELETE /v1/keys`; local copies kept.
  Local→Cloud: upload this PC's keys.
- Stats: each completed dictation produces a stats row `{uid = dictation id, created_at, words,
  duration_ms, app, fixes}` uploaded when signed in. Insights shows "All your computers"
  (local rows ∪ rows from other devices, deduped by uid) when signed in, otherwise this PC.

## Tauri commands (UI contract)
- `account_status()` → `{available: bool, signedIn: bool, email?, name?, deviceId?, keyStorage: "local"|"cloud", lastSync?: ms, syncError?: string}`
- `account_sign_in()` → starts browser login; resolves when done (or errors with plain English; 5-minute timeout); returns `account_status`.
- `account_sign_out()` → status. `account_sync_now()` → status.
- `account_set_key_storage(storage)` → status. `account_devices()` → `[{id,name,createdAt,lastSeen,current}]`
- `account_remove_device(id)`, `account_delete()` (deletes cloud data, signs out).
- Event `account-changed` (payload = status) whenever status changes (sign-in, sync, expiry).
- Existing `get_stats` returns combined stats when signed in; add field `scope: "this-computer"|"all-computers"`.

## UI
- Settings section "Account" (replaces "Backup & account"; backup/restore controls stay in it):
  signed out → short explanation, **Sign in with Google**, note "Wisper works fully without an
  account". Signed in → email, sync status ("Synced 2 minutes ago" / error), "Where to keep your
  API keys" segmented control (This computer only / Your account, synced) with one-line
  description, devices list with "Sign out" per other device, Sign out, Delete account (confirm).
- AI providers section: a one-line note under the key boxes saying where keys are stored.
- First run (onboarding) and Home: a small dismissible card "Use Wisper on more than one
  computer? Sign in to sync" — never blocking.
- Insights: label "All your computers" or "This computer".
- Every setting has a one-line plain-English description.

## Review decisions (2026-11-23)
- Keys are only ever deleted locally when key syncing is active on this computer (Cloud AND
  chosen) AND the account holds exactly that key (stored hash matches). Before the choice is
  made, sign-out/revocation never touches keys.
- Deleting the account keeps this computer's local keys (only the cloud copy is destroyed).
- Revocation reasons: `{"error":"revoked","reason":"reuse"|"removed"}`. `reuse` (a replayed
  refresh token — possibly just a dropped response) signs out but keeps local keys; `removed`
  (signed out from another computer / logout) clears cloud-synced keys.
- Remote settings are never applied while recording or processing; they wait until idle.
- Google failures in the callback redirect to the loopback with `error=google_failed` so the
  app stops waiting. The app has `account_cancel_sign_in`.

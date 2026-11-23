# Wisper cloud (Cloudflare Worker `wisper-api`)

Backend for Wisper's optional accounts: Google sign-in, encrypted API-key storage, and sync of
settings, dictionary, snippets and usage numbers. The contract is in `../docs/ACCOUNTS.md`.
Wisper works fully without any of this.

## Everyday commands
    npm ci
    npm test            # unit + integration tests (real D1 in Miniflare, Google is mocked)
    npm run typecheck

Deployment is automatic: pushing to `main` runs the `cloud-deploy` job in
`.github/workflows/build.yml` (migrations, deploy, secrets), then builds the installer with the
Worker's URL baked in as `WISPER_CLOUD_URL`.

## GitHub secrets (set by the owner)
| Secret | What it is |
| --- | --- |
| `CLOUDFLARE_API_TOKEN` | Lets CI deploy the Worker and run D1 migrations (Workers + D1 edit rights). If missing, the deploy is skipped and the app is built without accounts. |
| `CLOUDFLARE_ACCOUNT_ID` | Which Cloudflare account to deploy to. |
| `GOOGLE_CLIENT_ID` | Google OAuth client id (public). Passed to the Worker as a variable at deploy time. |
| `GOOGLE_CLIENT_SECRET` | Google OAuth client secret. CI re-uploads it to the Worker on every deploy. |

Google's OAuth client must list `https://wisper-api.<subdomain>.workers.dev/auth/google/callback`
as an authorised redirect URI.

## Worker secrets (created by CI, never in git)
| Secret | What it does | Created |
| --- | --- | --- |
| `MASTER_KEY` | 32 random bytes. Every user's API-key encryption key is derived from it. | Once, in CI, only if missing. Never printed. |
| `TOKEN_SECRET` | Signs the short-lived access tokens. | Once, in CI, only if missing. Never printed. |
| `GOOGLE_CLIENT_SECRET` | See above. | Every deploy. |

## Important: MASTER_KEY
**If `MASTER_KEY` is deleted or replaced, every API key stored in the cloud becomes permanently
unreadable.** There is no copy anywhere. People would have to type their keys in again (the app
keeps a local copy on each computer, so they can simply be re-uploaded by switching key storage
off and on). CI never regenerates an existing `MASTER_KEY`, and stops rather than guess if it
cannot read the list of secrets.

## Rotation
- `TOKEN_SECRET`: safe to rotate at any time. Run `npx wrangler secret put TOKEN_SECRET` with a new
  `openssl rand -base64 32` value (or delete it and let the next deploy create one). Everybody's
  15-minute access tokens stop working and the apps silently fetch new ones using their refresh
  tokens, so nobody has to sign in again.
- `GOOGLE_CLIENT_SECRET`: change the GitHub secret and push; CI uploads it.
- `MASTER_KEY`: there is no automatic rotation. Keys are stored with `key_version = 1`; a future
  version would add a second master key, re-encrypt each row, then retire the old one. Until that
  exists, treat the key as permanent.
- A stolen device: the user removes it in Settings > Account, or `DELETE /v1/devices/:id`.

## Notes
- No passwords exist anywhere; sign-in is Google only. No dictated text is ever sent here.
- The free Workers plan allows 10 ms of CPU per request, so the code avoids heavy work (no password
  hashing, one SQL statement per batch upload).
- Rate limit: 20 requests per minute per IP on `/auth/*` (`AUTH_LIMITER` binding in `wrangler.jsonc`).
- `npm install` needs `legacy-peer-deps` (see `.npmrc`) because the test package's peer
  dependencies trip an npm bug.
- `@cloudflare/vitest-pool-workers` has been renamed `@cloudflare/vitest-plugin` upstream; the
  pinned version still works. Migrating later is a small change to `vitest.config.ts`.

// /auth/* : Google sign-in (server-side OAuth + PKCE via a loopback listener), token refresh, logout.
import { requireUser } from "./auth";
import { randomToken, sha256B64url, timingSafeEqual } from "./crypto";
import { exec, one, stmt } from "./db";
import { exchangeCodeForIdentity, googleAuthUrl } from "./google";
import {
  HttpError, htmlPage, json, noContent, readJsonBody, redirect, requireString, revokedError,
} from "./http";
import { ACCESS_TOKEN_TTL_SECONDS, signAccessToken } from "./tokens";
import type { Deps, Env } from "./types";

export const LOGIN_REQUEST_TTL_MS = 10 * 60 * 1000;
export const LOGIN_CODE_TTL_MS = 2 * 60 * 1000;
export const REFRESH_TTL_MS = 180 * 24 * 60 * 60 * 1000;
export const MAX_ACTIVE_DEVICES = 20;

const CHALLENGE_RE = /^[A-Za-z0-9_-]{43}$/; // base64url of a SHA-256 digest
const VERIFIER_RE = /^[A-Za-z0-9\-._~]{43,128}$/;

interface UserRow {
  id: string;
  email: string;
  name: string | null;
}

// ---------- GET /auth/google/start ----------

export async function googleStart(req: Request, env: Env): Promise<Response> {
  const url = new URL(req.url);
  const challenge = url.searchParams.get("challenge") ?? "";
  const port = Number(url.searchParams.get("port"));
  if (!CHALLENGE_RE.test(challenge) || !Number.isInteger(port) || port < 1024 || port > 65535) {
    return htmlPage("This sign-in link is not valid. Please start again from Wisper.", 400);
  }

  const now = Date.now();
  const id = randomToken();
  // Tidy up old requests as we go so the table stays small.
  await exec(env.DB, "DELETE FROM login_requests WHERE created_at < ?", now - 60 * 60 * 1000);
  await exec(
    env.DB,
    "INSERT INTO login_requests (id, challenge, port, created_at, used) VALUES (?, ?, ?, ?, 0)",
    id, challenge, port, now,
  );
  return redirect(googleAuthUrl(env, `${url.origin}/auth/google/callback`, id));
}

// ---------- GET /auth/google/callback ----------

export async function googleCallback(req: Request, env: Env, deps: Required<Deps>): Promise<Response> {
  const url = new URL(req.url);
  const state = url.searchParams.get("state") ?? "";
  const code = url.searchParams.get("code") ?? "";
  const now = Date.now();

  const lr = state
    ? await one<{ id: string; port: number; created_at: number; code_hash: string | null }>(
        env.DB,
        "SELECT id, port, created_at, code_hash FROM login_requests WHERE id = ?",
        state,
      )
    : null;
  if (!lr || lr.code_hash !== null) {
    return htmlPage("This sign-in link is not valid or was already used. Please start again from Wisper.", 400);
  }
  if (now - lr.created_at > LOGIN_REQUEST_TTL_MS) {
    return htmlPage("This sign-in took too long. Please close this tab and start again from Wisper.", 400);
  }
  const loopback = (query: string) => redirect(`http://127.0.0.1:${lr.port}/callback?${query}`);

  if (!code) return loopback("error=access_denied"); // user cancelled at Google

  let identity;
  try {
    identity = await exchangeCodeForIdentity(env, deps.fetch, deps.googleTokenUrl, code, `${url.origin}/auth/google/callback`);
  } catch (e) {
    // The port is known here, so send the browser back and let the app stop waiting.
    if (e instanceof HttpError) return loopback("error=google_failed");
    throw e;
  }

  // Upsert by Google's stable user id.
  await exec(
    env.DB,
    `INSERT INTO users (id, google_sub, email, name, created_at) VALUES (?, ?, ?, ?, ?)
     ON CONFLICT(google_sub) DO UPDATE SET email = excluded.email, name = excluded.name`,
    crypto.randomUUID(), identity.sub, identity.email, identity.name, now,
  );
  const user = await one<{ id: string }>(env.DB, "SELECT id FROM users WHERE google_sub = ?", identity.sub);
  if (!user) throw new HttpError(500, "server_error", "Something went wrong on our side. Please try again.");

  const loginCode = randomToken();
  const changed = await exec(
    env.DB,
    "UPDATE login_requests SET code_hash = ?, code_expires = ?, user_id = ? WHERE id = ? AND code_hash IS NULL",
    await sha256B64url(loginCode), now + LOGIN_CODE_TTL_MS, user.id, lr.id,
  );
  if (changed !== 1) return htmlPage("This sign-in link was already used. Please start again from Wisper.", 400);
  return loopback(`code=${encodeURIComponent(loginCode)}`);
}

// ---------- device + token issuing ----------

async function issueSession(env: Env, user: UserRow, deviceId: string, refreshToken: string) {
  return {
    access_token: await signAccessToken(env.TOKEN_SECRET, user.id, deviceId),
    refresh_token: refreshToken,
    expires_in: ACCESS_TOKEN_TTL_SECONDS,
    user: { id: user.id, email: user.email, name: user.name },
    device_id: deviceId,
  };
}

function cleanDeviceName(v: unknown): string {
  const raw = typeof v === "string" ? v : "";
  // eslint-disable-next-line no-control-regex
  const name = raw.replace(/[\u0000-\u001f\u007f]/g, "").trim().slice(0, 80);
  return name || "Unnamed computer";
}

// ---------- POST /auth/token ----------

export async function exchangeLoginCode(req: Request, env: Env): Promise<Response> {
  const body = await readJsonBody(req);
  const code = requireString(body.code, "The code", 200);
  const verifier = requireString(body.verifier, "The verifier", 200);
  const deviceName = cleanDeviceName(body.device_name);
  const invalid = new HttpError(400, "invalid_code", "That sign-in code is not valid any more. Please sign in again.");
  const now = Date.now();

  const lr = await one<{ id: string; challenge: string; code_expires: number | null; user_id: string | null; used: number }>(
    env.DB,
    "SELECT id, challenge, code_expires, user_id, used FROM login_requests WHERE code_hash = ?",
    await sha256B64url(code),
  );
  if (!lr || lr.used !== 0 || !lr.user_id || !lr.code_expires || lr.code_expires <= now) throw invalid;

  // Single use: claim the code atomically BEFORE checking the verifier. A wrong verifier burns
  // the code, so a stolen code cannot be guessed against.
  if ((await exec(env.DB, "UPDATE login_requests SET used = 1 WHERE id = ? AND used = 0", lr.id)) !== 1) throw invalid;
  if (!VERIFIER_RE.test(verifier)) throw invalid;
  if (!timingSafeEqual(await sha256B64url(verifier), lr.challenge)) throw invalid;

  const user = await one<UserRow>(env.DB, "SELECT id, email, name FROM users WHERE id = ?", lr.user_id);
  if (!user) throw invalid;

  const deviceId = crypto.randomUUID();
  const refreshToken = randomToken();
  await env.DB.batch([
    // Keep at most MAX_ACTIVE_DEVICES: retire the least recently used ones.
    stmt(
      env.DB,
      `UPDATE devices SET revoked = 1, revoked_reason = 'removed'
       WHERE user_id = ? AND revoked = 0 AND id IN (
         SELECT id FROM devices WHERE user_id = ? AND revoked = 0
         ORDER BY COALESCE(last_seen, created_at) DESC LIMIT -1 OFFSET ?)`,
      user.id, user.id, MAX_ACTIVE_DEVICES - 1,
    ),
    stmt(
      env.DB,
      `INSERT INTO devices (id, user_id, name, refresh_hash, prev_refresh_hash, created_at, last_seen, revoked)
       VALUES (?, ?, ?, ?, NULL, ?, ?, 0)`,
      deviceId, user.id, deviceName, await sha256B64url(refreshToken), now, now,
    ),
  ]);
  return json(await issueSession(env, user, deviceId, refreshToken));
}

// ---------- POST /auth/refresh ----------

export async function refresh(req: Request, env: Env): Promise<Response> {
  const body = await readJsonBody(req);
  const presented = requireString(body.refresh_token, "The refresh token", 200);
  const hash = await sha256B64url(presented);
  const now = Date.now();
  const invalid = new HttpError(401, "invalid", "Please sign in again.");

  // Pre-auth lookup: the refresh token itself is the credential, so there is no user id yet.
  // (Lookup is by SHA-256 of a 256-bit random value; there is nothing useful to time.)
  const dev = await one<{
    id: string; user_id: string; revoked: number; revoked_reason: string | null; created_at: number | null; last_seen: number | null;
    email: string; name: string | null;
  }>(
    env.DB,
    `SELECT d.id, d.user_id, d.revoked, d.revoked_reason, d.created_at, d.last_seen, u.email, u.name
     FROM devices d JOIN users u ON u.id = d.user_id WHERE d.refresh_hash = ?`,
    hash,
  );

  if (!dev) {
    // An already-rotated token being replayed means it was copied: revoke that device.
    const reused = await one<{ id: string; revoked: number; revoked_reason: string | null }>(
      env.DB,
      "SELECT id, revoked, revoked_reason FROM devices WHERE prev_refresh_hash = ?",
      hash,
    );
    if (reused) {
      if (reused.revoked) throw revokedError(reused.revoked_reason); // keep the original reason
      await exec(env.DB, "UPDATE devices SET revoked = 1, revoked_reason = 'reuse' WHERE id = ? AND revoked = 0", reused.id);
      throw revokedError("reuse");
    }
    throw invalid;
  }
  if (dev.revoked) throw revokedError(dev.revoked_reason);
  if (now - (dev.last_seen ?? dev.created_at ?? 0) > REFRESH_TTL_MS) throw invalid;

  const next = randomToken();
  // Compare-and-swap so two simultaneous refreshes cannot both succeed.
  const changed = await exec(
    env.DB,
    `UPDATE devices SET prev_refresh_hash = refresh_hash, refresh_hash = ?, last_seen = ?
     WHERE id = ? AND user_id = ? AND refresh_hash = ? AND revoked = 0`,
    await sha256B64url(next), now, dev.id, dev.user_id, hash,
  );
  if (changed !== 1) throw invalid;

  return json(await issueSession(env, { id: dev.user_id, email: dev.email, name: dev.name }, dev.id, next));
}

// ---------- POST /auth/logout ----------

export async function logout(req: Request, env: Env): Promise<Response> {
  const { userId, deviceId } = await requireUser(req, env);
  await exec(env.DB, "UPDATE devices SET revoked = 1, revoked_reason = 'removed' WHERE id = ? AND user_id = ? AND revoked = 0", deviceId, userId);
  return noContent();
}

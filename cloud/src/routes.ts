// /v1/* handlers. Each one starts with requireUser() and every SQL statement on user data
// contains `user_id = ?` bound from that verified token.
import { requireUser } from "./auth";
import { decryptApiKey, encryptApiKey } from "./crypto";
import { all, clampFuture, exec, one, stmt } from "./db";
import {
  badRequest, HttpError, isRecord, json, noContent, readJsonBody, requireInt,
} from "./http";
import type { Env } from "./types";

export const PROVIDERS = ["openai", "anthropic"] as const;
export const ITEM_KINDS = ["dictionary", "snippet"] as const;
export const MAX_ITEMS_PER_CALL = 500;
export const MAX_STATS_PER_CALL = 1000;
export const MAX_SETTING_SECTIONS = 64;
const MAX_KEY_LENGTH = 512;
const MAX_SECTION_BYTES = 256 * 1024;
const MAX_ITEM_DATA_BYTES = 64 * 1024;
const STATS_PAGE = 20000;
const ID_RE = /^[A-Za-z0-9._:-]{1,100}$/;
const SECTION_RE = /^[A-Za-z0-9._-]{1,64}$/;

type Provider = (typeof PROVIDERS)[number];
const isProvider = (v: string): v is Provider => (PROVIDERS as readonly string[]).includes(v);
const isKind = (v: unknown): v is (typeof ITEM_KINDS)[number] =>
  typeof v === "string" && (ITEM_KINDS as readonly string[]).includes(v);

const notFound = () => new HttpError(404, "not_found", "That was not found.");

// ---------- me / devices / account ----------

export async function getMe(req: Request, env: Env): Promise<Response> {
  const { userId, deviceId } = await requireUser(req, env);
  const user = await one<{ id: string; email: string; name: string | null }>(
    env.DB,
    "SELECT id, email, name FROM users WHERE id = ?", // users.id IS the token's user id
    userId,
  );
  if (!user) throw new HttpError(401, "invalid", "Please sign in again.");
  return json({ user, device_id: deviceId });
}

export async function listDevices(req: Request, env: Env): Promise<Response> {
  const { userId, deviceId } = await requireUser(req, env);
  const rows = await all<{ id: string; name: string; created_at: number; last_seen: number }>(
    env.DB,
    "SELECT id, name, created_at, last_seen FROM devices WHERE user_id = ? AND revoked = 0 ORDER BY created_at",
    userId,
  );
  return json(rows.map((d) => ({ ...d, current: d.id === deviceId })));
}

export async function removeDevice(req: Request, env: Env, id: string): Promise<Response> {
  const { userId } = await requireUser(req, env);
  // Someone else's device id (or a made-up one) matches no row here: same answer as "not found".
  const changed = await exec(
    env.DB,
    "UPDATE devices SET revoked = 1, revoked_reason = 'removed' WHERE id = ? AND user_id = ? AND revoked = 0",
    id, userId,
  );
  if (changed !== 1) throw notFound();
  return noContent();
}

export async function deleteAccount(req: Request, env: Env): Promise<Response> {
  const { userId } = await requireUser(req, env);
  await env.DB.batch([
    stmt(env.DB, "DELETE FROM api_keys WHERE user_id = ?", userId),
    stmt(env.DB, "DELETE FROM settings WHERE user_id = ?", userId),
    stmt(env.DB, "DELETE FROM items WHERE user_id = ?", userId),
    stmt(env.DB, "DELETE FROM stats WHERE user_id = ?", userId),
    stmt(env.DB, "DELETE FROM login_requests WHERE user_id = ?", userId),
    stmt(env.DB, "DELETE FROM devices WHERE user_id = ?", userId),
    stmt(env.DB, "DELETE FROM users WHERE id = ?", userId),
  ]);
  return noContent();
}

// ---------- API keys ----------

export async function getKeys(req: Request, env: Env): Promise<Response> {
  const { userId } = await requireUser(req, env);
  const rows = await all<{ provider: string; ciphertext: string; iv: string; key_version: number; updated_at: number }>(
    env.DB,
    "SELECT provider, ciphertext, iv, key_version, updated_at FROM api_keys WHERE user_id = ?",
    userId,
  );
  const out: Record<string, { key: string; updated_at: number } | null> = {};
  for (const p of PROVIDERS) out[p] = null;
  for (const row of rows) {
    if (!isProvider(row.provider)) continue;
    try {
      out[row.provider] = { key: await decryptApiKey(env.MASTER_KEY, userId, row.provider, row), updated_at: row.updated_at };
    } catch {
      // Unreadable (tampered, moved, or MASTER_KEY changed). Say nothing about the data.
      console.error("api key could not be decrypted for provider", row.provider);
    }
  }
  return json(out);
}

export async function putKey(req: Request, env: Env, provider: string): Promise<Response> {
  const { userId } = await requireUser(req, env);
  if (!isProvider(provider)) {
    throw badRequest(`Unknown provider. Use one of: ${PROVIDERS.join(", ")}.`);
  }
  const body = await readJsonBody(req);
  if (typeof body.key !== "string") throw badRequest("Send the key as text in a field called key.");
  const key = body.key.trim();
  if (key.length > MAX_KEY_LENGTH) throw badRequest("That key is too long to be a real API key.");
  const now = Date.now();

  if (key === "") {
    await exec(env.DB, "DELETE FROM api_keys WHERE user_id = ? AND provider = ?", userId, provider);
    return json({ updated_at: now });
  }
  const enc = await encryptApiKey(env.MASTER_KEY, userId, provider, key);
  await exec(
    env.DB,
    `INSERT INTO api_keys (user_id, provider, ciphertext, iv, key_version, updated_at) VALUES (?, ?, ?, ?, ?, ?)
     ON CONFLICT(user_id, provider) DO UPDATE SET
       ciphertext = excluded.ciphertext, iv = excluded.iv, key_version = excluded.key_version, updated_at = excluded.updated_at`,
    userId, provider, enc.ciphertext, enc.iv, enc.keyVersion, now,
  );
  return json({ updated_at: now });
}

export async function deleteKeys(req: Request, env: Env): Promise<Response> {
  const { userId } = await requireUser(req, env);
  await exec(env.DB, "DELETE FROM api_keys WHERE user_id = ?", userId);
  return noContent();
}

// ---------- settings (last write wins, per section) ----------

async function readSections(env: Env, userId: string) {
  const rows = await all<{ section: string; value: string; updated_at: number }>(
    env.DB,
    "SELECT section, value, updated_at FROM settings WHERE user_id = ?",
    userId,
  );
  const sections: Record<string, { value: unknown; updated_at: number }> = {};
  for (const r of rows) sections[r.section] = { value: JSON.parse(r.value), updated_at: r.updated_at };
  return sections;
}

export async function getSettings(req: Request, env: Env): Promise<Response> {
  const { userId } = await requireUser(req, env);
  return json({ sections: await readSections(env, userId) });
}

export async function putSettings(req: Request, env: Env): Promise<Response> {
  const { userId } = await requireUser(req, env);
  const body = await readJsonBody(req);
  if (!isRecord(body.sections)) throw badRequest("Send the settings as an object called sections.");
  const entries = Object.entries(body.sections);
  if (entries.length > MAX_SETTING_SECTIONS) throw badRequest(`At most ${MAX_SETTING_SECTIONS} sections can be sent at once.`);

  const now = Date.now();
  const rows: { section: string; value: string; updated_at: number }[] = [];
  for (const [name, sec] of entries) {
    if (!SECTION_RE.test(name)) throw badRequest(`"${name.slice(0, 40)}" is not a valid section name.`);
    if (!isRecord(sec) || !("value" in sec)) throw badRequest(`Section "${name}" needs a value and an updated_at.`);
    const value = JSON.stringify(sec.value);
    if (value === undefined) throw badRequest(`Section "${name}" has a value that cannot be saved.`);
    if (value.length > MAX_SECTION_BYTES) throw badRequest(`Section "${name}" is too large.`);
    rows.push({ section: name, value, updated_at: clampFuture(requireInt(sec.updated_at, "updated_at", 0, Number.MAX_SAFE_INTEGER), now) });
  }

  if (rows.length > 0) {
    // One statement for the whole batch (D1 allows few statements per request and 100 bound values each).
    await exec(
      env.DB,
      `INSERT INTO settings (user_id, section, value, updated_at)
       SELECT ?, json_extract(j.value, '$.section'), json_extract(j.value, '$.value'), json_extract(j.value, '$.updated_at')
       FROM json_each(?) AS j WHERE true
       ON CONFLICT(user_id, section) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at
       WHERE excluded.updated_at > settings.updated_at`,
      userId, JSON.stringify(rows),
    );
  }
  return json({ sections: await readSections(env, userId) });
}

// ---------- items (dictionary, snippets) ----------

export async function getItems(req: Request, env: Env): Promise<Response> {
  const { userId } = await requireUser(req, env);
  const sinceRaw = new URL(req.url).searchParams.get("since");
  const since = sinceRaw === null ? 0 : Number(sinceRaw);
  if (!Number.isInteger(since) || since < 0) throw badRequest("since must be a whole number of milliseconds.");

  const now = Date.now();
  // Cursor is the server's clock (server_at), not the client's updated_at. Inclusive (>=) on purpose:
  // merging is idempotent, and it cannot miss a same-millisecond write.
  const rows = await all<{ kind: string; uid: string; data: string | null; updated_at: number; deleted: number; server_at: number }>(
    env.DB,
    "SELECT kind, uid, data, updated_at, deleted, server_at FROM items WHERE user_id = ? AND server_at >= ? ORDER BY server_at, kind, uid",
    userId, since,
  );
  return json({
    items: rows.map((r) => ({
      kind: r.kind, uid: r.uid, data: r.data === null ? null : JSON.parse(r.data),
      updated_at: r.updated_at, deleted: r.deleted === 1, server_at: r.server_at,
    })),
    now,
  });
}

export async function postItems(req: Request, env: Env): Promise<Response> {
  const { userId } = await requireUser(req, env);
  const body = await readJsonBody(req);
  if (!Array.isArray(body.items)) throw badRequest("Send the items as a list called items.");
  if (body.items.length > MAX_ITEMS_PER_CALL) throw badRequest(`At most ${MAX_ITEMS_PER_CALL} items can be sent at once.`);

  const now = Date.now();
  const rows: { kind: string; uid: string; data: string | null; updated_at: number; deleted: number }[] = [];
  for (const it of body.items) {
    if (!isRecord(it)) throw badRequest("Every item must be an object.");
    if (!isKind(it.kind)) throw badRequest(`Item kind must be one of: ${ITEM_KINDS.join(", ")}.`);
    if (typeof it.uid !== "string" || !ID_RE.test(it.uid)) throw badRequest("Every item needs a uid (letters, digits, dashes; up to 100 characters).");
    const data = it.data === undefined || it.data === null ? null : JSON.stringify(it.data);
    if (data !== null && data.length > MAX_ITEM_DATA_BYTES) throw badRequest("One of the items is too large.");
    if (it.deleted !== undefined && typeof it.deleted !== "boolean" && it.deleted !== 0 && it.deleted !== 1) {
      throw badRequest("deleted must be true or false.");
    }
    rows.push({
      kind: it.kind,
      uid: it.uid,
      data,
      updated_at: clampFuture(requireInt(it.updated_at, "updated_at", 0, Number.MAX_SAFE_INTEGER), now),
      deleted: it.deleted === true || it.deleted === 1 ? 1 : 0,
    });
  }

  if (rows.length > 0) {
    await exec(
      env.DB,
      `INSERT INTO items (user_id, kind, uid, data, updated_at, deleted, server_at)
       SELECT ?, json_extract(j.value, '$.kind'), json_extract(j.value, '$.uid'), json_extract(j.value, '$.data'),
              json_extract(j.value, '$.updated_at'), json_extract(j.value, '$.deleted'), ?
       FROM json_each(?) AS j WHERE true
       ON CONFLICT(user_id, kind, uid) DO UPDATE SET
         data = excluded.data, updated_at = excluded.updated_at, deleted = excluded.deleted, server_at = excluded.server_at
       WHERE excluded.updated_at > items.updated_at`,
      userId, now, JSON.stringify(rows),
    );
  }
  return json({ received: rows.length, now });
}

// ---------- stats (numbers only, deduped by uid) ----------

export async function postStats(req: Request, env: Env): Promise<Response> {
  const { userId, deviceId } = await requireUser(req, env);
  const body = await readJsonBody(req);
  if (!Array.isArray(body.rows)) throw badRequest("Send the rows as a list called rows.");
  if (body.rows.length > MAX_STATS_PER_CALL) throw badRequest(`At most ${MAX_STATS_PER_CALL} rows can be sent at once.`);

  const rows: { uid: string; created_at: number; words: number; duration_ms: number; app: string; fixes: number }[] = [];
  for (const r of body.rows) {
    if (!isRecord(r)) throw badRequest("Every row must be an object.");
    if (typeof r.uid !== "string" || !ID_RE.test(r.uid)) throw badRequest("Every row needs a uid (letters, digits, dashes; up to 100 characters).");
    if (r.app !== undefined && r.app !== null && typeof r.app !== "string") throw badRequest("app must be text.");
    rows.push({
      uid: r.uid,
      created_at: requireInt(r.created_at, "created_at", 0, Number.MAX_SAFE_INTEGER),
      words: requireInt(r.words, "words", 0, 1_000_000),
      duration_ms: requireInt(r.duration_ms, "duration_ms", 0, 86_400_000),
      app: typeof r.app === "string" ? r.app.slice(0, 200) : "",
      fixes: requireInt(r.fixes ?? 0, "fixes", 0, 1_000_000),
    });
  }

  let inserted = 0;
  if (rows.length > 0) {
    inserted = await exec(
      env.DB,
      `INSERT OR IGNORE INTO stats (user_id, uid, device_id, created_at, words, duration_ms, app, fixes, server_at)
       SELECT ?, json_extract(j.value, '$.uid'), ?, json_extract(j.value, '$.created_at'), json_extract(j.value, '$.words'),
              json_extract(j.value, '$.duration_ms'), json_extract(j.value, '$.app'), json_extract(j.value, '$.fixes'), ?
       FROM json_each(?) AS j WHERE true`,
      userId, deviceId, Date.now(), JSON.stringify(rows),
    );
  }
  return json({ received: rows.length, inserted });
}

export async function getStats(req: Request, env: Env): Promise<Response> {
  const { userId } = await requireUser(req, env);
  const sinceRaw = new URL(req.url).searchParams.get("since");
  const since = sinceRaw === null ? 0 : Number(sinceRaw);
  if (!Number.isInteger(since) || since < 0) throw badRequest("since must be a whole number of milliseconds.");
  const now = Date.now();
  // Server clock cursor, oldest first. Ask for one extra row to know whether the page was capped.
  // To continue, call again with since = the last row's server_at (inclusive, so nothing is missed).
  const found = await all<Record<string, unknown>>(
    env.DB,
    `SELECT uid, device_id, created_at, words, duration_ms, app, fixes, server_at FROM stats
     WHERE user_id = ? AND server_at >= ? ORDER BY server_at, uid LIMIT ?`,
    userId, since, STATS_PAGE + 1,
  );
  const more = found.length > STATS_PAGE;
  const rows = more ? found.slice(0, STATS_PAGE) : found;
  return json(more ? { rows, now, more: true } : { rows, now });
}


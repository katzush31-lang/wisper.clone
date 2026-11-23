export const MAX_BODY_BYTES = 1024 * 1024; // 1 MB

/** An error whose message is safe to show to the user (plain English). */
export class HttpError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

const BASE_HEADERS = { "cache-control": "no-store" };

export function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { ...BASE_HEADERS, "content-type": "application/json; charset=utf-8" },
  });
}

export function noContent(): Response {
  return new Response(null, { status: 204, headers: BASE_HEADERS });
}

export function redirect(location: string): Response {
  return new Response(null, { status: 302, headers: { ...BASE_HEADERS, location } });
}

/** A tiny static page shown in the browser tab during Google sign-in. */
export function htmlPage(message: string, status = 200): Response {
  const body = `<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Wisper</title><body style="font-family:system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1rem"><h1>Wisper</h1><p>${message}</p></body>`;
  return new Response(body, {
    status,
    headers: { ...BASE_HEADERS, "content-type": "text/html; charset=utf-8" },
  });
}

export function errorResponse(e: HttpError): Response {
  return json({ error: e.code, message: e.message }, e.status);
}

/** Reads a JSON object body, refusing anything over 1 MB (even with no Content-Length). */
export async function readJsonBody(req: Request): Promise<Record<string, unknown>> {
  const declared = Number(req.headers.get("content-length") ?? "0");
  if (declared > MAX_BODY_BYTES) throw tooLarge();
  if (!req.body) throw new HttpError(400, "bad_json", "This request needs a JSON body.");

  const reader = req.body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > MAX_BODY_BYTES) {
      await reader.cancel();
      throw tooLarge();
    }
    chunks.push(value);
  }
  const merged = new Uint8Array(total);
  let offset = 0;
  for (const c of chunks) {
    merged.set(c, offset);
    offset += c.byteLength;
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(new TextDecoder().decode(merged));
  } catch {
    throw new HttpError(400, "bad_json", "The request was not valid JSON.");
  }
  if (!isRecord(parsed)) {
    throw new HttpError(400, "bad_json", "The request body must be a JSON object.");
  }
  return parsed;
}

function tooLarge(): HttpError {
  return new HttpError(413, "too_large", "That request is too big (the limit is 1 MB).");
}

export function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

export function badRequest(message: string): HttpError {
  return new HttpError(400, "bad_request", message);
}

/** A whole number within [min, max], else a plain-English 400. */
export function requireInt(v: unknown, what: string, min: number, max: number): number {
  if (typeof v !== "number" || !Number.isInteger(v) || v < min || v > max) {
    throw badRequest(`${what} must be a whole number between ${min} and ${max}.`);
  }
  return v;
}

export function requireString(v: unknown, what: string, maxLen: number, minLen = 1): string {
  if (typeof v !== "string" || v.length < minLen || v.length > maxLen) {
    throw badRequest(`${what} must be text of ${minLen} to ${maxLen} characters.`);
  }
  return v;
}

// Compact HS256 tokens (JWT format). Access tokens live 15 minutes.
import { b64urlDecode, b64urlEncode, fromUtf8, hmacKey, utf8 } from "./crypto";

export const ACCESS_TOKEN_TTL_SECONDS = 15 * 60;

export interface AccessClaims {
  sub: string; // user id
  did: string; // device id
  iat: number; // seconds
  exp: number; // seconds
}

const HEADER = b64urlEncode(utf8(JSON.stringify({ alg: "HS256", typ: "JWT" })));

export async function signAccessToken(
  tokenSecret: string,
  userId: string,
  deviceId: string,
  nowSeconds = Math.floor(Date.now() / 1000),
  ttlSeconds = ACCESS_TOKEN_TTL_SECONDS,
): Promise<string> {
  const claims: AccessClaims = { sub: userId, did: deviceId, iat: nowSeconds, exp: nowSeconds + ttlSeconds };
  const body = `${HEADER}.${b64urlEncode(utf8(JSON.stringify(claims)))}`;
  const sig = await crypto.subtle.sign("HMAC", await hmacKey(tokenSecret, "sign"), utf8(body));
  return `${body}.${b64urlEncode(new Uint8Array(sig))}`;
}

/** Returns the claims, or null for anything wrong (bad format, signature, algorithm, expiry). */
export async function verifyAccessToken(
  tokenSecret: string,
  token: string,
  nowSeconds = Math.floor(Date.now() / 1000),
): Promise<AccessClaims | null> {
  const parts = token.split(".");
  if (parts.length !== 3) return null;
  const [h, p, s] = parts as [string, string, string];
  try {
    if (h !== HEADER) return null; // only our exact header: rejects alg "none" and friends
    // crypto.subtle.verify compares the signature in constant time.
    const ok = await crypto.subtle.verify(
      "HMAC",
      await hmacKey(tokenSecret, "verify"),
      b64urlDecode(s),
      utf8(`${h}.${p}`),
    );
    if (!ok) return null;
    const c = JSON.parse(fromUtf8(b64urlDecode(p))) as Partial<AccessClaims>;
    if (
      typeof c.sub !== "string" || !c.sub ||
      typeof c.did !== "string" || !c.did ||
      typeof c.iat !== "number" || typeof c.exp !== "number"
    ) return null;
    if (c.exp <= nowSeconds) return null;
    if (c.iat > nowSeconds + 60) return null;
    return c as AccessClaims;
  } catch {
    return null; // misconfigured secret errors are surfaced by sign(), not here
  }
}

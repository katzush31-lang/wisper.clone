import { b64urlDecode, fromUtf8 } from "./crypto";
import { HttpError } from "./http";
import type { Env } from "./types";

export const GOOGLE_AUTH_URL = "https://accounts.google.com/o/oauth2/v2/auth";
export const GOOGLE_TOKEN_URL = "https://oauth2.googleapis.com/token";
const VALID_ISSUERS = ["https://accounts.google.com", "accounts.google.com"];

export interface GoogleIdentity {
  sub: string;
  email: string;
  name: string | null;
}

export function googleAuthUrl(env: Env, redirectUri: string, state: string): string {
  const u = new URL(GOOGLE_AUTH_URL);
  u.searchParams.set("client_id", env.GOOGLE_CLIENT_ID);
  u.searchParams.set("redirect_uri", redirectUri);
  u.searchParams.set("response_type", "code");
  u.searchParams.set("scope", "openid email profile");
  u.searchParams.set("state", state);
  u.searchParams.set("prompt", "select_account");
  return u.toString();
}

function googleFailed(): HttpError {
  return new HttpError(502, "google_failed", "Google sign-in did not work. Please try again.");
}

/**
 * Exchanges the authorization code at Google's token endpoint (server to server, with our client
 * secret) and checks the id_token claims.
 *
 * We do NOT verify the id_token's JWT signature: the token comes straight from Google's token
 * endpoint over TLS in response to our own authenticated request (client id + secret), which is
 * the case OpenID Connect Core 3.1.3.7 says may skip signature validation. We still check
 * aud, iss, exp and email_verified.
 */
export async function exchangeCodeForIdentity(
  env: Env,
  fetchFn: typeof fetch,
  tokenUrl: string,
  code: string,
  redirectUri: string,
  nowSeconds = Math.floor(Date.now() / 1000),
): Promise<GoogleIdentity> {
  let res: Response;
  try {
    res = await fetchFn(tokenUrl, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        code,
        client_id: env.GOOGLE_CLIENT_ID,
        client_secret: env.GOOGLE_CLIENT_SECRET,
        redirect_uri: redirectUri,
        grant_type: "authorization_code",
      }).toString(),
      signal: AbortSignal.timeout(8000),
    });
  } catch {
    throw googleFailed();
  }
  if (!res.ok) throw googleFailed(); // never log the body

  let idToken: unknown;
  try {
    idToken = ((await res.json()) as { id_token?: unknown }).id_token;
  } catch {
    throw googleFailed();
  }
  if (typeof idToken !== "string") throw googleFailed();

  let claims: Record<string, unknown>;
  try {
    claims = JSON.parse(fromUtf8(b64urlDecode(idToken.split(".")[1] ?? ""))) as Record<string, unknown>;
  } catch {
    throw googleFailed();
  }

  const aud = claims.aud;
  const audOk = aud === env.GOOGLE_CLIENT_ID || (Array.isArray(aud) && aud.includes(env.GOOGLE_CLIENT_ID));
  const verified = claims.email_verified === true || claims.email_verified === "true";
  if (
    !audOk ||
    typeof claims.iss !== "string" || !VALID_ISSUERS.includes(claims.iss) ||
    typeof claims.exp !== "number" || claims.exp <= nowSeconds ||
    !verified ||
    typeof claims.sub !== "string" || !claims.sub ||
    typeof claims.email !== "string" || !claims.email
  ) {
    throw new HttpError(403, "google_rejected", "Google did not confirm a verified email address for this account.");
  }

  return {
    sub: claims.sub,
    email: claims.email,
    name: typeof claims.name === "string" && claims.name ? claims.name.slice(0, 200) : null,
  };
}

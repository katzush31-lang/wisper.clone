import { HttpError, revokedError } from "./http";
import { one } from "./db";
import { verifyAccessToken } from "./tokens";
import type { AuthedUser, Env } from "./types";

function unauthorized(): HttpError {
  return new HttpError(401, "invalid", "Please sign in again.");
}

/**
 * THE only way a /v1 handler learns who the caller is. The user id comes from the signed token
 * (never from the URL, query or body), and the device in the token must still exist, belong to
 * that user and not be revoked.
 */
export async function requireUser(req: Request, env: Env): Promise<AuthedUser> {
  const header = req.headers.get("authorization") ?? "";
  const match = /^Bearer ([A-Za-z0-9._-]+)$/.exec(header);
  if (!match) throw unauthorized();

  const claims = await verifyAccessToken(env.TOKEN_SECRET, match[1]!);
  if (!claims) throw unauthorized();

  const device = await one<{ revoked: number; revoked_reason: string | null }>(
    env.DB,
    "SELECT revoked, revoked_reason FROM devices WHERE id = ? AND user_id = ?",
    claims.did,
    claims.sub,
  );
  // A device that no longer exists (account deleted elsewhere) counts as removed.
  if (!device) throw revokedError("removed");
  if (device.revoked) throw revokedError(device.revoked_reason);

  return { userId: claims.sub, deviceId: claims.did };
}

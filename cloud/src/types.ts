/** Bindings, secrets and vars of the Worker. See wrangler.jsonc and README.md. */
export interface Env {
  DB: D1Database;
  /** Workers rate-limiting binding for /auth/*. Optional so tests can omit it. */
  AUTH_LIMITER?: { limit(options: { key: string }): Promise<{ success: boolean }> };
  /** Secret: 32 random bytes, base64. Source of every per-user key-encryption key. */
  MASTER_KEY: string;
  /** Secret: random bytes (>= 32), base64. Signs access tokens. */
  TOKEN_SECRET: string;
  /** Secret: Google OAuth client secret. */
  GOOGLE_CLIENT_SECRET: string;
  /** Var: Google OAuth client id (public). */
  GOOGLE_CLIENT_ID: string;
}

/** Things the tests replace. Production uses the defaults. */
export interface Deps {
  fetch?: typeof fetch;
  googleTokenUrl?: string;
}

export interface AuthedUser {
  userId: string;
  deviceId: string;
}

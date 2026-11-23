import { env } from "cloudflare:workers";
import { b64urlEncode, sha256B64url, utf8 } from "../src/crypto";
import { createHandler } from "../src/index";
import type { Env } from "../src/types";

export const GOOGLE_TOKEN_URL = "https://google.test/token";
export const PORT = 53682;

export function testEnv(over: Partial<Env> = {}): Env {
  return {
    DB: env.DB,
    MASTER_KEY: env.MASTER_KEY,
    TOKEN_SECRET: env.TOKEN_SECRET,
    GOOGLE_CLIENT_SECRET: env.GOOGLE_CLIENT_SECRET,
    GOOGLE_CLIENT_ID: env.GOOGLE_CLIENT_ID,
    ...over,
  };
}

/** Stands in for Google's token endpoint. */
export class FakeGoogle {
  private codes = new Map<string, Record<string, unknown>>();
  requests: URLSearchParams[] = [];
  fail = false;

  /** Registers a Google authorization code for these claims and returns it. */
  issue(claims: Partial<Record<string, unknown>> & { sub: string; email: string }): string {
    const code = `g-${crypto.randomUUID()}`;
    this.codes.set(code, {
      iss: "https://accounts.google.com",
      aud: env.GOOGLE_CLIENT_ID,
      exp: Math.floor(Date.now() / 1000) + 3600,
      email_verified: true,
      name: "Test Person",
      ...claims,
    });
    return code;
  }

  fetch: typeof fetch = async (input, init) => {
    expectUrl(input, GOOGLE_TOKEN_URL);
    const form = new URLSearchParams(String(init?.body));
    this.requests.push(form);
    const claims = this.codes.get(form.get("code") ?? "");
    if (
      this.fail || !claims ||
      form.get("client_secret") !== env.GOOGLE_CLIENT_SECRET ||
      form.get("client_id") !== env.GOOGLE_CLIENT_ID ||
      form.get("grant_type") !== "authorization_code"
    ) {
      return new Response(JSON.stringify({ error: "invalid_grant" }), { status: 400 });
    }
    this.codes.delete(form.get("code")!); // Google codes are single use too
    const part = (o: unknown) => b64urlEncode(utf8(JSON.stringify(o)));
    const idToken = `${part({ alg: "RS256" })}.${part(claims)}.fake-signature`;
    return new Response(JSON.stringify({ id_token: idToken, access_token: "unused" }));
  };
}

function expectUrl(input: RequestInfo | URL, expected: string) {
  const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
  if (url !== expected) throw new Error(`unexpected outbound fetch to ${url}`);
}

export interface Session {
  access: string;
  refresh: string;
  deviceId: string;
  userId: string;
  email: string;
}

export class App {
  google = new FakeGoogle();
  env: Env;
  handler: ReturnType<typeof createHandler>;

  constructor(over: Partial<Env> = {}) {
    this.env = testEnv(over);
    this.handler = createHandler({ fetch: this.google.fetch, googleTokenUrl: GOOGLE_TOKEN_URL });
  }

  call(path: string, opts: { method?: string; token?: string; body?: unknown; rawBody?: string; headers?: Record<string, string> } = {}) {
    const headers: Record<string, string> = { ...(opts.headers ?? {}) };
    if (opts.token) headers.authorization = `Bearer ${opts.token}`;
    let body: string | undefined;
    if (opts.rawBody !== undefined) body = opts.rawBody;
    else if (opts.body !== undefined) {
      body = JSON.stringify(opts.body);
      headers["content-type"] = "application/json";
    }
    return this.handler(new Request(`https://api.test${path}`, { method: opts.method ?? (body ? "POST" : "GET"), headers, body }), this.env);
  }

  /** Runs the whole Google + PKCE login for one person and one computer. */
  async signIn(
    who: { sub: string; email: string; name?: string } = { sub: "sub-alice", email: "alice@example.com" },
    deviceName = "Test PC",
  ): Promise<Session> {
    const { verifier, challenge } = await pkce();
    const start = await this.call(`/auth/google/start?challenge=${challenge}&port=${PORT}`);
    const state = new URL(start.headers.get("location")!).searchParams.get("state")!;
    const googleCode = this.google.issue(who);
    const cb = await this.call(`/auth/google/callback?code=${googleCode}&state=${state}`);
    const loginCode = new URL(cb.headers.get("location")!).searchParams.get("code")!;
    const res = await this.call("/auth/token", { body: { code: loginCode, verifier, device_name: deviceName } });
    if (res.status !== 200) throw new Error(`sign-in failed: ${res.status} ${await res.text()}`);
    const j = (await res.json()) as any;
    return { access: j.access_token, refresh: j.refresh_token, deviceId: j.device_id, userId: j.user.id, email: j.user.email };
  }
}

export async function pkce() {
  const verifier = b64urlEncode(crypto.getRandomValues(new Uint8Array(48)));
  return { verifier, challenge: await sha256B64url(verifier) };
}

export async function jsonOf(res: Response): Promise<any> {
  return res.json();
}

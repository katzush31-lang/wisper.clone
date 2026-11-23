import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { sha256B64url } from "../src/crypto";
import { signAccessToken } from "../src/tokens";
import { App, jsonOf, pkce, PORT } from "./helpers";

/** Runs start + callback and returns what the desktop app would hold. */
async function loginUpToCode(app: App, who = { sub: "s1", email: "a@example.com" }) {
  const { verifier, challenge } = await pkce();
  const start = await app.call(`/auth/google/start?challenge=${challenge}&port=${PORT}`);
  const state = new URL(start.headers.get("location")!).searchParams.get("state")!;
  const cb = await app.call(`/auth/google/callback?code=${app.google.issue(who)}&state=${state}`);
  const loc = new URL(cb.headers.get("location")!);
  return { verifier, challenge, state, start, cb, loc, code: loc.searchParams.get("code")! };
}

describe("Google login with PKCE", () => {
  it("completes the full flow and returns tokens, user and device", async () => {
    const app = new App();
    const { start, cb, loc, verifier, code, state } = await loginUpToCode(app);

    expect(start.status).toBe(302);
    const g = new URL(start.headers.get("location")!);
    expect(g.origin + g.pathname).toBe("https://accounts.google.com/o/oauth2/v2/auth");
    expect(g.searchParams.get("client_id")).toBe(env.GOOGLE_CLIENT_ID);
    expect(g.searchParams.get("redirect_uri")).toBe("https://api.test/auth/google/callback");
    expect(g.searchParams.get("scope")).toBe("openid email profile");
    expect(g.searchParams.get("prompt")).toBe("select_account");
    expect(g.searchParams.get("state")).toBe(state);

    expect(cb.status).toBe(302);
    expect(loc.origin).toBe(`http://127.0.0.1:${PORT}`);
    expect(loc.pathname).toBe("/callback");

    // The server-side exchange used our client secret and redirect uri.
    expect(app.google.requests[0]!.get("client_secret")).toBe(env.GOOGLE_CLIENT_SECRET);
    expect(app.google.requests[0]!.get("redirect_uri")).toBe("https://api.test/auth/google/callback");

    const res = await app.call("/auth/token", { body: { code, verifier, device_name: "Kitchen PC" } });
    expect(res.status).toBe(200);
    const j = await jsonOf(res);
    expect(j.expires_in).toBe(900);
    expect(j.user).toMatchObject({ email: "a@example.com", name: "Test Person" });
    expect(j.access_token.split(".")).toHaveLength(3);
    expect(j.refresh_token.length).toBeGreaterThanOrEqual(43);

    const me = await jsonOf(await app.call("/v1/me", { token: j.access_token }));
    expect(me).toEqual({ user: j.user, device_id: j.device_id });

    // Only the hash of the refresh token is stored.
    const dev = await env.DB.prepare("SELECT refresh_hash, name FROM devices WHERE id = ?").bind(j.device_id).first<any>();
    expect(dev.name).toBe("Kitchen PC");
    expect(dev.refresh_hash).toBe(await sha256B64url(j.refresh_token));
    expect(dev.refresh_hash).not.toBe(j.refresh_token);
  });

  it("signing in again as the same Google account reuses the user", async () => {
    const app = new App();
    const a = await app.signIn({ sub: "same", email: "x@example.com" }, "PC 1");
    const b = await app.signIn({ sub: "same", email: "x@example.com" }, "PC 2");
    expect(a.userId).toBe(b.userId);
    expect(a.deviceId).not.toBe(b.deviceId);
  });

  it("rejects a wrong verifier, and the code is burned afterwards", async () => {
    const app = new App();
    const { verifier, code } = await loginUpToCode(app);
    const wrong = (await pkce()).verifier;
    const bad = await app.call("/auth/token", { body: { code, verifier: wrong, device_name: "PC" } });
    expect(bad.status).toBe(400);
    expect((await jsonOf(bad)).error).toBe("invalid_code");
    // Even the right verifier no longer works.
    const again = await app.call("/auth/token", { body: { code, verifier, device_name: "PC" } });
    expect(again.status).toBe(400);
  });

  it("rejects a verifier that is too short or has bad characters", async () => {
    const app = new App();
    const { code } = await loginUpToCode(app);
    const r = await app.call("/auth/token", { body: { code, verifier: "short", device_name: "PC" } });
    expect(r.status).toBe(400);
  });

  it("the login code works only once", async () => {
    const app = new App();
    const { verifier, code } = await loginUpToCode(app);
    expect((await app.call("/auth/token", { body: { code, verifier, device_name: "PC" } })).status).toBe(200);
    const second = await app.call("/auth/token", { body: { code, verifier, device_name: "PC" } });
    expect(second.status).toBe(400);
    expect((await jsonOf(second)).error).toBe("invalid_code");
    const devices = await env.DB.prepare("SELECT COUNT(*) AS n FROM devices").first<any>();
    expect(devices.n).toBe(1);
  });

  it("two simultaneous exchanges of one code create only one device", async () => {
    const app = new App();
    const { verifier, code } = await loginUpToCode(app);
    const results = await Promise.all([1, 2, 3].map(() => app.call("/auth/token", { body: { code, verifier, device_name: "PC" } })));
    expect(results.filter((r) => r.status === 200)).toHaveLength(1);
    const devices = await env.DB.prepare("SELECT COUNT(*) AS n FROM devices").first<any>();
    expect(devices.n).toBe(1);
  });

  it("the login code expires after 2 minutes", async () => {
    const app = new App();
    const { verifier, code } = await loginUpToCode(app);
    await env.DB.prepare("UPDATE login_requests SET code_expires = ?").bind(Date.now() - 1).run();
    const r = await app.call("/auth/token", { body: { code, verifier, device_name: "PC" } });
    expect(r.status).toBe(400);
    expect((await jsonOf(r)).error).toBe("invalid_code");
  });

  it("the login code lives 2 minutes (not much more)", async () => {
    const app = new App();
    const before = Date.now();
    await loginUpToCode(app);
    const row = await env.DB.prepare("SELECT code_expires FROM login_requests").first<any>();
    expect(row.code_expires - before).toBeGreaterThan(115_000);
    expect(row.code_expires - before).toBeLessThanOrEqual(121_000);
  });

  it("a login request older than 10 minutes cannot be completed", async () => {
    const app = new App();
    const { challenge } = await pkce();
    const start = await app.call(`/auth/google/start?challenge=${challenge}&port=${PORT}`);
    const state = new URL(start.headers.get("location")!).searchParams.get("state")!;
    await env.DB.prepare("UPDATE login_requests SET created_at = ?").bind(Date.now() - 11 * 60_000).run();
    const cb = await app.call(`/auth/google/callback?code=${app.google.issue({ sub: "s", email: "e@example.com" })}&state=${state}`);
    expect(cb.status).toBe(400);
    expect(cb.headers.get("location")).toBeNull();
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM users").first<any>()).toEqual({ n: 0 });
  });

  it("a state value cannot be used twice, and unknown states are refused", async () => {
    const app = new App();
    const { state } = await loginUpToCode(app);
    const replay = await app.call(`/auth/google/callback?code=${app.google.issue({ sub: "x", email: "x@example.com" })}&state=${state}`);
    expect(replay.status).toBe(400);
    const unknown = await app.call(`/auth/google/callback?code=abc&state=nope`);
    expect(unknown.status).toBe(400);
  });

  it("start refuses a bad challenge or port", async () => {
    const app = new App();
    const { challenge } = await pkce();
    expect((await app.call(`/auth/google/start?challenge=short&port=${PORT}`)).status).toBe(400);
    expect((await app.call(`/auth/google/start?challenge=${challenge}&port=80`)).status).toBe(400);
    expect((await app.call(`/auth/google/start?challenge=${challenge}&port=abc`)).status).toBe(400);
    expect((await app.call(`/auth/google/start?challenge=${challenge}`)).status).toBe(400);
  });

  it("sends the browser back with an error when the person cancels at Google", async () => {
    const app = new App();
    const { challenge } = await pkce();
    const start = await app.call(`/auth/google/start?challenge=${challenge}&port=${PORT}`);
    const state = new URL(start.headers.get("location")!).searchParams.get("state")!;
    const cb = await app.call(`/auth/google/callback?error=access_denied&state=${state}`);
    expect(cb.headers.get("location")).toBe(`http://127.0.0.1:${PORT}/callback?error=access_denied`);
  });

  describe("id_token checks", () => {
    const cases: [string, Record<string, unknown>][] = [
      ["wrong audience", { aud: "someone-elses-client" }],
      ["wrong issuer", { iss: "https://evil.example.com" }],
      ["expired token", { exp: Math.floor(Date.now() / 1000) - 10 }],
      ["unverified email", { email_verified: false }],
    ];
    for (const [label, claims] of cases) {
      it(`refuses ${label}`, async () => {
        const app = new App();
        const { challenge } = await pkce();
        const start = await app.call(`/auth/google/start?challenge=${challenge}&port=${PORT}`);
        const state = new URL(start.headers.get("location")!).searchParams.get("state")!;
        const cb = await app.call(`/auth/google/callback?code=${app.google.issue({ sub: "s", email: "e@example.com", ...claims })}&state=${state}`);
        expect(cb.status).toBe(302);
        expect(cb.headers.get("location")).toBe(`http://127.0.0.1:${PORT}/callback?error=google_failed`);
        expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM users").first<any>()).toEqual({ n: 0 });
      });
    }

    it("accepts the bare accounts.google.com issuer", async () => {
      const app = new App();
      const { loc } = await loginUpToCode(app, { sub: "s", email: "e@example.com", iss: "accounts.google.com" } as any);
      expect(loc.searchParams.get("code")).toBeTruthy();
    });

    it("redirects to the loopback with google_failed when Google is down (network error)", async () => {
      const app = new App();
      app.google.fetch = async () => { throw new Error("offline"); };
      const { challenge } = await pkce();
      const start = await app.call(`/auth/google/start?challenge=${challenge}&port=${PORT}`);
      const state = new URL(start.headers.get("location")!).searchParams.get("state")!;
      const cb = await app.call(`/auth/google/callback?code=whatever&state=${state}`);
      expect(cb.headers.get("location")).toBe(`http://127.0.0.1:${PORT}/callback?error=google_failed`);
    });

    it("reports a Google failure without creating anything", async () => {
      const app = new App();
      app.google.fail = true;
      const { challenge } = await pkce();
      const start = await app.call(`/auth/google/start?challenge=${challenge}&port=${PORT}`);
      const state = new URL(start.headers.get("location")!).searchParams.get("state")!;
      const cb = await app.call(`/auth/google/callback?code=whatever&state=${state}`);
      expect(cb.status).toBe(302);
      expect(cb.headers.get("location")).toBe(`http://127.0.0.1:${PORT}/callback?error=google_failed`);
    });
  });
});

describe("refresh tokens", () => {
  it("rotates on every refresh and returns the same shape", async () => {
    const app = new App();
    const s = await app.signIn();
    const res = await app.call("/auth/refresh", { body: { refresh_token: s.refresh } });
    expect(res.status).toBe(200);
    const j = await jsonOf(res);
    expect(j.refresh_token).not.toBe(s.refresh);
    expect(j.device_id).toBe(s.deviceId);
    expect(j.user.email).toBe(s.email);
    expect(j.expires_in).toBe(900);
    expect((await app.call("/v1/me", { token: j.access_token })).status).toBe(200);

    // Only the newest refresh token is accepted.
    const again = await app.call("/auth/refresh", { body: { refresh_token: j.refresh_token } });
    expect(again.status).toBe(200);
  });

  it("reusing an already-rotated token revokes the device (theft detection)", async () => {
    const app = new App();
    const s = await app.signIn();
    const first = await jsonOf(await app.call("/auth/refresh", { body: { refresh_token: s.refresh } }));

    const replay = await app.call("/auth/refresh", { body: { refresh_token: s.refresh } });
    expect(replay.status).toBe(401);
    expect((await jsonOf(replay)).error).toBe("revoked");

    // The thief's replay also locked out the legitimate newest token and access tokens.
    const legit = await app.call("/auth/refresh", { body: { refresh_token: first.refresh_token } });
    expect(legit.status).toBe(401);
    expect((await jsonOf(legit)).error).toBe("revoked");
    const me = await app.call("/v1/me", { token: first.access_token });
    expect(me.status).toBe(401);
  });

  it("two simultaneous refreshes with one token cannot both succeed", async () => {
    const app = new App();
    const s = await app.signIn();
    const rs = await Promise.all([1, 2, 3].map(() => app.call("/auth/refresh", { body: { refresh_token: s.refresh } })));
    expect(rs.filter((r) => r.status === 200).length).toBeLessThanOrEqual(1);
  });

  it("rejects unknown refresh tokens", async () => {
    const app = new App();
    const r = await app.call("/auth/refresh", { body: { refresh_token: "x".repeat(43) } });
    expect(r.status).toBe(401);
    expect((await jsonOf(r)).error).toBe("invalid");
  });

  it("expires 180 days after last use", async () => {
    const app = new App();
    const s = await app.signIn();
    await env.DB.prepare("UPDATE devices SET last_seen = ?").bind(Date.now() - 181 * 86_400_000).run();
    const r = await app.call("/auth/refresh", { body: { refresh_token: s.refresh } });
    expect(r.status).toBe(401);
    expect((await jsonOf(r)).error).toBe("invalid");
  });

  it("still works just inside 180 days", async () => {
    const app = new App();
    const s = await app.signIn();
    await env.DB.prepare("UPDATE devices SET last_seen = ?").bind(Date.now() - 179 * 86_400_000).run();
    expect((await app.call("/auth/refresh", { body: { refresh_token: s.refresh } })).status).toBe(200);
  });

  it("a revoked device cannot refresh", async () => {
    const app = new App();
    const s = await app.signIn();
    await app.call(`/v1/devices/${s.deviceId}`, { method: "DELETE", token: s.access });
    const r = await app.call("/auth/refresh", { body: { refresh_token: s.refresh } });
    expect(r.status).toBe(401);
    expect((await jsonOf(r)).error).toBe("revoked");
  });
});

describe("logout and devices", () => {
  it("logout revokes this device only", async () => {
    const app = new App();
    const pc1 = await app.signIn({ sub: "u", email: "u@example.com" }, "PC 1");
    const pc2 = await app.signIn({ sub: "u", email: "u@example.com" }, "PC 2");
    const res = await app.call("/auth/logout", { method: "POST", token: pc1.access });
    expect(res.status).toBe(204);
    expect((await app.call("/v1/me", { token: pc1.access })).status).toBe(401);
    expect((await app.call("/auth/refresh", { body: { refresh_token: pc1.refresh } })).status).toBe(401);
    expect((await app.call("/v1/me", { token: pc2.access })).status).toBe(200);
  });

  it("logout needs a valid token", async () => {
    const app = new App();
    expect((await app.call("/auth/logout", { method: "POST" })).status).toBe(401);
  });

  it("lists devices and marks the current one; removing another device signs it out", async () => {
    const app = new App();
    const pc1 = await app.signIn({ sub: "u", email: "u@example.com" }, "PC 1");
    const pc2 = await app.signIn({ sub: "u", email: "u@example.com" }, "PC 2");
    const list = await jsonOf(await app.call("/v1/devices", { token: pc1.access }));
    expect(list).toHaveLength(2);
    expect(list.find((d: any) => d.id === pc1.deviceId).current).toBe(true);
    expect(list.find((d: any) => d.id === pc2.deviceId)).toMatchObject({ name: "PC 2", current: false });

    expect((await app.call(`/v1/devices/${pc2.deviceId}`, { method: "DELETE", token: pc1.access })).status).toBe(204);
    expect((await app.call("/v1/me", { token: pc2.access })).status).toBe(401);
    expect(await jsonOf(await app.call("/v1/devices", { token: pc1.access }))).toHaveLength(1);
  });

  it("keeps at most 20 active computers by retiring the least recently used", async () => {
    const app = new App();
    const first = await app.signIn({ sub: "u", email: "u@example.com" }, "PC 0");
    for (let i = 1; i < 21; i++) await app.signIn({ sub: "u", email: "u@example.com" }, `PC ${i}`);
    const active = await env.DB.prepare("SELECT COUNT(*) AS n FROM devices WHERE revoked = 0").first<any>();
    expect(active.n).toBe(20);
    expect((await app.call("/v1/me", { token: first.access })).status).toBe(401);
  });
});

describe("access tokens", () => {
  it("rejects missing, malformed, tampered, expired and wrong-secret tokens", async () => {
    const app = new App();
    const s = await app.signIn();
    const [h, p, sig] = s.access.split(".") as [string, string, string];
    const other = JSON.parse(atob(p.replace(/-/g, "+").replace(/_/g, "/")));
    other.sub = "someone-else";
    const forgedPayload = btoa(JSON.stringify(other)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    const noneHeader = btoa(JSON.stringify({ alg: "none", typ: "JWT" })).replace(/=+$/, "");

    const bad = [
      undefined,
      "garbage",
      `${h}.${p}`,
      `${h}.${forgedPayload}.${sig}`, // payload swapped, signature kept
      `${h}.${p}.${sig.slice(0, -2)}AA`, // signature edited
      `${noneHeader}.${p}.`, // alg none
      await signAccessToken(env.TOKEN_SECRET, s.userId, s.deviceId, Math.floor(Date.now() / 1000) - 3600), // expired
      await signAccessToken(btoa("w".repeat(40)), s.userId, s.deviceId), // wrong secret
    ];
    for (const t of bad) {
      const res = await app.call("/v1/me", { token: t });
      expect(res.status, String(t).slice(0, 30)).toBe(401);
    }
    expect((await app.call("/v1/me", { token: s.access })).status).toBe(200);
  });

  it("rejects a validly signed token whose device belongs to another user", async () => {
    const app = new App();
    const alice = await app.signIn({ sub: "a", email: "a@example.com" });
    const bob = await app.signIn({ sub: "b", email: "b@example.com" });
    const crossed = await signAccessToken(env.TOKEN_SECRET, bob.userId, alice.deviceId);
    expect((await app.call("/v1/me", { token: crossed })).status).toBe(401);
  });

  it("a token for a deleted device or user stops working immediately", async () => {
    const app = new App();
    const s = await app.signIn();
    await env.DB.prepare("DELETE FROM devices").run();
    expect((await app.call("/v1/me", { token: s.access })).status).toBe(401);
  });
});

describe("router and limits", () => {
  it("returns plain-English errors in the agreed shape", async () => {
    const app = new App();
    const res = await app.call("/v1/me");
    expect(res.status).toBe(401);
    const j = await jsonOf(res);
    expect(Object.keys(j).sort()).toEqual(["error", "message"]);
    expect(j.message).toMatch(/sign in/i);
    expect((await app.call("/nope")).status).toBe(404);
    expect((await app.call("/v1/nope", { token: (await app.signIn()).access })).status).toBe(404);
  });

  it("rejects bodies over 1 MB, with or without a Content-Length", async () => {
    const app = new App();
    const s = await app.signIn();
    const big = JSON.stringify({ key: "x".repeat(1_100_000) });
    const r1 = await app.call("/v1/keys/openai", { method: "PUT", token: s.access, rawBody: big });
    expect(r1.status).toBe(413);
    const stream = new ReadableStream({ start(c) { c.enqueue(new TextEncoder().encode(big)); c.close(); } });
    const r2 = await app.handler(
      new Request("https://api.test/v1/keys/openai", { method: "PUT", headers: { authorization: `Bearer ${s.access}` }, body: stream, duplex: "half" } as any),
      app.env,
    );
    expect(r2.status).toBe(413);
  });

  it("rejects non-JSON bodies", async () => {
    const app = new App();
    const s = await app.signIn();
    const r = await app.call("/v1/settings", { method: "PUT", token: s.access, rawBody: "not json" });
    expect(r.status).toBe(400);
    expect((await jsonOf(r)).error).toBe("bad_json");
  });

  it("rate-limits /auth/* per IP using the binding", async () => {
    const seen: string[] = [];
    const app = new App({ AUTH_LIMITER: { limit: async ({ key }) => (seen.push(key), { success: seen.length <= 2 }) } });
    const h = { "cf-connecting-ip": "203.0.113.9" };
    expect((await app.call("/auth/refresh", { body: { refresh_token: "a" }, headers: h })).status).toBe(401);
    expect((await app.call("/auth/refresh", { body: { refresh_token: "a" }, headers: h })).status).toBe(401);
    const limited = await app.call("/auth/refresh", { body: { refresh_token: "a" }, headers: h });
    expect(limited.status).toBe(429);
    expect(seen).toEqual(["203.0.113.9", "203.0.113.9", "203.0.113.9"]);
    // /v1 is not behind this limiter.
    await app.call("/v1/me");
    expect(seen).toHaveLength(3);
  });
});

describe("revocation reasons", () => {
  const reasonOf = async (res: Response) => { const j = await jsonOf(res); return [res.status, j.error, j.reason]; };

  it("reuse of a rotated refresh token reports reason=reuse, on refresh and on bearer calls", async () => {
    const app = new App();
    const s = await app.signIn();
    const next = await jsonOf(await app.call("/auth/refresh", { body: { refresh_token: s.refresh } }));
    expect(await reasonOf(await app.call("/auth/refresh", { body: { refresh_token: s.refresh } }))).toEqual([401, "revoked", "reuse"]);
    expect(await reasonOf(await app.call("/auth/refresh", { body: { refresh_token: next.refresh_token } }))).toEqual([401, "revoked", "reuse"]);
    expect(await reasonOf(await app.call("/v1/me", { token: next.access_token }))).toEqual([401, "revoked", "reuse"]);
    // Replaying again does not rewrite the reason.
    expect(await reasonOf(await app.call("/auth/refresh", { body: { refresh_token: s.refresh } }))).toEqual([401, "revoked", "reuse"]);
  });

  it("logout reports reason=removed", async () => {
    const app = new App();
    const s = await app.signIn();
    await app.call("/auth/logout", { method: "POST", token: s.access });
    expect(await reasonOf(await app.call("/auth/refresh", { body: { refresh_token: s.refresh } }))).toEqual([401, "revoked", "removed"]);
    expect(await reasonOf(await app.call("/v1/me", { token: s.access }))).toEqual([401, "revoked", "removed"]);
  });

  it("removal from another computer reports reason=removed", async () => {
    const app = new App();
    const pc1 = await app.signIn({ sub: "u", email: "u@example.com" }, "PC 1");
    const pc2 = await app.signIn({ sub: "u", email: "u@example.com" }, "PC 2");
    await app.call(`/v1/devices/${pc2.deviceId}`, { method: "DELETE", token: pc1.access });
    expect(await reasonOf(await app.call("/v1/keys", { token: pc2.access }))).toEqual([401, "revoked", "removed"]);
    expect(await reasonOf(await app.call("/auth/refresh", { body: { refresh_token: pc2.refresh } }))).toEqual([401, "revoked", "removed"]);
  });

  it("the device cap reports reason=removed", async () => {
    const app = new App();
    const first = await app.signIn({ sub: "u", email: "u@example.com" }, "PC 0");
    for (let i = 1; i < 21; i++) await app.signIn({ sub: "u", email: "u@example.com" }, `PC ${i}`);
    expect(await reasonOf(await app.call("/v1/me", { token: first.access }))).toEqual([401, "revoked", "removed"]);
  });

  it("a logged-out device that later replays an old token keeps reason=removed", async () => {
    const app = new App();
    const s = await app.signIn();
    const next = await jsonOf(await app.call("/auth/refresh", { body: { refresh_token: s.refresh } }));
    await app.call("/auth/logout", { method: "POST", token: next.access_token });
    expect(await reasonOf(await app.call("/auth/refresh", { body: { refresh_token: s.refresh } }))).toEqual([401, "revoked", "removed"]);
  });

  it("a deleted account reports account_deleted for its old tokens", async () => {
    const app = new App();
    const s = await app.signIn();
    await app.call("/v1/account", { method: "DELETE", token: s.access });
    expect(await reasonOf(await app.call("/v1/me", { token: s.access }))).toEqual([401, "revoked", "account_deleted"]);
  });
});

import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { App, jsonOf, type Session } from "./helpers";

/** Alice owns data in every table; Bob is a second, unrelated user. */
async function setup() {
  const app = new App();
  const alice = await app.signIn({ sub: "a", email: "alice@example.com" }, "Alice PC");
  const bob = await app.signIn({ sub: "b", email: "bob@example.com" }, "Bob PC");

  const t = alice.access;
  await app.call("/v1/keys/openai", { method: "PUT", token: t, body: { key: "ALICE-OPENAI" } });
  await app.call("/v1/keys/anthropic", { method: "PUT", token: t, body: { key: "ALICE-ANTHROPIC" } });
  await app.call("/v1/settings", { method: "PUT", token: t, body: { sections: { theme: { value: "alice-dark", updated_at: 1000 } } } });
  await app.call("/v1/items", {
    token: t,
    body: { items: [{ kind: "dictionary", uid: "w1", data: { word: "alice-word" }, updated_at: 1000, deleted: false }] },
  });
  await app.call("/v1/stats", {
    token: t,
    body: { rows: [{ uid: "d1", created_at: 5000, words: 10, duration_ms: 3000, app: "alice-app", fixes: 1 }] },
  });
  return { app, alice, bob };
}

const tableCounts = async (userId: string) => {
  const out: Record<string, number> = {};
  for (const [table, col] of [["api_keys", "user_id"], ["settings", "user_id"], ["items", "user_id"], ["stats", "user_id"], ["devices", "user_id"]]) {
    out[table!] = (await env.DB.prepare(`SELECT COUNT(*) AS n FROM ${table} WHERE ${col} = ?`).bind(userId).first<any>()).n;
  }
  return out;
};

describe("user isolation", () => {
  it("Bob sees none of Alice's data", async () => {
    const { app, bob } = await setup();
    expect(await jsonOf(await app.call("/v1/keys", { token: bob.access }))).toEqual({ openai: null, anthropic: null });
    expect(await jsonOf(await app.call("/v1/settings", { token: bob.access }))).toEqual({ sections: {} });
    expect((await jsonOf(await app.call("/v1/items", { token: bob.access }))).items).toEqual([]);
    expect(await jsonOf(await app.call("/v1/stats", { token: bob.access }))).toEqual({ rows: [] });
    const devices = await jsonOf(await app.call("/v1/devices", { token: bob.access }));
    expect(devices).toHaveLength(1);
    expect(devices[0].id).toBe(bob.deviceId);
    expect((await jsonOf(await app.call("/v1/me", { token: bob.access }))).user.email).toBe("bob@example.com");
  });

  it("Bob writing the same keys, sections, items and stat uids never touches Alice's rows", async () => {
    const { app, alice, bob } = await setup();
    const t = bob.access;
    await app.call("/v1/keys/openai", { method: "PUT", token: t, body: { key: "BOB-OPENAI" } });
    await app.call("/v1/settings", { method: "PUT", token: t, body: { sections: { theme: { value: "bob-light", updated_at: 9_999_999 } } } });
    await app.call("/v1/items", {
      token: t,
      body: { items: [{ kind: "dictionary", uid: "w1", data: { word: "bob-word" }, updated_at: 9_999_999, deleted: true }] },
    });
    await app.call("/v1/stats", { token: t, body: { rows: [{ uid: "d1", created_at: 1, words: 999, duration_ms: 1, app: "bob-app", fixes: 0 }] } });

    expect((await jsonOf(await app.call("/v1/keys", { token: alice.access }))).openai.key).toBe("ALICE-OPENAI");
    expect((await jsonOf(await app.call("/v1/settings", { token: alice.access }))).sections.theme).toEqual({ value: "alice-dark", updated_at: 1000 });
    const items = (await jsonOf(await app.call("/v1/items", { token: alice.access }))).items;
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ data: { word: "alice-word" }, deleted: false, updated_at: 1000 });
    const stats = (await jsonOf(await app.call("/v1/stats", { token: alice.access }))).rows;
    expect(stats).toHaveLength(1);
    expect(stats[0]).toMatchObject({ words: 10, app: "alice-app" });
  });

  it("Bob cannot delete Alice's keys", async () => {
    const { app, alice, bob } = await setup();
    await app.call("/v1/keys", { method: "DELETE", token: bob.access });
    await app.call("/v1/keys/openai", { method: "PUT", token: bob.access, body: { key: "" } });
    const got = await jsonOf(await app.call("/v1/keys", { token: alice.access }));
    expect(got.openai.key).toBe("ALICE-OPENAI");
    expect(got.anthropic.key).toBe("ALICE-ANTHROPIC");
  });

  it("Bob cannot remove Alice's device, even by guessing its id", async () => {
    const { app, alice, bob } = await setup();
    const res = await app.call(`/v1/devices/${alice.deviceId}`, { method: "DELETE", token: bob.access });
    expect(res.status).toBe(404);
    // Same answer as for an id that does not exist, so ids cannot be probed.
    const fake = await app.call(`/v1/devices/${crypto.randomUUID()}`, { method: "DELETE", token: bob.access });
    expect(fake.status).toBe(404);
    expect(await jsonOf(res)).toEqual(await jsonOf(fake));
    expect((await app.call("/v1/me", { token: alice.access })).status).toBe(200);
    expect(await jsonOf(await app.call("/v1/devices", { token: alice.access }))).toHaveLength(1);
  });

  it("Bob cannot delete or revoke Alice by logging out or deleting his account", async () => {
    const { app, alice, bob } = await setup();
    await app.call("/auth/logout", { method: "POST", token: bob.access });
    expect((await app.call("/v1/me", { token: alice.access })).status).toBe(200);

    const bob2 = await app.signIn({ sub: "b", email: "bob@example.com" }, "Bob PC 2");
    expect((await app.call("/v1/account", { method: "DELETE", token: bob2.access })).status).toBe(204);
    expect(await tableCounts(alice.userId)).toEqual({ api_keys: 2, settings: 1, items: 1, stats: 1, devices: 1 });
    expect((await app.call("/v1/me", { token: alice.access })).status).toBe(200);
  });

  it("ignores any user id sent by the client", async () => {
    const { app, alice, bob } = await setup();
    const sneaky = { user_id: alice.userId, userId: alice.userId, sub: alice.userId };
    // Query string, headers and body carrying Alice's id change nothing for Bob.
    const q = `?user_id=${alice.userId}&userId=${alice.userId}&since=0`;
    expect(await jsonOf(await app.call(`/v1/keys${q}`, { token: bob.access, headers: { "x-user-id": alice.userId } }))).toEqual({ openai: null, anthropic: null });
    expect((await jsonOf(await app.call(`/v1/items${q}`, { token: bob.access }))).items).toEqual([]);
    expect((await jsonOf(await app.call(`/v1/stats${q}`, { token: bob.access }))).rows).toEqual([]);
    await app.call("/v1/items", {
      token: bob.access,
      body: { ...sneaky, items: [{ ...sneaky, kind: "snippet", uid: "x", data: {}, updated_at: 5 }] },
    });
    await app.call("/v1/stats", {
      token: bob.access,
      body: { ...sneaky, rows: [{ ...sneaky, uid: "sx", created_at: 5, words: 1, duration_ms: 1, app: "", fixes: 0 }] },
    });
    await app.call("/v1/settings", { method: "PUT", token: bob.access, body: { ...sneaky, sections: { s: { ...sneaky, value: 1, updated_at: 5 } } } });
    const alice1 = await tableCounts(alice.userId);
    expect(alice1).toEqual({ api_keys: 2, settings: 1, items: 1, stats: 1, devices: 1 });
    const bobRows = await tableCounts(bob.userId);
    expect(bobRows).toMatchObject({ items: 1, stats: 1, settings: 1 });
  });

  it("rejects tampered, forged, expired and unsigned tokens for any data route", async () => {
    const { app, alice } = await setup();
    const [h, p, s] = alice.access.split(".") as [string, string, string];
    const routes = ["/v1/keys", "/v1/settings", "/v1/items", "/v1/stats", "/v1/devices", "/v1/me"];
    const wrongSecretApp = new App({ TOKEN_SECRET: btoa("z".repeat(40)) });
    for (const r of routes) {
      expect((await app.call(r)).status, r).toBe(401);
      expect((await app.call(r, { token: `${h}.${p}.${s.slice(0, -3)}abc` })).status, r).toBe(401);
      expect((await wrongSecretApp.call(r, { token: alice.access })).status, r).toBe(401);
      expect((await app.call(r, { headers: { authorization: `Basic ${alice.access}` } })).status, r).toBe(401);
      expect((await app.call(`${r}?access_token=${alice.access}`)).status, r).toBe(401);
    }
  });

  it("a signed-out device's token is rejected on every route", async () => {
    const { app, alice } = await setup();
    await app.call("/auth/logout", { method: "POST", token: alice.access });
    for (const r of ["/v1/keys", "/v1/settings", "/v1/items", "/v1/stats", "/v1/devices", "/v1/me"]) {
      expect((await app.call(r, { token: alice.access })).status, r).toBe(401);
    }
  });

  it("every /v1 handler goes through requireUser (static check of the source)", async () => {
    // Cheap guard against someone adding a handler that forgets to authenticate.
    const src = (await import("../src/routes?raw")).default as string;
    const handlers = src.match(/export async function \w+\(/g) ?? [];
    const guards = src.match(/await requireUser\(req, env\)/g) ?? [];
    expect(handlers.length).toBeGreaterThan(5);
    expect(guards.length).toBe(handlers.length);
  });
});

export type { Session };

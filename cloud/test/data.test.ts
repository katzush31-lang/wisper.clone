import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { App, jsonOf } from "./helpers";

const put = (app: App, token: string, sections: Record<string, unknown>) =>
  app.call("/v1/settings", { method: "PUT", token, body: { sections } });

describe("settings (last write wins)", () => {
  it("keeps the newer updated_at per section and returns the merged result", async () => {
    const app = new App();
    const s = await app.signIn();
    const r1 = await put(app, s.access, {
      theme: { value: "dark", updated_at: 2000 },
      hotkeys: { value: { talk: "Ctrl+Space" }, updated_at: 2000 },
    });
    expect(r1.status).toBe(200);

    const r2 = await jsonOf(
      await put(app, s.access, {
        theme: { value: "light", updated_at: 1000 }, // older: ignored
        hotkeys: { value: { talk: "F9" }, updated_at: 3000 }, // newer: wins
        sounds: { value: false, updated_at: 500 }, // new section
      }),
    );
    expect(r2.sections).toEqual({
      theme: { value: "dark", updated_at: 2000 },
      hotkeys: { value: { talk: "F9" }, updated_at: 3000 },
      sounds: { value: false, updated_at: 500 },
    });
    expect((await jsonOf(await app.call("/v1/settings", { token: s.access }))).sections).toEqual(r2.sections);
  });

  it("an equal timestamp keeps what is stored; any JSON value type survives", async () => {
    const app = new App();
    const s = await app.signIn();
    await put(app, s.access, { a: { value: null, updated_at: 10 }, b: { value: [1, "x", { y: true }], updated_at: 10 }, c: { value: "text", updated_at: 10 } });
    const r = await jsonOf(await put(app, s.access, { c: { value: "other", updated_at: 10 } }));
    expect(r.sections.c.value).toBe("text");
    expect(r.sections.a.value).toBeNull();
    expect(r.sections.b.value).toEqual([1, "x", { y: true }]);
  });

  it("clamps a timestamp from a clock that is far in the future", async () => {
    const app = new App();
    const s = await app.signIn();
    const r = await jsonOf(await put(app, s.access, { theme: { value: "x", updated_at: Date.now() + 10 * 86_400_000 } }));
    expect(r.sections.theme.updated_at).toBeLessThan(Date.now() + 11 * 60_000);
  });

  it("validates sections", async () => {
    const app = new App();
    const s = await app.signIn();
    for (const sections of [
      { "bad name!": { value: 1, updated_at: 1 } },
      { x: { value: 1 } },
      { x: { value: 1, updated_at: "now" } },
      { x: { value: 1, updated_at: -5 } },
      { x: { updated_at: 1 } },
      { x: "nope" },
      Object.fromEntries(Array.from({ length: 65 }, (_, i) => [`s${i}`, { value: 1, updated_at: 1 }])),
    ]) {
      expect((await put(app, s.access, sections)).status, JSON.stringify(sections).slice(0, 40)).toBe(400);
    }
    expect((await app.call("/v1/settings", { method: "PUT", token: s.access, body: { sections: [] } })).status).toBe(400);
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM settings").first<any>()).toEqual({ n: 0 });
  });
});

describe("items (dictionary, snippets)", () => {
  const item = (over: Record<string, unknown> = {}) => ({ kind: "dictionary", uid: "u1", data: { word: "zechus" }, updated_at: 1000, deleted: false, ...over });

  it("upserts only when the incoming updated_at is newer", async () => {
    const app = new App();
    const s = await app.signIn();
    await app.call("/v1/items", { token: s.access, body: { items: [item()] } });
    await app.call("/v1/items", { token: s.access, body: { items: [item({ data: { word: "stale" }, updated_at: 900 })] } });
    await app.call("/v1/items", { token: s.access, body: { items: [item({ data: { word: "same-time" }, updated_at: 1000 })] } });
    let items = (await jsonOf(await app.call("/v1/items", { token: s.access }))).items;
    expect(items).toEqual([{ kind: "dictionary", uid: "u1", data: { word: "zechus" }, updated_at: 1000, deleted: false }]);

    await app.call("/v1/items", { token: s.access, body: { items: [item({ data: { word: "Shabbos" }, updated_at: 2000 })] } });
    items = (await jsonOf(await app.call("/v1/items", { token: s.access }))).items;
    expect(items[0]).toMatchObject({ data: { word: "Shabbos" }, updated_at: 2000 });
  });

  it("a newer delete wins, and a stale edit cannot resurrect it", async () => {
    const app = new App();
    const s = await app.signIn();
    await app.call("/v1/items", { token: s.access, body: { items: [item()] } });
    await app.call("/v1/items", { token: s.access, body: { items: [item({ updated_at: 3000, deleted: true })] } });
    await app.call("/v1/items", { token: s.access, body: { items: [item({ data: { word: "back" }, updated_at: 2000, deleted: false })] } });
    const items = (await jsonOf(await app.call("/v1/items", { token: s.access }))).items;
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ deleted: true, updated_at: 3000 });
  });

  it("keeps kinds separate, and handles duplicates inside one call (newest wins)", async () => {
    const app = new App();
    const s = await app.signIn();
    const res = await app.call("/v1/items", {
      token: s.access,
      body: {
        items: [
          item({ kind: "snippet", data: { trigger: "addr" } }),
          item({ updated_at: 10, data: { word: "old" } }),
          item({ updated_at: 30, data: { word: "newest" } }),
          item({ updated_at: 20, data: { word: "middle" } }),
        ],
      },
    });
    expect(res.status).toBe(200);
    const items = (await jsonOf(await app.call("/v1/items", { token: s.access }))).items;
    expect(items).toHaveLength(2);
    expect(items.find((i: any) => i.kind === "dictionary").data.word).toBe("newest");
    expect(items.find((i: any) => i.kind === "snippet").data.trigger).toBe("addr");
  });

  it("returns only items since a time, plus the server clock", async () => {
    const app = new App();
    const s = await app.signIn();
    await app.call("/v1/items", { token: s.access, body: { items: [item({ uid: "a", updated_at: 100 }), item({ uid: "b", updated_at: 200 }), item({ uid: "c", updated_at: 300 })] } });
    const r = await jsonOf(await app.call("/v1/items?since=200", { token: s.access }));
    expect(r.items.map((i: any) => i.uid)).toEqual(["b", "c"]);
    expect(Math.abs(r.now - Date.now())).toBeLessThan(5000);
    expect((await app.call("/v1/items?since=abc", { token: s.access })).status).toBe(400);
    expect((await app.call("/v1/items?since=-1", { token: s.access })).status).toBe(400);
  });

  it("validates kinds, ids, sizes and the 500-item cap", async () => {
    const app = new App();
    const s = await app.signIn();
    const bad: unknown[] = [
      [item({ kind: "history" })],
      [item({ kind: "settings" })],
      [item({ uid: "" })],
      [item({ uid: "x".repeat(101) })],
      [item({ uid: "has space" })],
      [item({ updated_at: "now" })],
      [item({ deleted: "yes" })],
      [item({ data: { big: "x".repeat(70_000) } })],
      ["not an object"],
      Array.from({ length: 501 }, (_, i) => item({ uid: `u${i}` })),
    ];
    for (const items of bad) {
      expect((await app.call("/v1/items", { token: s.access, body: { items } })).status, JSON.stringify(items).slice(0, 60)).toBe(400);
    }
    expect((await app.call("/v1/items", { token: s.access, body: {} })).status).toBe(400);
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM items").first<any>()).toEqual({ n: 0 });

    const ok = await app.call("/v1/items", { token: s.access, body: { items: Array.from({ length: 500 }, (_, i) => item({ uid: `u${i}` })) } });
    expect(ok.status).toBe(200);
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM items").first<any>()).toEqual({ n: 500 });
  });
});

describe("stats", () => {
  const row = (over: Record<string, unknown> = {}) => ({ uid: "d1", created_at: 5000, words: 42, duration_ms: 12_000, app: "Notepad", fixes: 2, ...over });

  it("inserts rows and ignores a repeated uid (keeps the first)", async () => {
    const app = new App();
    const s = await app.signIn();
    const r1 = await jsonOf(await app.call("/v1/stats", { token: s.access, body: { rows: [row(), row({ uid: "d2", created_at: 6000 })] } }));
    expect(r1).toEqual({ received: 2, inserted: 2 });
    const r2 = await jsonOf(await app.call("/v1/stats", { token: s.access, body: { rows: [row({ words: 999 }), row({ uid: "d3", created_at: 7000 }), row({ uid: "d3", created_at: 7000, words: 1 })] } }));
    expect(r2).toEqual({ received: 3, inserted: 1 });

    const rows = (await jsonOf(await app.call("/v1/stats", { token: s.access }))).rows;
    expect(rows.map((r: any) => r.uid)).toEqual(["d1", "d2", "d3"]);
    expect(rows[0]).toEqual({ uid: "d1", device_id: s.deviceId, created_at: 5000, words: 42, duration_ms: 12_000, app: "Notepad", fixes: 2 });
    expect(rows[2].words).toBe(42); // the first d3 row in the batch is kept
  });

  it("takes device_id from the token and lets a second device see the first one's rows", async () => {
    const app = new App();
    const pc1 = await app.signIn({ sub: "u", email: "u@example.com" }, "PC 1");
    const pc2 = await app.signIn({ sub: "u", email: "u@example.com" }, "PC 2");
    await app.call("/v1/stats", { token: pc1.access, body: { rows: [row({ device_id: "spoofed" })] } });
    const rows = (await jsonOf(await app.call("/v1/stats", { token: pc2.access }))).rows;
    expect(rows).toHaveLength(1);
    expect(rows[0].device_id).toBe(pc1.deviceId);
  });

  it("filters by since (created_at)", async () => {
    const app = new App();
    const s = await app.signIn();
    await app.call("/v1/stats", { token: s.access, body: { rows: [row({ uid: "a", created_at: 100 }), row({ uid: "b", created_at: 200 })] } });
    const rows = (await jsonOf(await app.call("/v1/stats?since=150", { token: s.access }))).rows;
    expect(rows.map((r: any) => r.uid)).toEqual(["b"]);
    expect((await app.call("/v1/stats?since=x", { token: s.access })).status).toBe(400);
  });

  it("stores numbers and the app name only, and validates input and the 1000-row cap", async () => {
    const app = new App();
    const s = await app.signIn();
    const bad: unknown[][] = [
      [row({ uid: "" })],
      [row({ words: -1 })],
      [row({ words: 1.5 })],
      [row({ words: "10" })],
      [row({ duration_ms: 10 ** 12 })],
      [row({ created_at: null })],
      [row({ app: 5 })],
      Array.from({ length: 1001 }, (_, i) => row({ uid: `u${i}` })),
    ];
    for (const rows of bad) {
      expect((await app.call("/v1/stats", { token: s.access, body: { rows } })).status, JSON.stringify(rows).slice(0, 60)).toBe(400);
    }
    const ok = await app.call("/v1/stats", { token: s.access, body: { rows: Array.from({ length: 1000 }, (_, i) => row({ uid: `u${i}`, created_at: i })) } });
    expect(ok.status).toBe(200);
    expect((await jsonOf(ok)).inserted).toBe(1000);
    // an app name is cut to a sane length
    await app.call("/v1/stats", { token: s.access, body: { rows: [row({ uid: "long", app: "a".repeat(500) })] } });
    const long = await env.DB.prepare("SELECT app FROM stats WHERE uid = 'long'").first<any>();
    expect(long.app).toHaveLength(200);
  });
});

describe("delete account", () => {
  it("removes every row for the user, leaves other users alone, and kills the tokens", async () => {
    const app = new App();
    const alice = await app.signIn({ sub: "a", email: "a@example.com" }, "A1");
    const alice2 = await app.signIn({ sub: "a", email: "a@example.com" }, "A2");
    const bob = await app.signIn({ sub: "b", email: "b@example.com" });

    for (const [s, tag] of [[alice, "a"], [bob, "b"]] as const) {
      await app.call("/v1/keys/openai", { method: "PUT", token: s.access, body: { key: `key-${tag}` } });
      await app.call("/v1/keys/anthropic", { method: "PUT", token: s.access, body: { key: `key-${tag}2` } });
      await put(app, s.access, { theme: { value: tag, updated_at: 1 }, hotkeys: { value: tag, updated_at: 1 } });
      await app.call("/v1/items", { token: s.access, body: { items: [{ kind: "dictionary", uid: "w", data: {}, updated_at: 1 }, { kind: "snippet", uid: "s", data: {}, updated_at: 1 }] } });
      await app.call("/v1/stats", { token: s.access, body: { rows: [{ uid: "x", created_at: 1, words: 1, duration_ms: 1, app: "", fixes: 0 }] } });
    }

    const res = await app.call("/v1/account", { method: "DELETE", token: alice.access });
    expect(res.status).toBe(204);

    const count = async (table: string, col: string, id: string) =>
      (await env.DB.prepare(`SELECT COUNT(*) AS n FROM ${table} WHERE ${col} = ?`).bind(id).first<any>()).n;
    for (const [table, col] of [["users", "id"], ["devices", "user_id"], ["login_requests", "user_id"], ["api_keys", "user_id"], ["settings", "user_id"], ["items", "user_id"], ["stats", "user_id"]] as const) {
      expect(await count(table, col, alice.userId), table).toBe(0);
      expect(await count(table, col, bob.userId), `bob ${table}`).toBeGreaterThanOrEqual(1);
    }
    expect(await count("api_keys", "user_id", bob.userId)).toBe(2);

    expect((await app.call("/v1/me", { token: alice.access })).status).toBe(401);
    expect((await app.call("/v1/me", { token: alice2.access })).status).toBe(401);
    expect((await app.call("/auth/refresh", { body: { refresh_token: alice.refresh } })).status).toBe(401);
    expect((await app.call("/v1/me", { token: bob.access })).status).toBe(200);
  });

  it("signing in again after deleting creates a fresh, empty account", async () => {
    const app = new App();
    const first = await app.signIn({ sub: "a", email: "a@example.com" });
    await app.call("/v1/keys/openai", { method: "PUT", token: first.access, body: { key: "old" } });
    await app.call("/v1/account", { method: "DELETE", token: first.access });
    const again = await app.signIn({ sub: "a", email: "a@example.com" });
    expect(again.userId).not.toBe(first.userId);
    expect(await jsonOf(await app.call("/v1/keys", { token: again.access }))).toEqual({ openai: null, anthropic: null });
  });
});

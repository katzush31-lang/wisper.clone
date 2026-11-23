import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { decryptApiKey, encryptApiKey } from "../src/crypto";
import { App, jsonOf } from "./helpers";

const SECRET_KEY = "sk-test-1234567890-very-secret";

async function rowOf(userId: string, provider: string) {
  return env.DB.prepare("SELECT * FROM api_keys WHERE user_id = ? AND provider = ?").bind(userId, provider).first<any>();
}

describe("API keys", () => {
  it("round-trips a key through PUT and GET", async () => {
    const app = new App();
    const s = await app.signIn();
    const put = await app.call("/v1/keys/openai", { method: "PUT", token: s.access, body: { key: SECRET_KEY } });
    expect(put.status).toBe(200);
    const { updated_at } = await jsonOf(put);
    expect(typeof updated_at).toBe("number");

    const got = await jsonOf(await app.call("/v1/keys", { token: s.access }));
    expect(got).toEqual({ openai: { key: SECRET_KEY, updated_at }, anthropic: null });
  });

  it("stores ciphertext, never the plaintext, with a fresh IV each write", async () => {
    const app = new App();
    const s = await app.signIn();
    await app.call("/v1/keys/anthropic", { method: "PUT", token: s.access, body: { key: SECRET_KEY } });
    const first = await rowOf(s.userId, "anthropic");
    expect(first.ciphertext).not.toContain(SECRET_KEY);
    expect(first.ciphertext).not.toBe(btoa(SECRET_KEY));
    expect(first.key_version).toBe(1);
    expect(atob(first.iv)).toHaveLength(12);

    await app.call("/v1/keys/anthropic", { method: "PUT", token: s.access, body: { key: SECRET_KEY } });
    const second = await rowOf(s.userId, "anthropic");
    expect(second.iv).not.toBe(first.iv);
    expect(second.ciphertext).not.toBe(first.ciphertext);

    // Nothing in the whole database contains the plaintext.
    const dump = JSON.stringify(await env.DB.prepare("SELECT * FROM api_keys").all());
    expect(dump).not.toContain(SECRET_KEY);
  });

  it("a ciphertext copied to another user does not decrypt", async () => {
    const app = new App();
    const alice = await app.signIn({ sub: "a", email: "a@example.com" });
    const bob = await app.signIn({ sub: "b", email: "b@example.com" });
    await app.call("/v1/keys/openai", { method: "PUT", token: alice.access, body: { key: SECRET_KEY } });
    const row = await rowOf(alice.userId, "openai");

    await env.DB.prepare(
      "INSERT INTO api_keys (user_id, provider, ciphertext, iv, key_version, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
    ).bind(bob.userId, "openai", row.ciphertext, row.iv, row.key_version, row.updated_at).run();

    const got = await jsonOf(await app.call("/v1/keys", { token: bob.access }));
    expect(got.openai).toBeNull();
    await expect(decryptApiKey(env.MASTER_KEY, bob.userId, "openai", row)).rejects.toThrow();
    // Alice still reads her own.
    expect((await jsonOf(await app.call("/v1/keys", { token: alice.access }))).openai.key).toBe(SECRET_KEY);
  });

  it("a ciphertext moved to another provider does not decrypt", async () => {
    const app = new App();
    const s = await app.signIn();
    await app.call("/v1/keys/openai", { method: "PUT", token: s.access, body: { key: SECRET_KEY } });
    const row = await rowOf(s.userId, "openai");
    await env.DB.prepare(
      "INSERT INTO api_keys (user_id, provider, ciphertext, iv, key_version, updated_at) VALUES (?, 'anthropic', ?, ?, 1, ?)",
    ).bind(s.userId, row.ciphertext, row.iv, row.updated_at).run();

    const got = await jsonOf(await app.call("/v1/keys", { token: s.access }));
    expect(got.anthropic).toBeNull();
    expect(got.openai.key).toBe(SECRET_KEY);
  });

  it("tampered ciphertext, a different master key, or an unknown version fails to decrypt", async () => {
    const app = new App();
    const s = await app.signIn();
    const enc = await encryptApiKey(env.MASTER_KEY, s.userId, "openai", SECRET_KEY);
    const stored = { ciphertext: enc.ciphertext, iv: enc.iv, key_version: enc.keyVersion };
    expect(await decryptApiKey(env.MASTER_KEY, s.userId, "openai", stored)).toBe(SECRET_KEY);

    const flipped = atob(stored.ciphertext).split("");
    flipped[0] = String.fromCharCode(flipped[0]!.charCodeAt(0) ^ 1);
    await expect(decryptApiKey(env.MASTER_KEY, s.userId, "openai", { ...stored, ciphertext: btoa(flipped.join("")) })).rejects.toThrow();
    await expect(decryptApiKey(btoa("m".repeat(32)), s.userId, "openai", stored)).rejects.toThrow();
    await expect(decryptApiKey(env.MASTER_KEY, s.userId, "openai", { ...stored, key_version: 2 })).rejects.toThrow();
  });

  it("derives a different encryption key for each user", async () => {
    const a = await encryptApiKey(env.MASTER_KEY, "user-a", "openai", SECRET_KEY);
    await expect(
      decryptApiKey(env.MASTER_KEY, "user-b", "openai", { ciphertext: a.ciphertext, iv: a.iv, key_version: 1 }),
    ).rejects.toThrow();
  });

  it("an empty key deletes that provider only", async () => {
    const app = new App();
    const s = await app.signIn();
    await app.call("/v1/keys/openai", { method: "PUT", token: s.access, body: { key: "k1" } });
    await app.call("/v1/keys/anthropic", { method: "PUT", token: s.access, body: { key: "k2" } });
    const del = await app.call("/v1/keys/openai", { method: "PUT", token: s.access, body: { key: "  " } });
    expect(del.status).toBe(200);
    const got = await jsonOf(await app.call("/v1/keys", { token: s.access }));
    expect(got.openai).toBeNull();
    expect(got.anthropic.key).toBe("k2");
  });

  it("DELETE /v1/keys removes every provider", async () => {
    const app = new App();
    const s = await app.signIn();
    await app.call("/v1/keys/openai", { method: "PUT", token: s.access, body: { key: "k1" } });
    await app.call("/v1/keys/anthropic", { method: "PUT", token: s.access, body: { key: "k2" } });
    expect((await app.call("/v1/keys", { method: "DELETE", token: s.access })).status).toBe(204);
    expect(await jsonOf(await app.call("/v1/keys", { token: s.access }))).toEqual({ openai: null, anthropic: null });
  });

  it("validates the provider and the key", async () => {
    const app = new App();
    const s = await app.signIn();
    for (const p of ["google", "OPENAI", "..%2Fopenai", "openai%00"]) {
      const r = await app.call(`/v1/keys/${p}`, { method: "PUT", token: s.access, body: { key: "k" } });
      expect(r.status, p).toBe(400);
    }
    expect((await app.call("/v1/keys/openai", { method: "PUT", token: s.access, body: { key: 123 } })).status).toBe(400);
    expect((await app.call("/v1/keys/openai", { method: "PUT", token: s.access, body: {} })).status).toBe(400);
    expect((await app.call("/v1/keys/openai", { method: "PUT", token: s.access, body: { key: "x".repeat(513) } })).status).toBe(400);
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM api_keys").first<any>()).toEqual({ n: 0 });
  });

  it("reports a clear server error (not a leak) if MASTER_KEY is missing", async () => {
    const app = new App({ MASTER_KEY: "" });
    const s = await app.signIn();
    const r = await app.call("/v1/keys/openai", { method: "PUT", token: s.access, body: { key: SECRET_KEY } });
    expect(r.status).toBe(500);
    expect(await r.text()).not.toContain(SECRET_KEY);
  });
});

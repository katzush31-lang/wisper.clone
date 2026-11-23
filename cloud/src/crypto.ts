// WebCrypto only. No keys, tokens or plaintext are ever logged from here.
import { HttpError } from "./http";

const enc = new TextEncoder();
const dec = new TextDecoder();

export const KEY_VERSION = 1;
const HKDF_SALT = "wisper-keys-v1";

// ---------- encoding ----------

export function b64Encode(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s);
}

export function b64Decode(s: string): Uint8Array<ArrayBuffer> {
  const bin = atob(s); // throws on invalid input
  const out = new Uint8Array(new ArrayBuffer(bin.length));
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function b64urlEncode(bytes: Uint8Array): string {
  return b64Encode(bytes).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function b64urlDecode(s: string): Uint8Array<ArrayBuffer> {
  if (!/^[A-Za-z0-9_-]*$/.test(s)) throw new Error("bad base64url");
  const std = s.replace(/-/g, "+").replace(/_/g, "/");
  return b64Decode(std + "=".repeat((4 - (std.length % 4)) % 4));
}

export function utf8(s: string): Uint8Array<ArrayBuffer> {
  return enc.encode(s) as Uint8Array<ArrayBuffer>;
}

export function fromUtf8(b: ArrayBuffer | Uint8Array): string {
  return dec.decode(b);
}

// ---------- randomness and hashing ----------

export function randomBytes(n: number): Uint8Array<ArrayBuffer> {
  return crypto.getRandomValues(new Uint8Array(new ArrayBuffer(n)));
}

/** 32 random bytes as base64url (refresh tokens, login codes). */
export function randomToken(): string {
  return b64urlEncode(randomBytes(32));
}

/** base64url(SHA-256(text)). Used for stored hashes and the PKCE challenge. */
export async function sha256B64url(text: string): Promise<string> {
  return b64urlEncode(new Uint8Array(await crypto.subtle.digest("SHA-256", utf8(text))));
}

/** Compares two strings without bailing out at the first difference. */
export function timingSafeEqual(a: string, b: string): boolean {
  const x = utf8(a);
  const y = utf8(b);
  let diff = x.length ^ y.length;
  const len = Math.max(x.length, y.length);
  for (let i = 0; i < len; i++) diff |= (x[i] ?? 0) ^ (y[i] ?? 0);
  return diff === 0;
}

// ---------- HMAC (access tokens) ----------

export function misconfigured(): HttpError {
  return new HttpError(500, "server_error", "The server is not set up correctly. Please tell the owner.");
}

function secretBytes(b64: string | undefined, minBytes: number): Uint8Array<ArrayBuffer> {
  if (!b64) throw misconfigured();
  let bytes: Uint8Array<ArrayBuffer>;
  try {
    bytes = b64Decode(b64);
  } catch {
    throw misconfigured();
  }
  if (bytes.length < minBytes) throw misconfigured();
  return bytes;
}

export async function hmacKey(tokenSecretB64: string, usage: "sign" | "verify"): Promise<CryptoKey> {
  return crypto.subtle.importKey(
    "raw",
    secretBytes(tokenSecretB64, 32),
    { name: "HMAC", hash: "SHA-256" },
    false,
    [usage],
  );
}

// ---------- per-user API-key encryption ----------

/** HKDF-SHA256(ikm = MASTER_KEY, salt = "wisper-keys-v1", info = user_id) -> AES-256-GCM key. */
export async function deriveUserKey(masterKeyB64: string, userId: string): Promise<CryptoKey> {
  const ikm = await crypto.subtle.importKey("raw", secretBytes(masterKeyB64, 32), "HKDF", false, ["deriveKey"]);
  return crypto.subtle.deriveKey(
    { name: "HKDF", hash: "SHA-256", salt: utf8(HKDF_SALT), info: utf8(userId) },
    ikm,
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt", "decrypt"],
  );
}

/** The AAD ties a ciphertext to one user and one provider. */
function aad(userId: string, provider: string): Uint8Array<ArrayBuffer> {
  return utf8(`${userId}:${provider}`);
}

export interface EncryptedKey {
  ciphertext: string;
  iv: string;
  keyVersion: number;
}

export async function encryptApiKey(
  masterKeyB64: string,
  userId: string,
  provider: string,
  plaintext: string,
): Promise<EncryptedKey> {
  const key = await deriveUserKey(masterKeyB64, userId);
  const iv = randomBytes(12);
  const ct = await crypto.subtle.encrypt(
    { name: "AES-GCM", iv, additionalData: aad(userId, provider) },
    key,
    utf8(plaintext),
  );
  return { ciphertext: b64Encode(new Uint8Array(ct)), iv: b64Encode(iv), keyVersion: KEY_VERSION };
}

/** Throws if the data was tampered with or belongs to a different user/provider. */
export async function decryptApiKey(
  masterKeyB64: string,
  userId: string,
  provider: string,
  stored: { ciphertext: string; iv: string; key_version: number },
): Promise<string> {
  if (stored.key_version !== KEY_VERSION) throw new Error("unknown key version");
  const key = await deriveUserKey(masterKeyB64, userId);
  const pt = await crypto.subtle.decrypt(
    { name: "AES-GCM", iv: b64Decode(stored.iv), additionalData: aad(userId, provider) },
    key,
    b64Decode(stored.ciphertext),
  );
  return fromUtf8(pt);
}

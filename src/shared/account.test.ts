import { describe, expect, it } from "vitest";
import { accountState, keyStorageNote, shouldOfferSignIn, syncLine, timeAgo, whoLabel } from "./account";

const NOW = 1_700_000_000_000;

describe("timeAgo", () => {
  it("gives plain relative text", () => {
    expect(timeAgo(NOW - 5_000, NOW)).toBe("just now");
    expect(timeAgo(NOW - 60_000, NOW)).toBe("1 minute ago");
    expect(timeAgo(NOW - 2 * 60_000, NOW)).toBe("2 minutes ago");
    expect(timeAgo(NOW - 3 * 3_600_000, NOW)).toBe("3 hours ago");
    expect(timeAgo(NOW - 24 * 3_600_000, NOW)).toBe("yesterday");
    expect(timeAgo(NOW - 5 * 86_400_000, NOW)).toBe("5 days ago");
  });
  it("never goes negative when the clock is off", () => {
    expect(timeAgo(NOW + 60_000, NOW)).toBe("just now");
  });
});

describe("syncLine", () => {
  it("shows the last sync", () => {
    expect(syncLine({ lastSync: NOW - 120_000 }, NOW)).toEqual({ text: "Synced 2 minutes ago", tone: "ok" });
  });
  it("prefers an error", () => {
    const l = syncLine({ lastSync: NOW, syncError: "No internet" }, NOW);
    expect(l.tone).toBe("error");
    expect(l.text).toContain("No internet");
  });
  it("handles never synced", () => {
    expect(syncLine({}, NOW).tone).toBe("idle");
  });
});

describe("accountState", () => {
  it("maps status to a state", () => {
    expect(accountState(null)).toBe("unavailable");
    expect(accountState({ available: false, signedIn: false })).toBe("unavailable");
    expect(accountState({ available: true, signedIn: false })).toBe("signed-out");
    expect(accountState({ available: true, signedIn: true })).toBe("signed-in");
  });
});

describe("keyStorageNote", () => {
  it("says cloud only when signed in with cloud storage", () => {
    expect(keyStorageNote({ available: true, signedIn: true, keyStorage: "cloud" })).toMatch(/synced/);
    expect(keyStorageNote({ available: true, signedIn: true, keyStorage: "local" })).toBe("Stored on this computer only");
    expect(keyStorageNote({ available: true, signedIn: false, keyStorage: "cloud" })).toBe("Stored on this computer only");
    expect(keyStorageNote(null)).toBe("Stored on this computer only");
  });
});

describe("whoLabel / shouldOfferSignIn", () => {
  it("formats who is signed in", () => {
    expect(whoLabel({ name: "Dina", email: "d@x.com" })).toBe("Dina (d@x.com)");
    expect(whoLabel({ email: "d@x.com" })).toBe("d@x.com");
    expect(whoLabel({})).toBe("Signed in");
  });
  it("offers the card only when signed out, available and not dismissed", () => {
    expect(shouldOfferSignIn({ available: true, signedIn: false }, false)).toBe(true);
    expect(shouldOfferSignIn({ available: true, signedIn: false }, true)).toBe(false);
    expect(shouldOfferSignIn({ available: true, signedIn: true }, false)).toBe(false);
    expect(shouldOfferSignIn({ available: false, signedIn: false }, false)).toBe(false);
    expect(shouldOfferSignIn(null, false)).toBe(false);
  });
});

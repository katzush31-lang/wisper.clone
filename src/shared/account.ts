// Pure helpers for the optional Wisper account: plain-English text from the account status.
import type { AccountStatus, KeyStorage } from "./api";

/** "just now", "2 minutes ago", "3 hours ago", "yesterday", "5 days ago". */
export function timeAgo(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return m <= 1 ? "1 minute ago" : `${m} minutes ago`;
  const h = Math.round(m / 60);
  if (h < 24) return h === 1 ? "1 hour ago" : `${h} hours ago`;
  const d = Math.round(h / 24);
  return d === 1 ? "yesterday" : `${d} days ago`;
}

export type SyncTone = "ok" | "error" | "idle";

/** The one-line sync status shown when signed in. An error wins over the last-sync time. */
export function syncLine(status: Pick<AccountStatus, "lastSync" | "syncError">, now = Date.now()): { text: string; tone: SyncTone } {
  if (status.syncError) return { text: `Couldn't sync: ${status.syncError}`, tone: "error" };
  if (status.lastSync) return { text: `Synced ${timeAgo(status.lastSync, now)}`, tone: "ok" };
  return { text: "Not synced yet", tone: "idle" };
}

export type AccountState = "unavailable" | "signed-out" | "signed-in";

export function accountState(status: Pick<AccountStatus, "available" | "signedIn"> | null | undefined): AccountState {
  if (!status || !status.available) return "unavailable";
  return status.signedIn ? "signed-in" : "signed-out";
}

/** Where API keys live right now, for the muted line under the key boxes. */
export function keyStorageNote(
  status: Pick<AccountStatus, "available" | "signedIn" | "keyStorage" | "keyStorageChosen"> | null | undefined,
): string {
  // Until the choice is made on this computer, nothing has been uploaded.
  if (status?.available && status.signedIn && status.keyStorage === "cloud" && status.keyStorageChosen !== false) {
    return "Stored in your account and synced to your computers (encrypted)";
  }
  return "Stored on this computer only";
}

export const KEY_STORAGE_DESCRIPTIONS: Record<KeyStorage, string> = {
  local: "Your API keys stay on this PC and are never uploaded. This choice is just for this computer.",
  cloud: "Your API keys are encrypted and synced with your other computers that also use this option, so you don't paste them again.",
};

/** Who is signed in: name and email when both are known, otherwise whichever exists. */
export function whoLabel(status: Pick<AccountStatus, "name" | "email">): string {
  return status.name && status.email ? `${status.name} (${status.email})` : status.name || status.email || "Signed in";
}

/** Whether to offer the "sign in to sync" card on Home. */
export function shouldOfferSignIn(status: Pick<AccountStatus, "available" | "signedIn"> | null | undefined, dismissed: boolean): boolean {
  return accountState(status) === "signed-out" && !dismissed;
}

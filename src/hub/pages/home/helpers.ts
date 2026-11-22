import type { HistoryItem } from "../../../shared/api";

export function itemText(i: HistoryItem): string {
  return i.finalText ?? i.cleanText ?? i.rawText ?? "";
}

export interface StatusInfo {
  label: string;
  tone: "warn" | "bad" | "neutral";
  reason: string;
}

/** A short plain-English chip + reason for items that are not simply "done". */
export function statusInfo(i: HistoryItem): StatusInfo | null {
  switch (i.status) {
    case "queued":
      return {
        label: "Waiting",
        tone: "warn",
        reason: i.error || "Saved, and waiting for the internet or a fix before it can be written out.",
      };
    case "failed":
      return {
        label: "Didn't work",
        tone: "bad",
        reason: i.error || "Something went wrong. Your recording is saved, so you can try again.",
      };
    case "cancelled":
      return { label: "Cancelled", tone: "neutral", reason: "You cancelled this one. The recording is still saved." };
    case "empty":
      return { label: "No speech", tone: "neutral", reason: "No speech was heard in this recording." };
    case "transcribing":
    case "cleaning":
    case "recording":
      return { label: "Working…", tone: "neutral", reason: "Still being turned into text." };
    default:
      return null;
  }
}

export function ms(n: number | undefined): string | null {
  if (n == null) return null;
  return n >= 1000 ? `${(n / 1000).toFixed(1)} s` : `${Math.round(n)} ms`;
}

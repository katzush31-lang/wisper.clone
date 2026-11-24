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
    case "silent":
      return { label: "No speech heard", tone: "neutral", reason: "No speech was heard. If you did speak, choose Try again. It is deleted after 24 hours." };
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

const MODEL_NAMES: Record<string, string> = {
  local: "tidied on this computer",
  "claude-sonnet-5-5": "Claude Sonnet 5.5",
  "claude-haiku-5-5": "Claude Haiku 5.5",
  "claude-opus-5-5": "Claude Opus 5.5",
  "gpt-6.2-sol": "GPT-6.2 Sol",
  "gpt-6-luna": "GPT-6 Luna",
  "gpt-6.1-terra": "GPT-6.1 Terra",
  "gpt-6.2-astra": "GPT-6.2 Astra",
};

/** Friendly name for a model id (the id itself if it is not a known one). */
export function modelName(id: string): string {
  return MODEL_NAMES[id] ?? id;
}

/** History detail: how much of the cleanup happened while the user was still talking. */
export function tidyNote(t: HistoryItem["timings"]): string | null {
  if (!t) return null;
  if (t.chunked && t.chunked > 0) {
    return `${t.chunked} ${t.chunked === 1 ? "part" : "parts"} tidied while you talked`;
  }
  if (t.chunkCalls && t.chunkCalls > 0) return "tidied all at once at the end";
  return null;
}

/** Failed and queued recordings still hold speech; History shows them as one row. */
export function isWaiting(i: HistoryItem): boolean {
  return i.status === "queued" || i.status === "failed";
}

export interface KeyState {
  openai: boolean;
  anthropic: boolean;
}

function providerOf(message: string): "OpenAI" | "Claude" | null {
  if (/openai/i.test(message)) return "OpenAI";
  if (/claude|anthropic/i.test(message)) return "Claude";
  return null;
}

/** One short plain-English reason for a single waiting recording. */
export function waitingReason(i: HistoryItem, keys?: KeyState | null): string {
  const who = providerOf(i.error ?? "");
  switch (i.errorKind) {
    case "auth": {
      const missing = who === "OpenAI" ? keys?.openai === false : who === "Claude" ? keys?.anthropic === false : false;
      return `${missing ? "add" : "check"} your ${who ?? "AI"} key`;
    }
    case "quota":
      return `your ${who ?? "AI"} account is out of credit`;
    case "network":
    case "timeout":
      return "waiting for an internet connection";
    case "rateLimited":
    case "server":
      return "the AI service is busy, Wisper will keep trying";
    default:
      return "open one to see what went wrong";
  }
}

export interface WaitingSummary {
  count: number;
  /** "3 recordings are waiting to be processed — add your OpenAI key" */
  text: string;
}

/** The compact row's text: the count plus the reason given by most of the recordings. */
export function waitingSummary(items: HistoryItem[], keys?: KeyState | null): WaitingSummary {
  const count = items.length;
  const tally = new Map<string, number>();
  for (const it of items) {
    const r = waitingReason(it, keys);
    tally.set(r, (tally.get(r) ?? 0) + 1);
  }
  let reason = "";
  let best = 0;
  for (const [r, n] of tally) {
    if (n > best) {
      best = n;
      reason = r;
    }
  }
  const lead = count === 1 ? "1 recording is waiting to be processed" : `${count} recordings are waiting to be processed`;
  return { count, text: reason ? `${lead} \u2014 ${reason}` : lead };
}

/** The Home screen asks for keys only when one it actually needs is missing. */
export function keysMissing(keys: KeyState | null, cleanupProvider: "anthropic" | "openAi" | undefined): boolean {
  if (!keys) return false;
  if (!keys.openai) return true; // speech-to-text always uses OpenAI
  return cleanupProvider === "openAi" ? !keys.openai : !keys.anthropic;
}

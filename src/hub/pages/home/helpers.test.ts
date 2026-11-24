import { describe, expect, it } from "vitest";
import type { HistoryItem } from "../../../shared/api";
import { isWaiting, keysMissing, tidyNote, waitingReason, waitingSummary } from "./helpers";

function item(over: Partial<HistoryItem>): HistoryItem {
  return {
    id: "x",
    createdAt: 0,
    kind: "dictation",
    status: "failed",
    modeId: null,
    appKey: null,
    appName: null,
    durationMs: 1000,
    rawText: null,
    cleanText: null,
    finalText: null,
    selectedText: null,
    wordCount: 0,
    audioPath: "a.flac",
    error: null,
    errorKind: null,
    inserted: false,
    attempts: 1,
    timings: null,
    meta: null,
    ...over,
  };
}

const noKey = { openai: false, anthropic: true };

describe("waiting recordings", () => {
  it("only failed and queued count as waiting", () => {
    expect(isWaiting(item({ status: "failed" }))).toBe(true);
    expect(isWaiting(item({ status: "queued" }))).toBe(true);
    for (const status of ["done", "cancelled", "empty", "transcribing"] as const) {
      expect(isWaiting(item({ status }))).toBe(false);
    }
  });

  it("says to add a missing key, or to check one that is set", () => {
    const i = item({ errorKind: "auth", error: "Your OpenAI API key isn't working." });
    expect(waitingReason(i, noKey)).toBe("add your OpenAI key");
    expect(waitingReason(i, { openai: true, anthropic: true })).toBe("check your OpenAI key");
    expect(waitingReason(item({ errorKind: "auth", error: "Your Claude API key isn't working." }), noKey)).toBe("check your Claude key");
  });

  it("summarises with the most common reason", () => {
    const auth = item({ errorKind: "auth", error: "Your OpenAI API key isn't working." });
    const net = item({ errorKind: "network", error: "No internet connection." });
    const s = waitingSummary([auth, auth, net], noKey);
    expect(s.count).toBe(3);
    expect(s.text).toBe("3 recordings are waiting to be processed \u2014 add your OpenAI key");
    expect(waitingSummary([net]).text).toBe("1 recording is waiting to be processed \u2014 waiting for an internet connection");
  });

  it("falls back to a neutral reason", () => {
    expect(waitingReason(item({ errorKind: "badRequest" }))).toBe("open one to see what went wrong");
  });
});

describe("keysMissing", () => {
  it("needs OpenAI always, and the key of the chosen cleanup provider", () => {
    expect(keysMissing(null, "anthropic")).toBe(false); // not loaded yet
    expect(keysMissing({ openai: false, anthropic: true }, "anthropic")).toBe(true);
    expect(keysMissing({ openai: true, anthropic: false }, "anthropic")).toBe(true);
    expect(keysMissing({ openai: true, anthropic: true }, "anthropic")).toBe(false);
  });
  it("does not ask for a Claude key when cleanup uses OpenAI", () => {
    expect(keysMissing({ openai: true, anthropic: false }, "openAi")).toBe(false);
    expect(keysMissing({ openai: false, anthropic: true }, "openAi")).toBe(true);
  });
});

describe("tidyNote", () => {
  it("says how many parts were tidied while talking", () => {
    expect(tidyNote({ chunked: 3, chunkCalls: 3 })).toBe("3 parts tidied while you talked");
    expect(tidyNote({ chunked: 1, chunkCalls: 1 })).toBe("1 part tidied while you talked");
  });
  it("notes when background work wasn't used, and is silent otherwise", () => {
    expect(tidyNote({ chunked: 0, chunkCalls: 2, chunkFallback: "a possible correction near the end" })).toBe("tidied all at once at the end");
    expect(tidyNote({ chunked: 0, chunkCalls: 0 })).toBeNull();
    expect(tidyNote({})).toBeNull();
    expect(tidyNote(null)).toBeNull();
  });
});

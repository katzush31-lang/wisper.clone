import { describe, expect, it } from "vitest";
import { jumpScore, rankJump } from "./jump";

const E = [
  { id: "home", label: "Home", hint: "Start here", keywords: "history" },
  { id: "s:providers", label: "AI providers", hint: "Your OpenAI and Anthropic keys", keywords: "api key" },
  { id: "s:shortcuts", label: "Shortcuts", hint: "Keys that start dictation", keywords: "hotkey" },
  { id: "snippets", label: "Snippets", hint: "Say a short phrase" },
];

describe("jump search", () => {
  it("shows everything for an empty query, in the original order", () => {
    expect(rankJump(E, "  ").map((e) => e.id)).toEqual(E.map((e) => e.id));
  });

  it("puts a label prefix match first", () => {
    expect(rankJump(E, "sn")[0].id).toBe("snippets");
  });

  it("finds entries by keyword and hint", () => {
    expect(rankJump(E, "api").map((e) => e.id)).toEqual(["s:providers"]);
    expect(rankJump(E, "history").map((e) => e.id)).toEqual(["home"]);
  });

  it("needs every typed word to match", () => {
    expect(jumpScore(E[1], "openai keys")).toBeGreaterThan(0);
    expect(jumpScore(E[1], "openai banana")).toBe(0);
  });
});

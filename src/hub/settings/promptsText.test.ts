import { describe, expect, it } from "vitest";
import type { PromptPreview } from "../../shared/api";
import { commandPromptText, dictationPromptText } from "./promptsText";

const p: PromptPreview = {
  modeId: "email",
  modeName: "Email",
  autoPicked: false,
  chosenModeName: "Email",
  aiUsed: true,
  level: "light",
  levelText: "Edit lightly",
  baseSystem: "BASE RULES\n",
  modeSystem: "MODE TEXT\n",
  userMessage: "<transcript>\nhi\n</transcript>",
  appTypes: [],
  commandSystem: "COMMAND RULES\n",
  commandUser: "<instruction>\nx\n</instruction>",
  transforms: [],
};

describe("prompt text for copying", () => {
  it("keeps the blocks in the order they are sent", () => {
    const t = dictationPromptText(p);
    expect(t.indexOf("BASE RULES")).toBeLessThan(t.indexOf("MODE TEXT"));
    expect(t.indexOf("MODE TEXT")).toBeLessThan(t.indexOf("<transcript>"));
    expect(t).toContain("Email mode");
  });
  it("includes the command rules and message", () => {
    const t = commandPromptText(p);
    expect(t).toContain("COMMAND RULES");
    expect(t).toContain("<instruction>");
  });
});

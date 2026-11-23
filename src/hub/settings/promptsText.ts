import type { PromptPreview } from "../../shared/api";

/** Everything sent for one dictation, as plain text for the Copy button. */
export function dictationPromptText(p: PromptPreview): string {
  return [
    "=== SYSTEM, part 1: shared instructions and dictionary ===",
    p.baseSystem.trim(),
    "",
    `=== SYSTEM, part 2: ${p.modeName} mode and cleanup level ===`,
    p.modeSystem.trim(),
    "",
    "=== USER MESSAGE (example) ===",
    p.userMessage.trim(),
    "",
  ].join("\n");
}

/** The voice-command prompt as plain text. */
export function commandPromptText(p: PromptPreview): string {
  return ["=== SYSTEM ===", p.commandSystem.trim(), "", "=== USER MESSAGE (example) ===", p.commandUser.trim(), ""].join("\n");
}

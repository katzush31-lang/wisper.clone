import { describe, expect, it } from "vitest";
import {
  AUDIO_RETENTION,
  DEFAULT_HOTKEYS,
  clampWpm,
  languageChoice,
  languagesFor,
  parseExcludedApps,
  retentionFromString,
  retentionToString,
  setCombo,
  mouseButtonToVk,
  doubleClickChoice,
  withCurrent,
} from "./helpers";

describe("retention mapping", () => {
  it("round-trips every audio option", () => {
    for (const o of AUDIO_RETENTION) expect(retentionFromString(retentionToString(o.value))).toBe(o.value);
  });
  it("maps forever to null", () => {
    expect(retentionFromString("forever")).toBeNull();
    expect(retentionFromString("30")).toBe(30);
  });
  it("keeps an unusual saved value visible", () => {
    expect(withCurrent(AUDIO_RETENTION, 14).some((o) => o.value === 14)).toBe(true);
    expect(withCurrent(AUDIO_RETENTION, 30)).toBe(AUDIO_RETENTION);
  });
});

describe("languages", () => {
  it("maps both ways", () => {
    expect(languageChoice([])).toBe("auto");
    expect(languageChoice(["en"])).toBe("en");
    expect(languageChoice(["en", "he"])).toBe("en-he");
    expect(languagesFor("en-he")).toEqual(["en", "he"]);
    expect(languagesFor("auto")).toEqual([]);
  });
});

describe("misc helpers", () => {
  it("parses excluded apps", () => {
    expect(parseExcludedApps(" 1Password.exe, code.exe ,, 1password.exe")).toEqual(["1password.exe", "code.exe"]);
  });
  it("clamps typing speed", () => {
    expect(clampWpm(5)).toBe(10);
    expect(clampWpm(1000)).toBe(250);
    expect(clampWpm(NaN)).toBe(40);
  });
  it("replaces one combo without mutating", () => {
    const next = setCombo(DEFAULT_HOTKEYS, "dictate", [0x11, 0x20]);
    expect(next.dictate).toEqual([{ keys: [0x11, 0x20] }]);
    expect(DEFAULT_HOTKEYS.dictate).toEqual([{ keys: [0x11, 0x5b] }]);
  });
  it("saves double-click mouse shortcuts with the flag", () => {
    expect(setCombo(DEFAULT_HOTKEYS, "dictate", [0x04], true).dictate).toEqual([{ keys: [0x04], doubleClick: true }]);
  });
  it("maps browser mouse buttons to Windows codes", () => {
    expect(mouseButtonToVk(1)).toBe(0x04);
    expect(mouseButtonToVk(3)).toBe(0x05);
    expect(mouseButtonToVk(4)).toBe(0x06);
    expect(mouseButtonToVk(0)).toBe(0x01);
    expect(mouseButtonToVk(2)).toBe(0x02);
    expect(mouseButtonToVk(7)).toBeNull();
  });
  it("offers double-click only for a lone mouse button", () => {
    expect(doubleClickChoice([0x04])).toBe("forced");
    expect(doubleClickChoice([0x05])).toBe("optional");
    expect(doubleClickChoice([0x11, 0x04])).toBe("none");
    expect(doubleClickChoice([0x70])).toBe("none");
  });
});

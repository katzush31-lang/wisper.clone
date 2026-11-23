import { describe, expect, it } from "vitest";
import { comboKeyNames, comboLabel } from "../../shared/api";
import { keyToVk } from "./ShortcutsSection";

const ev = (key: string, code: string, keyCode = 0) => ({ key, code, keyCode });

describe("keyToVk", () => {
  it("maps modifiers to Windows key codes", () => {
    expect(keyToVk(ev("Control", "ControlLeft", 17))).toBe(17);
    expect(keyToVk(ev("Shift", "ShiftRight", 16))).toBe(16);
    expect(keyToVk(ev("Alt", "AltLeft", 18))).toBe(18);
    expect(keyToVk(ev("Meta", "MetaLeft", 91))).toBe(91);
  });
  it("maps letters, digits, space and function keys by position", () => {
    expect(keyToVk(ev("z", "KeyZ", 90))).toBe(90);
    // Hebrew layout: the key still maps to its physical letter.
    expect(keyToVk(ev("ז", "KeyZ", 90))).toBe(90);
    expect(keyToVk(ev("1", "Digit1", 49))).toBe(49);
    expect(keyToVk(ev(" ", "Space", 32))).toBe(32);
    expect(keyToVk(ev("F9", "F9", 120))).toBe(120);
  });
});

describe("shortcut key caps", () => {
  it("names mouse buttons and marks double-click shortcuts", () => {
    expect(comboKeyNames({ keys: [0x04], doubleClick: true })).toEqual(["Double-click", "Middle click"]);
    // The middle button on its own is always a double-click (same rule as the Rust side).
    expect(comboLabel({ keys: [0x04] })).toBe("Double-click + Middle click");
    expect(comboLabel({ keys: [0x05] })).toBe("Back button");
    expect(comboLabel({ keys: [0x06], doubleClick: true })).toBe("Double-click + Forward button");
    expect(comboLabel({ keys: [0x06, 0x11] })).toBe("Ctrl + Forward button");
    expect(comboLabel({ keys: [0x5b, 0x11] })).toBe("Ctrl + Win");
  });
});

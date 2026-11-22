import { describe, expect, it } from "vitest";
import { barHeight, pushLevel } from "./waveform";

describe("pushLevel", () => {
  it("keeps a fixed size, newest last, clamped", () => {
    let h: number[] = [];
    h = pushLevel(h, 0.5, 4);
    expect(h).toEqual([0, 0, 0, 0.5]);
    h = pushLevel(h, 2, 4);
    expect(h).toEqual([0, 0, 0.5, 1]);
    h = pushLevel(h, -1, 4);
    h = pushLevel(h, NaN, 4);
    expect(h).toEqual([0.5, 1, 0, 0]);
    expect(h).toHaveLength(4);
    expect(h[3]).toBe(0);
  });
});

describe("barHeight", () => {
  it("is monotonic within bounds", () => {
    expect(barHeight(0)).toBe(4);
    expect(barHeight(1)).toBe(22);
    expect(barHeight(0.1)).toBeGreaterThan(4);
    expect(barHeight(0.5)).toBeGreaterThan(barHeight(0.1));
  });
});

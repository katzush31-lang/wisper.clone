import { describe, expect, it } from "vitest";
import { buildHeatmap, formatMinutes, heatBand, weeklyTotals } from "./insights/utils";

describe("heatBand", () => {
  it("maps word counts to 5 bands", () => {
    expect([0, 1, 249, 250, 499, 500, 749, 750, 5000].map(heatBand)).toEqual([0, 1, 1, 2, 2, 3, 3, 4, 4]);
  });
});

describe("weeklyTotals", () => {
  const today = new Date(2026, 10, 22); // Sunday 22 Nov 2026
  it("returns 12 weeks ending with this week and sums days", () => {
    const out = weeklyTotals(
      [
        { date: "2026-11-22", words: 100 },
        { date: "2026-11-21", words: 50 }, // previous week (Saturday)
        { date: "2026-11-16", words: 25 },
        { date: "2020-01-01", words: 999 }, // too old, ignored
      ],
      12,
      today,
    );
    expect(out).toHaveLength(12);
    expect(out[11]).toEqual({ start: "2026-11-22", words: 100 });
    expect(out[10]).toEqual({ start: "2026-11-15", words: 75 });
    expect(out.reduce((s, w) => s + w.words, 0)).toBe(175);
  });
});

describe("buildHeatmap", () => {
  it("builds 26 columns of 7 rows, hides future days and bands words", () => {
    const today = new Date(2026, 10, 25); // Wednesday
    const h = buildHeatmap([{ date: "2026-11-25", words: 800 }], 26, today);
    expect(h.columns).toHaveLength(26);
    expect(h.columns.every((c) => c.length === 7)).toBe(true);
    const last = h.columns[25];
    expect(last[3]?.band).toBe(4);
    expect(last[4]).toBeNull();
    expect(h.months.length).toBeGreaterThan(4);
  });
});

describe("formatMinutes", () => {
  it("formats", () => {
    expect(formatMinutes(200)).toBe("3 h 20 min");
    expect(formatMinutes(45)).toBe("45 min");
    expect(formatMinutes(120)).toBe("2 h");
  });
});

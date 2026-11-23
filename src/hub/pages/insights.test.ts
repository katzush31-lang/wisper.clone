import { describe, expect, it } from "vitest";
import { buildHeatmap, formatMinutes, formatSeconds, heatBand, speedSummary, weeklyTotals } from "./insights/utils";

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

describe("formatSeconds", () => {
  it("shows one decimal and never 0.0", () => {
    expect(formatSeconds(1400)).toBe("1.4 s");
    expect(formatSeconds(1449)).toBe("1.4 s");
    expect(formatSeconds(12)).toBe("0.1 s");
    expect(formatSeconds(12000)).toBe("12.0 s");
  });
});

describe("speedSummary", () => {
  const base = { samples: 10, transcribeWaitMedianMs: 600, cleanupMedianMs: 800, totalWaitMedianMs: 1400, fastModelShare: 0.4 };
  it("is hidden with fewer than 3 samples", () => {
    expect(speedSummary(undefined)).toBeNull();
    expect(speedSummary({ ...base, samples: 2 })).toBeNull();
  });
  it("formats the parts and the fast model count", () => {
    expect(speedSummary(base)).toEqual({ total: "1.4 s", transcribe: "0.6 s", cleanup: "0.8 s", fast: "4 of 10 used the fast model" });
    expect(speedSummary({ ...base, fastModelShare: 0 })?.fast).toBe("None used the fast model");
  });
});

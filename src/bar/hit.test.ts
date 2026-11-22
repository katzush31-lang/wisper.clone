import { describe, expect, it } from "vitest";
import { sameRect, unionRect } from "./hit";

describe("unionRect", () => {
  it("covers every visible part", () => {
    expect(
      unionRect([
        { x: 120.4, y: 40.2, width: 60, height: 10 },
        { x: 30, y: 2, width: 240.5, height: 30 },
      ]),
    ).toEqual({ x: 30, y: 2, width: 241, height: 49 });
  });
  it("ignores empty parts and is empty when nothing is shown", () => {
    expect(unionRect([{ x: 5, y: 5, width: 0, height: 0 }])).toEqual({ x: 0, y: 0, width: 0, height: 0 });
    expect(unionRect([])).toEqual({ x: 0, y: 0, width: 0, height: 0 });
  });
  it("compares rectangles", () => {
    expect(sameRect({ x: 1, y: 2, width: 3, height: 4 }, { x: 1, y: 2, width: 3, height: 4 })).toBe(true);
    expect(sameRect(null, { x: 1, y: 2, width: 3, height: 4 })).toBe(false);
  });
});

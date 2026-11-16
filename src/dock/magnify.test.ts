import { describe, expect, it } from "vitest";
import { iconScale, maxRowGrowth, rowGrowth } from "./magnify";

describe("iconScale", () => {
  it("is max at the cursor", () => expect(iconScale(0, 48, 1.6)).toBeCloseTo(1.6));
  it("is 1 outside 3×icon size", () => expect(iconScale(145, 48, 1.6)).toBe(1));
  it("decreases monotonically", () => expect(iconScale(20, 48, 1.6)).toBeGreaterThan(iconScale(60, 48, 1.6)));
  it("maxScale 1 disables", () => expect(iconScale(0, 48, 1)).toBe(1));
  it("is symmetric and follows cos²", () => {
    expect(iconScale(-72, 48, 1.6)).toBeCloseTo(iconScale(72, 48, 1.6));
    // d = range/2 → cos²(π/4) = 0.5
    expect(iconScale(72, 48, 1.6)).toBeCloseTo(1.3);
  });
});

describe("row growth", () => {
  const centers = [29, 87, 145, 203, 261];
  it("one icon under the cursor grows by (max-1)·size", () => expect(rowGrowth([100], 100, 48, 1.6)).toBeCloseTo(28.8));
  it("max growth bounds every cursor position (±0.5 px sampling)", () => {
    const max = maxRowGrowth(centers, 48, 1.6);
    for (let x = 0; x < 300; x += 7) expect(rowGrowth(centers, x, 48, 1.6)).toBeLessThanOrEqual(max + 0.5);
  });
  it("no growth without magnification", () => expect(maxRowGrowth(centers, 48, 1)).toBe(0));
});

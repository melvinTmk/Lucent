import { describe, expect, it } from "vitest";
import { hitItem, SPRING_IDLE, stepSpring } from "./drop";

const rects = [
  { id: "a", x: 10, y: 10, w: 40, h: 40 },
  { id: "b", x: 60, y: 10, w: 40, h: 40 },
];

describe("hitItem", () => {
  it("resolves a position to the icon under it", () => {
    expect(hitItem(rects, 20, 20)).toBe("a");
    expect(hitItem(rects, 99, 49)).toBe("b");
  });
  it("returns null in gaps, outside and on the far edges", () => {
    expect(hitItem(rects, 55, 20)).toBeNull();
    expect(hitItem(rects, 50, 20)).toBeNull();
    expect(hitItem(rects, 20, 5)).toBeNull();
    expect(hitItem([], 1, 1)).toBeNull();
  });
});

describe("stepSpring", () => {
  it("fires once after 700 ms on a running app", () => {
    let r = stepSpring(SPRING_IDLE, "a", true, 1000);
    expect(r.fire).toBeNull();
    r = stepSpring(r.state, "a", true, 1699);
    expect(r.fire).toBeNull();
    r = stepSpring(r.state, "a", true, 1700);
    expect(r.fire).toBe("a");
    expect(stepSpring(r.state, "a", true, 3000).fire).toBeNull();
  });
  it("restarts when the hover moves to another icon", () => {
    let r = stepSpring(SPRING_IDLE, "a", true, 0);
    r = stepSpring(r.state, "b", true, 600);
    expect(stepSpring(r.state, "b", true, 1200).fire).toBeNull();
    expect(stepSpring(r.state, "b", true, 1300).fire).toBe("b");
  });
  it("resets when leaving icons, so re-entering can fire again", () => {
    let r = stepSpring(SPRING_IDLE, "a", true, 0);
    r = stepSpring(r.state, "a", true, 800);
    expect(r.fire).toBe("a");
    r = stepSpring(r.state, null, false, 900);
    expect(r.state).toEqual(SPRING_IDLE);
    r = stepSpring(r.state, "a", true, 1000);
    expect(stepSpring(r.state, "a", true, 1700).fire).toBe("a");
  });
  it("never fires for an app that is not running", () => {
    let r = stepSpring(SPRING_IDLE, "a", false, 0);
    r = stepSpring(r.state, "a", false, 5000);
    expect(r.fire).toBeNull();
  });
});

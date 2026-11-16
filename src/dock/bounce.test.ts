import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createBounceManager } from "./bounce";

describe("createBounceManager", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("marks an item as bouncing when triggered", () => {
    const mgr = createBounceManager();
    expect(mgr.isBouncing("app-1")).toBe(false);

    mgr.trigger("app-1");
    expect(mgr.isBouncing("app-1")).toBe(true);
    expect(mgr.bouncingIds()).toEqual(["app-1"]);
  });

  it("clears bouncing state when windows appear", () => {
    const onChange = vi.fn();
    const mgr = createBounceManager({ onChange });
    mgr.trigger("app-1");
    expect(onChange).toHaveBeenCalledTimes(1);

    // Update with 0 windows keeps it bouncing
    mgr.update([{ id: "app-1", windows: [] }]);
    expect(mgr.isBouncing("app-1")).toBe(true);

    // Update with windows clears bouncing
    mgr.update([{ id: "app-1", windows: [{ hwnd: 123 }] }]);
    expect(mgr.isBouncing("app-1")).toBe(false);
    expect(onChange).toHaveBeenCalledTimes(2);
  });

  it("clears bouncing state automatically after timeout (10s default)", () => {
    const onChange = vi.fn();
    const mgr = createBounceManager({ onChange, timeoutMs: 10_000 });
    mgr.trigger("app-1");
    expect(mgr.isBouncing("app-1")).toBe(true);

    vi.advanceTimersByTime(9_999);
    expect(mgr.isBouncing("app-1")).toBe(true);

    vi.advanceTimersByTime(1);
    expect(mgr.isBouncing("app-1")).toBe(false);
    expect(onChange).toHaveBeenCalledTimes(2);
  });

  it("clears all timers on destroy", () => {
    const mgr = createBounceManager();
    mgr.trigger("app-1");
    mgr.destroy();
    expect(mgr.isBouncing("app-1")).toBe(false);
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createHoverPreview, HOVER_DWELL_MS, HOVER_GRACE_MS } from "./hover";

function setup() {
  const open = vi.fn<(id: string) => void>();
  const close = vi.fn<() => void>();
  return { open, close, h: createHoverPreview({ open, close }) };
}

describe("hover previews", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("opens after resting on a running icon for the dwell delay", () => {
    const { open, h } = setup();
    h.icon("a");
    vi.advanceTimersByTime(HOVER_DWELL_MS - 1);
    expect(open).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(open).toHaveBeenCalledWith("a");
    expect(h.openId()).toBe("a");
  });

  it("restarts the dwell when moving to another icon before it fires", () => {
    const { open, h } = setup();
    h.icon("a");
    vi.advanceTimersByTime(300);
    h.icon("b");
    vi.advanceTimersByTime(300);
    expect(open).not.toHaveBeenCalled();
    vi.advanceTimersByTime(HOVER_DWELL_MS - 300);
    expect(open).toHaveBeenCalledExactlyOnceWith("b");
  });

  it("does not open when the pointer leaves before the dwell", () => {
    const { open, h } = setup();
    h.icon("a");
    vi.advanceTimersByTime(200);
    h.icon(null);
    vi.advanceTimersByTime(1000);
    expect(open).not.toHaveBeenCalled();
  });

  it("switches immediately to another running icon once open", () => {
    const { open, close, h } = setup();
    h.icon("a");
    vi.advanceTimersByTime(HOVER_DWELL_MS);
    h.icon(null);
    h.icon("b");
    expect(open).toHaveBeenLastCalledWith("b");
    expect(h.openId()).toBe("b");
    vi.advanceTimersByTime(1000);
    expect(close).not.toHaveBeenCalled();
  });

  it("closes after the grace once the pointer left both icon and popup", () => {
    const { close, h } = setup();
    h.icon("a");
    vi.advanceTimersByTime(HOVER_DWELL_MS);
    h.icon(null);
    vi.advanceTimersByTime(HOVER_GRACE_MS - 1);
    expect(close).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(close).toHaveBeenCalledOnce();
    expect(h.openId()).toBeNull();
  });

  it("stays open while the pointer travels from the icon into the popup", () => {
    const { close, h } = setup();
    h.icon("a");
    vi.advanceTimersByTime(HOVER_DWELL_MS);
    h.icon(null);
    vi.advanceTimersByTime(100);
    h.popup(true);
    vi.advanceTimersByTime(1000);
    expect(close).not.toHaveBeenCalled();
    h.popup(false);
    vi.advanceTimersByTime(HOVER_GRACE_MS);
    expect(close).toHaveBeenCalledOnce();
  });

  it("going back to the icon from the popup cancels the grace", () => {
    const { close, h } = setup();
    h.icon("a");
    vi.advanceTimersByTime(HOVER_DWELL_MS);
    h.icon(null);
    h.popup(true);
    h.popup(false);
    vi.advanceTimersByTime(100);
    h.icon("a");
    vi.advanceTimersByTime(1000);
    expect(close).not.toHaveBeenCalled();
  });

  it("never opens while blocked (drag, game, hidden dock)", () => {
    const { open, h } = setup();
    h.setBlocked(true);
    h.icon("a");
    vi.advanceTimersByTime(1000);
    expect(open).not.toHaveBeenCalled();
  });

  it("a drag starting mid-dwell cancels it, and an open popup closes", () => {
    const { open, close, h } = setup();
    h.icon("a");
    vi.advanceTimersByTime(200);
    h.setBlocked(true);
    vi.advanceTimersByTime(1000);
    expect(open).not.toHaveBeenCalled();
    h.setBlocked(false);
    h.icon("b");
    vi.advanceTimersByTime(HOVER_DWELL_MS);
    expect(open).toHaveBeenCalledWith("b");
    h.setBlocked(true);
    expect(close).toHaveBeenCalledOnce();
    expect(h.openId()).toBeNull();
  });

  it("after a click that focuses, the same icon does not preview until left", () => {
    const { open, h } = setup();
    h.icon("a");
    vi.advanceTimersByTime(100);
    h.click("a", false);
    vi.advanceTimersByTime(1000);
    expect(open).not.toHaveBeenCalled();
    h.icon(null);
    h.icon("a");
    vi.advanceTimersByTime(HOVER_DWELL_MS);
    expect(open).toHaveBeenCalledWith("a");
  });

  it("a click that opens the popup makes it hover-managed (closes on leave)", () => {
    const { open, close, h } = setup();
    h.icon("a");
    h.click("a", true);
    expect(open).not.toHaveBeenCalled(); // Rust opened it from the click
    expect(h.openId()).toBe("a");
    h.icon(null);
    vi.advanceTimersByTime(HOVER_GRACE_MS);
    expect(close).toHaveBeenCalledOnce();
  });

  it("a close from Rust resets the machine and mutes the icon under the pointer", () => {
    const { open, close, h } = setup();
    h.icon("a");
    vi.advanceTimersByTime(HOVER_DWELL_MS);
    h.closed(); // e.g. Escape
    expect(h.openId()).toBeNull();
    vi.advanceTimersByTime(1000);
    expect(open).toHaveBeenCalledOnce();
    expect(close).not.toHaveBeenCalled();
    // Moving to another icon needs the dwell again (nothing is open any more).
    h.icon("b");
    vi.advanceTimersByTime(HOVER_DWELL_MS - 1);
    expect(open).toHaveBeenCalledOnce();
    vi.advanceTimersByTime(1);
    expect(open).toHaveBeenLastCalledWith("b");
  });
});

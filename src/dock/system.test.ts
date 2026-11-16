// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { VolumeState } from "../shared/types";

const invoke = vi.fn(async (..._args: unknown[]) => undefined);
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({ getCurrentWebviewWindow: () => ({ label: "dock-1" }) }));

const { renderSystem } = await import("./system");

const vol = (level: number, muted = false): VolumeState => ({
  level,
  muted,
  icon: muted || level === 0 ? "mute" : level < 0.34 ? "low" : level < 0.67 ? "mid" : "high",
});

function view(over: Record<string, unknown> = {}) {
  return {
    volume: vol(0.5),
    network: { kind: "wifi" as const, name: "Maison" },
    showClock: true,
    now: new Date(2026, 10, 9, 14, 5),
    lang: "fr",
    ...over,
  };
}

let sys: HTMLElement;
beforeEach(() => {
  document.body.innerHTML = '<div class="island tray"><div id="sys"></div></div>';
  sys = document.getElementById("sys")!;
  invoke.mockClear();
});

describe("renderSystem", () => {
  it("wheel changes volume by 5%", () => {
    renderSystem(sys, view());
    const btn = sys.querySelector<HTMLElement>(".sys-volume")!;
    btn.dispatchEvent(new WheelEvent("wheel", { deltaY: -100, bubbles: true, cancelable: true }));
    expect(invoke).toHaveBeenLastCalledWith("set_volume", { level: 0.55 });
    btn.dispatchEvent(new WheelEvent("wheel", { deltaY: 100, bubbles: true, cancelable: true }));
    expect(invoke).toHaveBeenLastCalledWith("set_volume", { level: 0.5 });
  });

  it("clamps the wheel to 0..1", () => {
    renderSystem(sys, view({ volume: vol(0.98) }));
    sys.querySelector(".sys-volume")!.dispatchEvent(new WheelEvent("wheel", { deltaY: -1, bubbles: true }));
    expect(invoke).toHaveBeenLastCalledWith("set_volume", { level: 1 });
  });

  it("clock hidden when showClock=false", () => {
    renderSystem(sys, view());
    expect(sys.querySelector(".sys-clock")!.textContent).toBe("14:05");
    expect((sys.querySelector(".sys-clock") as HTMLElement).title).toMatch(/9/);
    renderSystem(sys, view({ showClock: false }));
    expect(sys.querySelector(".sys-clock")).toBeNull();
  });

  it("middle click toggles mute; bell and network call open_*", () => {
    renderSystem(sys, view());
    sys.querySelector(".sys-volume")!.dispatchEvent(new MouseEvent("auxclick", { button: 1, bubbles: true }));
    expect(invoke).toHaveBeenLastCalledWith("toggle_mute", undefined);
    sys.querySelector<HTMLElement>(".sys-bell")!.click();
    expect(invoke).toHaveBeenLastCalledWith("open_notifications", undefined);
    const net = sys.querySelector<HTMLElement>(".sys-network")!;
    expect(net.title).toBe("Maison");
    net.click();
    expect(invoke).toHaveBeenLastCalledWith("open_network", undefined);
  });

  it("volume click opens a slider popover that calls set_volume", () => {
    const layout = vi.fn();
    sys.addEventListener("traylayout", layout);
    renderSystem(sys, view());
    sys.querySelector<HTMLElement>(".sys-volume")!.click();
    const pop = sys.querySelector(".vol-pop")!;
    expect(pop).not.toBeNull();
    expect(layout).toHaveBeenCalled();
    const slider = pop.querySelector<HTMLInputElement>("input[type=range]")!;
    expect(slider.value).toBe("50");
    slider.value = "30";
    slider.dispatchEvent(new Event("input", { bubbles: true }));
    expect(invoke).toHaveBeenLastCalledWith("set_volume", { level: 0.3 });
    document.body.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    expect(sys.querySelector(".vol-pop")).toBeNull();
  });

  it("network icon reflects kind and keeps nodes", () => {
    renderSystem(sys, view());
    const net = sys.querySelector(".sys-network");
    renderSystem(sys, view({ network: { kind: "none", name: null } }));
    expect(sys.querySelector(".sys-network")).toBe(net);
    expect((net as HTMLElement).dataset.kind).toBe("none");
  });

  it("shows a red slashed mic only while the mic is muted; click unmutes", () => {
    renderSystem(sys, view());
    expect(sys.querySelector(".sys-mic")).toBeNull();
    renderSystem(sys, view({ micMuted: true }));
    const mic = sys.querySelector<HTMLElement>(".sys-mic")!;
    expect(mic).not.toBeNull();
    expect(mic.dataset.muted).toBe("true");
    expect(mic.title).toBe("Micro coupé");
    expect(mic.querySelector("svg")).not.toBeNull();
    // Before the volume icon.
    expect(mic.nextElementSibling).toBe(sys.querySelector(".sys-volume"));
    mic.click();
    expect(invoke).toHaveBeenLastCalledWith("mic_toggle", undefined);
    renderSystem(sys, view({ micMuted: false }));
    expect(sys.querySelector(".sys-mic")).toBeNull();
  });
});

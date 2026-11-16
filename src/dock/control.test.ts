// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AudioDevices, ControlCenterState } from "../shared/types";

const invoke = vi.fn(async (..._args: unknown[]) => undefined);
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({ getCurrentWebviewWindow: () => ({ label: "dock-1" }) }));

const { renderControlCenter } = await import("./control");
const { renderSystem } = await import("./system");
const powerMod = await import("../shared/power");

const cc = (over: Partial<ControlCenterState> = {}): ControlCenterState => ({
  wifi: { on: true, name: "Box" },
  bluetooth: { on: false },
  darkMode: true,
  brightness: 60,
  ...over,
});
const audio = (over: Partial<AudioDevices> = {}): AudioDevices => ({
  outputs: [
    { id: "a", name: "Casque", default: false },
    { id: "b", name: "Haut-parleurs", default: true },
  ],
  micMuted: false,
  ...over,
});

function view(over: Record<string, unknown> = {}) {
  return {
    cc: cc(),
    audio: audio(),
    volume: { level: 0.5, muted: false, icon: "mid" as const },
    network: { kind: "wifi" as const, name: "Maison" },
    ...over,
  };
}

const tile = (root: HTMLElement, name: string) => root.querySelector<HTMLElement>(`[data-tile="${name}"]`);

let panel: HTMLElement;
beforeEach(() => {
  document.body.innerHTML = '<div class="island cc-panel"></div>';
  panel = document.querySelector(".cc-panel")!;
  invoke.mockClear();
});

describe("renderControlCenter", () => {
  it("renders the tiles with their states", () => {
    renderControlCenter(panel, view());
    expect(tile(panel, "wifi")!.getAttribute("aria-pressed")).toBe("true");
    // The live network name wins over the (older) cc state one.
    expect(tile(panel, "wifi")!.textContent).toContain("Maison");
    expect(tile(panel, "bluetooth")!.getAttribute("aria-pressed")).toBe("false");
    expect(tile(panel, "bluetooth")!.textContent).toContain("Désactivé");
    expect(tile(panel, "dark")!.getAttribute("aria-pressed")).toBe("true");
    expect(tile(panel, "mic")!.getAttribute("aria-pressed")).toBe("true");
    expect(tile(panel, "dnd")).not.toBeNull();
    expect(panel.querySelector<HTMLInputElement>('[data-slider="brightness"] input')!.value).toBe("60");
    expect(panel.querySelector<HTMLInputElement>('[data-slider="volume"] input')!.value).toBe("50");
  });

  it("wifi subtitle: not connected when on without a network, off when off", () => {
    renderControlCenter(panel, view({ network: { kind: "ethernet", name: null }, cc: cc({ wifi: { on: true, name: null } }) }));
    expect(tile(panel, "wifi")!.textContent).toContain("Non connecté");
    renderControlCenter(panel, view({ cc: cc({ wifi: { on: false, name: null } }), network: { kind: "none", name: null } }));
    expect(tile(panel, "wifi")!.textContent).toContain("Désactivé");
  });

  it("hides what the machine does not have", () => {
    renderControlCenter(panel, view({ cc: cc({ wifi: null, bluetooth: null, brightness: null }) }));
    expect(tile(panel, "wifi")).toBeNull();
    expect(tile(panel, "bluetooth")).toBeNull();
    expect(panel.querySelector('[data-slider="brightness"]')).toBeNull();
    expect(tile(panel, "mic")).not.toBeNull();
    // Coming back shows them again.
    renderControlCenter(panel, view());
    expect(tile(panel, "wifi")).not.toBeNull();
    expect(panel.querySelector('[data-slider="brightness"]')).not.toBeNull();
  });

  it("before the first cc state: no radio tiles, no brightness", () => {
    renderControlCenter(panel, view({ cc: null }));
    expect(tile(panel, "wifi")).toBeNull();
    expect(panel.querySelector('[data-slider="brightness"]')).toBeNull();
    expect(tile(panel, "dark")!.getAttribute("aria-pressed")).toBe("false");
  });

  it("toggles call the commands and flip optimistically until the state confirms", () => {
    renderControlCenter(panel, view());
    tile(panel, "wifi")!.click();
    expect(invoke).toHaveBeenLastCalledWith("cc_set_wifi", { on: false });
    expect(tile(panel, "wifi")!.getAttribute("aria-pressed")).toBe("false");
    // A render with the stale state keeps the pending value.
    renderControlCenter(panel, view());
    expect(tile(panel, "wifi")!.getAttribute("aria-pressed")).toBe("false");
    tile(panel, "bluetooth")!.click();
    expect(invoke).toHaveBeenLastCalledWith("cc_set_bluetooth", { on: true });
    tile(panel, "dark")!.click();
    expect(invoke).toHaveBeenLastCalledWith("cc_set_dark", { on: false });
    expect(tile(panel, "dark")!.getAttribute("aria-pressed")).toBe("false");
    tile(panel, "mic")!.click();
    expect(invoke).toHaveBeenLastCalledWith("mic_toggle", undefined);
    expect(tile(panel, "mic")!.getAttribute("aria-pressed")).toBe("false");
    // The confirmed state clears the pending value: a later change from outside shows.
    renderControlCenter(panel, view({ cc: cc({ wifi: { on: false, name: null } }) }));
    renderControlCenter(panel, view());
    expect(tile(panel, "wifi")!.getAttribute("aria-pressed")).toBe("true");
  });

  it("muted mic tile is off and says so", () => {
    renderControlCenter(panel, view({ audio: audio({ micMuted: true }) }));
    expect(tile(panel, "mic")!.getAttribute("aria-pressed")).toBe("false");
    expect(tile(panel, "mic")!.textContent).toContain("Coupé");
  });

  it("sliders send brightness (0..100) and volume (0..1)", () => {
    renderControlCenter(panel, view());
    const b = panel.querySelector<HTMLInputElement>('[data-slider="brightness"] input')!;
    b.value = "30";
    b.dispatchEvent(new Event("input", { bubbles: true }));
    expect(invoke).toHaveBeenLastCalledWith("cc_set_brightness", { value: 30 });
    expect(b.closest<HTMLElement>(".cc-slider")!.style.getPropertyValue("--p")).toBe("30%");
    const v = panel.querySelector<HTMLInputElement>('[data-slider="volume"] input')!;
    v.value = "25";
    v.dispatchEvent(new Event("input", { bubbles: true }));
    expect(invoke).toHaveBeenLastCalledWith("set_volume", { level: 0.25 });
    // A state arriving mid-drag does not yank the thumb back.
    renderControlCenter(panel, view());
    expect(v.value).toBe("25");
  });

  it("output list: default checked, click switches, hidden with fewer than two", () => {
    renderControlCenter(panel, view());
    const rows = [...panel.querySelectorAll<HTMLElement>(".cc-output")];
    expect(rows.map((r) => r.textContent)).toEqual(["Casque", "Haut-parleurs"]);
    expect(rows.map((r) => r.getAttribute("aria-checked"))).toEqual(["false", "true"]);
    rows[0].click();
    expect(invoke).toHaveBeenLastCalledWith("audio_set_output", { id: "a" });
    expect(panel.querySelector('.cc-output[data-id="a"]')!.getAttribute("aria-checked")).toBe("true");
    renderControlCenter(panel, view({ audio: audio({ outputs: [{ id: "a", name: "Casque", default: true }] }) }));
    expect(panel.querySelector(".cc-outputs")).toBeNull();
    renderControlCenter(panel, view({ audio: null }));
    expect(panel.querySelector(".cc-outputs")).toBeNull();
  });

  it("do not disturb opens the Windows settings and asks to close the panel", () => {
    const close = vi.fn();
    panel.addEventListener("cc-close", close);
    renderControlCenter(panel, view());
    tile(panel, "dnd")!.click();
    expect(invoke).toHaveBeenLastCalledWith("open_external", { url: "ms-settings:notifications" });
    expect(close).toHaveBeenCalled();
  });

  it("keeps nodes across renders", () => {
    renderControlCenter(panel, view());
    const wifi = tile(panel, "wifi");
    renderControlCenter(panel, view({ cc: cc({ bluetooth: { on: true } }) }));
    expect(tile(panel, "wifi")).toBe(wifi);
  });
});

describe("clock opens the control center", () => {
  let sys: HTMLElement;
  const sysView = (over: Record<string, unknown> = {}) => ({
    volume: { level: 0.5, muted: false, icon: "mid" as const },
    network: { kind: "wifi" as const, name: "Maison" },
    showClock: true,
    now: new Date(2026, 10, 9, 14, 5),
    lang: "fr",
    controlCenter: true,
    cc: cc(),
    audio: audio(),
    ...over,
  });
  beforeEach(() => {
    document.body.innerHTML = '<div class="island tray"><div id="sys"></div></div>';
    sys = document.getElementById("sys")!;
  });

  it("click toggles the panel, refreshes the state and re-lays out", () => {
    const layout = vi.fn();
    sys.addEventListener("traylayout", layout);
    renderSystem(sys, sysView());
    const clock = sys.querySelector<HTMLElement>(".sys-clock")!;
    clock.click();
    expect(sys.querySelector(".island.cc-panel")).not.toBeNull();
    expect(clock.getAttribute("aria-expanded")).toBe("true");
    expect(invoke).toHaveBeenCalledWith("cc_refresh", undefined);
    expect(layout).toHaveBeenCalled();
    clock.click();
    expect(sys.querySelector(".cc-panel")).toBeNull();
  });

  it("closes on an outside click and when the volume popover opens", () => {
    renderSystem(sys, sysView());
    sys.querySelector<HTMLElement>(".sys-clock")!.click();
    document.body.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    expect(sys.querySelector(".cc-panel")).toBeNull();
    sys.querySelector<HTMLElement>(".sys-clock")!.click();
    sys.querySelector<HTMLElement>(".sys-volume")!.click();
    expect(sys.querySelector(".cc-panel")).toBeNull();
    expect(sys.querySelector(".vol-pop")).not.toBeNull();
  });

  it("does nothing when the control center is off in the settings", () => {
    renderSystem(sys, sysView({ controlCenter: false }));
    sys.querySelector<HTMLElement>(".sys-clock")!.click();
    expect(sys.querySelector(".cc-panel")).toBeNull();
  });

  it("DND closes the panel", () => {
    renderSystem(sys, sysView());
    sys.querySelector<HTMLElement>(".sys-clock")!.click();
    sys.querySelector<HTMLElement>('[data-tile="dnd"]')!.click();
    expect(sys.querySelector(".cc-panel")).toBeNull();
  });
});

describe("control center power row", () => {
  const { setPowerExecutor } = powerMod;
  const run = vi.fn(async (_id: string) => undefined);
  const btn = (id: string) => panel.querySelector<HTMLElement>(`[data-power="${id}"]`)!;
  beforeEach(() => {
    run.mockClear();
    setPowerExecutor(run);
  });

  it("shows five buttons and no confirmation at first", () => {
    renderControlCenter(panel, view());
    expect([...panel.querySelectorAll<HTMLElement>("[data-power]")].map((b) => b.dataset.power)).toEqual([
      "lock", "sleep", "restart", "shutdown", "signout",
    ]);
    expect(panel.querySelector<HTMLElement>(".cc-power-confirm")!.hidden).toBe(true);
  });

  it("lock and sleep act at once, without confirmation", () => {
    renderControlCenter(panel, view());
    btn("lock").click();
    btn("sleep").click();
    expect(run.mock.calls.map((c) => c[0])).toEqual(["lock", "sleep"]);
  });

  it("restart asks first; cancel does nothing; confirm executes", () => {
    renderControlCenter(panel, view());
    btn("restart").click();
    expect(run).not.toHaveBeenCalled();
    const confirm = panel.querySelector<HTMLElement>(".cc-power-confirm")!;
    expect(confirm.hidden).toBe(false);
    expect(panel.querySelector<HTMLElement>(".cc-power-row")!.hidden).toBe(true);
    expect(confirm.textContent).toContain("Redémarrer maintenant ? Les applications ouvertes vont se fermer.");
    panel.querySelector<HTMLElement>(".cc-power-cancel")!.click();
    expect(run).not.toHaveBeenCalled();
    expect(confirm.hidden).toBe(true);
    btn("restart").click();
    panel.querySelector<HTMLElement>(".cc-power-go")!.click();
    expect(run.mock.calls.map((c) => c[0])).toEqual(["restart"]);
  });

  it("shutdown and signout are confirmed too, and re-render keeps the confirmation", () => {
    renderControlCenter(panel, view());
    btn("shutdown").click();
    renderControlCenter(panel, view());
    expect(panel.querySelector<HTMLElement>(".cc-power-confirm")!.hidden).toBe(false);
    expect(panel.querySelector(".cc-power-go")!.textContent).toBe("Éteindre");
    panel.querySelector<HTMLElement>(".cc-power-cancel")!.click();
    btn("signout").click();
    expect(panel.querySelector(".cc-power-text")!.textContent).toContain("Se déconnecter maintenant");
    expect(run).not.toHaveBeenCalled();
  });

  it("shared monitor: a switch row in the Écran module only when configured", () => {
    const panel = document.createElement("div");
    renderControlCenter(panel, view({ cc: cc({ brightness: null }) }));
    expect(panel.querySelector('[data-tile="screen"]')).toBeNull();
    renderControlCenter(panel, view({ cc: cc({ brightness: null }), display: { monitor: "24E4", label: "PC portable" } }));
    const row = panel.querySelector<HTMLElement>('[data-tile="screen"]')!;
    expect(row.querySelector(".cc-label")!.textContent).toBe("Basculer vers PC portable");
    expect(panel.querySelector<HTMLElement>("[data-slider]")!.hidden).toBe(true);
    row.click();
    expect(invoke).toHaveBeenCalledWith("display_switch", {});
  });
});

// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GameState, SysStats } from "../shared/types";

const invoke = vi.fn(async (cmd: string, ..._rest: unknown[]) => (cmd.startsWith("wsl_") ? `résultat ${cmd}` : undefined));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({ getCurrentWebviewWindow: () => ({ label: "dock-1" }) }));

const { formatStats, showPill, renderPill, closePillPanel } = await import("./pill");

function stats(over: Partial<SysStats> = {}): SysStats {
  return { cpu: 23.4, ram: { used: 11469, total: 32607 }, gpu: 41, gpuTemp: 62, pingMs: 18, wslRamMb: null, ...over };
}

describe("formatStats", () => {
  it("formats every value with French decimals", () => {
    expect(formatStats(stats())).toBe("CPU 23 % · GPU 41 % 62° · RAM 11,2/32 Go · 18 ms");
    expect(formatStats(stats({ wslRamMb: 3174 }))).toBe("CPU 23 % · GPU 41 % 62° · RAM 11,2/32 Go · 18 ms · WSL 3,1 Go");
  });

  it("omits what is not available", () => {
    expect(formatStats(stats({ gpu: null, gpuTemp: null, pingMs: null }))).toBe("CPU 23 % · RAM 11,2/32 Go");
    expect(formatStats(stats({ gpuTemp: null }))).toBe("CPU 23 % · GPU 41 % · RAM 11,2/32 Go · 18 ms");
    expect(formatStats(stats({ gpu: null }))).toBe("CPU 23 % · GPU 62° · RAM 11,2/32 Go · 18 ms");
  });

  it("has a placeholder before the first sample", () => {
    expect(formatStats(null)).toBe("CPU – % · RAM –");
  });
});

describe("showPill", () => {
  const a = "\\\\.\\DISPLAY1";
  const b = "\\\\.\\DISPLAY2";
  const game = (active: boolean, monitorId: string | null = a): GameState => ({ active, name: active ? "Valorant" : null, monitorId, focused: true });

  it("only on another monitor than the game's, with the setting on, while the dock is hidden", () => {
    expect(showPill({ game: game(true), monitorId: b, monitorPill: true, revealed: false })).toBe(true);
    expect(showPill({ game: game(true), monitorId: a, monitorPill: true, revealed: false })).toBe(false);
    expect(showPill({ game: game(true), monitorId: b, monitorPill: false, revealed: false })).toBe(false);
    expect(showPill({ game: game(true), monitorId: b, monitorPill: true, revealed: true })).toBe(false);
    expect(showPill({ game: game(false), monitorId: b, monitorPill: true, revealed: false })).toBe(false);
    expect(showPill({ game: null, monitorId: b, monitorPill: true, revealed: false })).toBe(false);
  });
});

describe("renderPill", () => {
  let el: HTMLElement;
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = '<div id="dock"><div class="island pill"></div></div>';
    el = document.querySelector<HTMLElement>(".island.pill")!;
    invoke.mockClear();
  });
  afterEach(() => {
    closePillPanel(el);
    vi.useRealTimers();
  });

  const line = () => el.querySelector<HTMLButtonElement>(".pill-line")!;
  const panel = () => el.querySelector<HTMLElement>(".pill-panel");
  const btn = (cls: string) => panel()!.querySelector<HTMLButtonElement>(cls)!;

  it("shows the stats line and toggles the panel on click", () => {
    const changed = vi.fn();
    el.addEventListener("pilllayout", changed);
    renderPill(el, stats());
    expect(line().textContent).toBe("CPU 23 % · GPU 41 % 62° · RAM 11,2/32 Go · 18 ms");
    expect(panel()).toBeNull();
    line().click();
    expect(panel()).not.toBeNull();
    expect(changed).toHaveBeenCalledTimes(1);
    expect(btn(".pill-free").textContent).toBe("Libérer la RAM WSL");
    expect(btn(".pill-stop").textContent).toBe("Arrêter WSL…");
    line().click();
    expect(panel()).toBeNull();
  });

  it("keeps the panel nodes across stats updates", () => {
    renderPill(el, stats());
    line().click();
    const free = btn(".pill-free");
    renderPill(el, stats({ cpu: 50 }));
    expect(btn(".pill-free")).toBe(free);
    expect(line().textContent).toContain("CPU 50 %");
  });

  it("frees WSL memory in one click and shows the result for 4 s", async () => {
    renderPill(el, stats());
    line().click();
    btn(".pill-free").click();
    expect(invoke).toHaveBeenCalledWith("wsl_free", undefined);
    await vi.advanceTimersByTimeAsync(0);
    expect(panel()!.querySelector(".pill-result")!.textContent).toBe("résultat wsl_free");
    await vi.advanceTimersByTimeAsync(4000);
    expect(panel()!.querySelector(".pill-result")!.textContent).toBe("");
  });

  it("asks for confirmation before stopping WSL", async () => {
    renderPill(el, stats());
    line().click();
    const confirm = () => panel()!.querySelector<HTMLElement>(".pill-confirm")!;
    expect(confirm().hidden).toBe(true);
    btn(".pill-stop").click();
    expect(invoke).not.toHaveBeenCalled();
    expect(confirm().hidden).toBe(false);
    expect(confirm().querySelector(".pill-confirm-text")!.textContent).toBe(
      "Arrêter WSL ? Les agents et processus en cours seront coupés.",
    );
    // Cancel: nothing happens.
    btn(".pill-cancel").click();
    expect(confirm().hidden).toBe(true);
    expect(invoke).not.toHaveBeenCalled();
    // Second click on « Arrêter » after the warning: shut down.
    btn(".pill-stop").click();
    expect(btn(".pill-cancel").textContent).toBe("Annuler");
    expect(btn(".pill-shutdown").textContent).toBe("Arrêter");
    btn(".pill-shutdown").click();
    expect(invoke).toHaveBeenCalledWith("wsl_shutdown", undefined);
    expect(invoke).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(0);
    expect(confirm().hidden).toBe(true);
    expect(panel()!.querySelector(".pill-result")!.textContent).toBe("résultat wsl_shutdown");
  });

  it("closes on a click outside", () => {
    renderPill(el, stats());
    line().click();
    document.body.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    expect(panel()).toBeNull();
  });
});

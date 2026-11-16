// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TrayIconView } from "../shared/types";

const invoke = vi.fn(async (..._args: unknown[]) => undefined);
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({ getCurrentWebviewWindow: () => ({ label: "dock-1" }) }));

const { renderTray, closeTrayPanel } = await import("./tray");

function icon(key: string, promoted: boolean): TrayIconView {
  return { key, tip: `tip ${key}`, iconUrl: `http://icon.localhost/tray-${key}.png?v=1`, promoted };
}

let container: HTMLElement;
beforeEach(() => {
  document.body.innerHTML = '<div id="tray"></div>';
  container = document.getElementById("tray")!;
  invoke.mockClear();
});

const inline = () => [...container.querySelectorAll<HTMLElement>(":scope > .tray-icon")].map((b) => b.dataset.key);
const panel = () => container.querySelector<HTMLElement>(".tray-panel");
const chevron = () => container.querySelector<HTMLButtonElement>(".tray-chevron");

describe("renderTray", () => {
  it("puts non-promoted icons behind the chevron", () => {
    renderTray(container, [icon("a", true), icon("b", false), icon("c", false)]);
    expect(inline()).toEqual(["a"]);
    expect(chevron()).not.toBeNull();
    expect(panel()).toBeNull(); // closed until the chevron is used
    chevron()!.click();
    const hidden = [...panel()!.querySelectorAll<HTMLElement>(".tray-icon")].map((b) => b.dataset.key);
    expect(hidden).toEqual(["b", "c"]);
    // Nothing hidden: the chevron stays (the panel still holds the Lucent cell).
    renderTray(container, [icon("a", true)]);
    expect(chevron()).not.toBeNull();
    expect(panel()!.querySelectorAll(".tray-icon").length).toBe(0);
    expect(panel()!.lastElementChild!.classList.contains("tray-lucent")).toBe(true);
    expect(inline()).toEqual(["a"]);
  });

  it("chevron toggles the overflow panel", () => {
    const layout = vi.fn();
    container.addEventListener("traylayout", layout);
    renderTray(container, [icon("b", false)]);
    chevron()!.click();
    expect(panel()).not.toBeNull();
    expect(chevron()!.getAttribute("aria-expanded")).toBe("true");
    // The panel stays open across icon updates.
    renderTray(container, [icon("b", false), icon("d", false)]);
    expect(panel()!.querySelectorAll(".tray-icon").length).toBe(2);
    expect(panel()!.lastElementChild!.classList.contains("tray-lucent")).toBe(true);
    chevron()!.click();
    expect(panel()).toBeNull();
    expect(chevron()!.getAttribute("aria-expanded")).toBe("false");
    expect(layout).toHaveBeenCalledTimes(2);
    // Click outside closes it; a selection closes it too.
    chevron()!.click();
    document.body.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    expect(panel()).toBeNull();
    chevron()!.click();
    panel()!.querySelector<HTMLElement>(".tray-icon")!.click();
    expect(panel()).toBeNull();
    chevron()!.click();
    closeTrayPanel(container);
    expect(panel()).toBeNull();
  });

  it("right click sends button=right with icon center", () => {
    vi.stubGlobal("devicePixelRatio", 1.5);
    renderTray(container, [icon("a", true)]);
    const btn = container.querySelector<HTMLElement>('.tray-icon[data-key="a"]')!;
    btn.getBoundingClientRect = () => ({ left: 10, top: 20, width: 24, height: 24, right: 34, bottom: 44, x: 10, y: 20, toJSON: () => ({}) });
    const ev = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    btn.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(true);
    expect(invoke).toHaveBeenCalledWith("tray_click", { key: "a", button: "right", x: 33, y: 48 });
    invoke.mockClear();
    btn.dispatchEvent(new MouseEvent("click", { bubbles: true, detail: 1 }));
    btn.dispatchEvent(new MouseEvent("click", { bubbles: true, detail: 2 }));
    btn.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, detail: 2 }));
    expect(invoke.mock.calls.map((c) => (c[1] as { button: string }).button)).toEqual(["left", "double"]);
    vi.unstubAllGlobals();
  });

  it("reuses icon buttons and updates src/tooltip", () => {
    renderTray(container, [icon("a", true)]);
    const btn = container.querySelector('.tray-icon[data-key="a"]');
    const next = { ...icon("a", true), tip: "new", iconUrl: "http://icon.localhost/tray-a.png?v=2" };
    renderTray(container, [next]);
    const again = container.querySelector<HTMLElement>('.tray-icon[data-key="a"]')!;
    expect(again).toBe(btn);
    expect(again.title).toBe("new");
    expect(again.querySelector("img")!.getAttribute("src")).toBe("http://icon.localhost/tray-a.png?v=2");
  });

  it("chevron is shown with zero hidden icons, panel ends with the Lucent cell", () => {
    renderTray(container, []);
    expect(chevron()).not.toBeNull();
    chevron()!.click();
    const cells = [...panel()!.children] as HTMLElement[];
    expect(cells.length).toBe(1);
    const gd = cells[0];
    expect(gd.classList.contains("tray-lucent")).toBe(true);
    expect(gd.getAttribute("aria-label")).toBe("Réglages de Lucent");
    expect(gd.title).toBe("Réglages de Lucent");
    expect(gd.querySelector("img")!.getAttribute("src")).toBe("/lucent.png");
    // Hidden icons are added before it; it stays last (and is reused).
    renderTray(container, [icon("b", false), icon("c", false)]);
    const after = [...panel()!.children] as HTMLElement[];
    expect(after.map((el) => el.dataset.key ?? el.className)).toEqual(["b", "c", "tray-lucent"]);
    expect(after[2]).toBe(gd);
  });

  it("Lucent cell: left click opens settings, right click opens the dock menu", () => {
    renderTray(container, [icon("b", false)]);
    chevron()!.click();
    container.querySelector<HTMLElement>(".tray-lucent")!.dispatchEvent(new MouseEvent("click", { bubbles: true, detail: 1 }));
    expect(invoke.mock.calls.map((c) => c[0])).toEqual(["open_settings"]);
    expect(invoke.mock.calls.some((c) => c[0] === "tray_click")).toBe(false);
    expect(panel()).toBeNull(); // the panel closes
    invoke.mockClear();
    chevron()!.click();
    const ev = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    container.querySelector<HTMLElement>(".tray-lucent")!.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(true);
    expect(invoke).toHaveBeenCalledWith("dock_menu", { itemId: null });
    expect(invoke.mock.calls.some((c) => c[0] === "tray_click")).toBe(false);
    expect(panel()).toBeNull();
  });
});

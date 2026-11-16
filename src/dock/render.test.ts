// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import { isFullBleed, renderDock, type DockViewState } from "./render";
import type { DockItem, Settings } from "../shared/types";

function settings(): Settings {
  return {
    schemaVersion: 1,
    dock: {
      iconSize: 48,
      magnification: true,
      magnificationScale: 1.6,
      position: "bottom",
      revealDelayMs: 150,
      hideDelayMs: 400,
      monitors: "all",
      glass: "auto",
      pinned: [],
      showClock: true,
      showMedia: true,
      alwaysVisible: false,
    },
    animations: { speed: 1, style: "spring", cardDurationMs: 4000, reduceMotion: false },
    general: { launchAtStartup: true, hideWindowsTaskbar: true, language: "fr", checkUpdates: true },
    search: { hotkey: "Alt+Space", projectRoots: [], wslDistro: "", clipboardHistory: true, webUrl: "https://www.google.com/search?q=%s", textColor: "" },
    gameMode: { enabled: true, include: [], exclude: [], powerPlan: true, monitorPill: true, pingTarget: "1.1.1.1" },
    valorant: { riotId: "", region: "eu", apiKey: "", postMatch: true, pinCard: false },
    shortcuts: { switcher: "Alt+²", overview: "Ctrl+Alt+Space", snapping: true, micMute: "Ctrl+Alt+M", displaySwitch: "Ctrl+Alt+O", snap: { left: "Ctrl+Alt+Left", right: "Ctrl+Alt+Right", max: "Ctrl+Alt+Up", center: "Ctrl+Alt+Down", third1: "Ctrl+Alt+D", third2: "Ctrl+Alt+F", third3: "Ctrl+Alt+G", prevMonitor: "Ctrl+Alt+Shift+Left", nextMonitor: "Ctrl+Alt+Shift+Right" } },
    hotCorners: { topLeft: "overview", topRight: "none", bottomLeft: "none", bottomRight: "none" },
    controlCenter: true,
    quickLook: true,
  displaySwitch: { monitor: "", here: 1, other: 3, otherLabel: "Autre PC", detach: true },
  };
}

function item(id: string, isGame = false, windows = 0): DockItem {
  return {
    id,
    app: { kind: "exe", value: `c:\\${id}.exe` },
    name: id,
    iconUrl: `http://icon.localhost/${id}.png`,
    pinned: true,
    isGame,
    windows: Array.from({ length: windows }, (_, i) => ({ hwnd: i + 1, title: `${id} ${i}` })),
  };
}

function state(over: Partial<DockViewState> = {}): DockViewState {
  return { items: [], tray: [], system: { volume: null, network: { kind: "none", name: null } }, media: null, settings: settings(), tone: "light", now: new Date(2026, 10, 9, 14, 5), ...over };
}

let root: HTMLElement;
beforeEach(() => {
  document.body.innerHTML = '<div id="dock"></div>';
  root = document.getElementById("dock")!;
});

describe("renderDock", () => {
  it("renders games after a separator", () => {
    renderDock(root, state({ items: [item("a"), item("g", true), item("b")] }));
    const apps = root.querySelector(".island.apps")!;
    const seq = [...apps.children].map((c) => (c.classList.contains("sep") ? "sep" : (c as HTMLElement).dataset.id));
    expect(seq).toEqual(["a", "b", "sep", "g"]);
  });

  it("has no separator without games", () => {
    renderDock(root, state({ items: [item("a")] }));
    expect(root.querySelector(".sep")).toBeNull();
  });

  it("shows a dot only for running items", () => {
    renderDock(root, state({ items: [item("a", false, 1), item("b")] }));
    expect(root.querySelector('.app[data-id="a"] .dot')).not.toBeNull();
    expect(root.querySelector('.app[data-id="b"] .dot')).toBeNull();
  });

  it("has exactly one clock, in the tray island, honouring showClock", () => {
    renderDock(root, state());
    expect(root.querySelectorAll(".sys-clock").length).toBe(1);
    expect(root.querySelector(".island.tray .sys-clock")).not.toBeNull();
    expect(root.querySelector(".island.clock")).toBeNull();
    const s = settings();
    s.dock.showClock = false;
    renderDock(root, state({ settings: s }));
    expect(root.querySelector(".sys-clock")).toBeNull();
  });

  it("marks app buttons with data-pinned", () => {
    renderDock(root, state({ items: [item("a"), { ...item("u"), pinned: false }] }));
    expect(root.querySelector<HTMLElement>('.app[data-id="a"]')!.dataset.pinned).toBe("true");
    expect(root.querySelector<HTMLElement>('.app[data-id="u"]')!.dataset.pinned).toBe("false");
    renderDock(root, state({ items: [{ ...item("a"), pinned: false }, item("u")] }));
    expect(root.querySelector<HTMLElement>('.app[data-id="a"]')!.dataset.pinned).toBe("false");
    expect(root.querySelector<HTMLElement>('.app[data-id="u"]')!.dataset.pinned).toBe("true");
  });

  it("uses icon size from settings", () => {
    const s = settings();
    s.dock.iconSize = 64;
    renderDock(root, state({ items: [item("a")], settings: s }));
    const img = root.querySelector<HTMLImageElement>('.app[data-id="a"] img')!;
    expect(img.width).toBe(64);
    expect(img.getAttribute("src")).toBe("http://icon.localhost/a.png");
  });

  it("orders islands apps, tray and keeps #tray stable", () => {
    renderDock(root, state({ items: [item("a")] }));
    const tray = root.querySelector("#tray")!;
    tray.appendChild(document.createElement("span"));
    renderDock(root, state({ items: [item("a"), item("b")] }));
    const islands = [...root.querySelectorAll(".island")].map((e) => e.classList[1]);
    expect(islands).toEqual(["apps", "tray"]);
    expect(root.querySelector("#tray")).toBe(tray);
    expect(tray.childElementCount).toBe(2); // the (always present) chevron + the span
  });

  it("adds the system pill after the tray without moving apps/tray, and removes it", () => {
    const stats = { cpu: 5, ram: { used: 2048, total: 16384 }, gpu: null, gpuTemp: null, pingMs: null, wslRamMb: null };
    renderDock(root, state({ items: [item("a")] }));
    const apps = root.querySelector(".island.apps");
    const tray = root.querySelector(".island.tray");
    renderDock(root, state({ items: [item("a")], pill: { stats } }));
    expect([...root.querySelectorAll(":scope > .island")].map((e) => e.classList[1])).toEqual(["apps", "tray", "pill"]);
    expect(root.querySelector(".island.apps")).toBe(apps);
    expect(root.querySelector(".island.tray")).toBe(tray);
    expect(root.classList.contains("pill-mode")).toBe(true);
    expect(root.querySelector(".pill-line")!.textContent).toBe("CPU 5 % · RAM 2,0/16 Go");
    renderDock(root, state({ items: [item("a")], pill: null }));
    expect(root.querySelector(".island.pill")).toBeNull();
    expect(root.classList.contains("pill-mode")).toBe(false);
    expect([...root.querySelectorAll(":scope > .island")].map((e) => e.classList[1])).toEqual(["apps", "tray"]);
  });

  it("reuses item buttons across renders", () => {
    renderDock(root, state({ items: [item("a"), item("b")] }));
    const a = root.querySelector('.app[data-id="a"]');
    renderDock(root, state({ items: [item("b"), item("a", false, 1)] }));
    expect(root.querySelector('.app[data-id="a"]')).toBe(a);
    expect([...root.querySelectorAll<HTMLElement>(".app")].map((b) => b.dataset.id)).toEqual(["b", "a"]);
  });

  it("fills #tray with the tray icons and keeps only the chevron when empty", () => {
    const icon = (key: string, promoted: boolean) => ({ key, tip: key, iconUrl: `http://icon.localhost/tray-${key}.png?v=1`, promoted });
    renderDock(root, state({ tray: [icon("a", true), icon("b", false)] }));
    const tray = root.querySelector("#tray")!;
    expect(tray.querySelector('.tray-icon[data-key="a"]')).not.toBeNull();
    expect(tray.querySelector(".tray-chevron")).not.toBeNull();
    renderDock(root, state({ tray: [] }));
    // The chevron stays: its panel always holds the Lucent (settings) cell.
    expect(tray.childElementCount).toBe(1);
    expect(tray.querySelector(".tray-chevron")).not.toBeNull();
  });

  it("applies the tone to :root and shows the time", () => {
    renderDock(root, state({ tone: "dark" }));
    expect(document.documentElement.dataset.tone).toBe("dark");
    expect(root.querySelector(".sys-clock")!.textContent).toContain("14:05");
  });

  it("applies bouncing class when isBouncing predicate is true", () => {
    renderDock(root, state({ items: [item("app-1"), item("app-2")], bouncing: (id) => id === "app-1" }));
    const b1 = root.querySelector('.app[data-id="app-1"]')!;
    const b2 = root.querySelector('.app[data-id="app-2"]')!;
    expect(b1.classList.contains("bouncing")).toBe(true);
    expect(b2.classList.contains("bouncing")).toBe(false);
  });

  it("renders notification badge when item.badge is set, and removes when cleared", () => {
    const itemWithBadge = { ...item("discord"), badge: "3" };
    renderDock(root, state({ items: [itemWithBadge] }));
    const btn = root.querySelector('.app[data-id="discord"]')!;
    const badge = btn.querySelector(".badge-count");
    expect(badge).not.toBeNull();
    expect(badge!.textContent).toBe("3");

    // Re-render without badge
    const itemWithoutBadge = { ...item("discord"), badge: null };
    renderDock(root, state({ items: [itemWithoutBadge] }));
    expect(btn.querySelector(".badge-count")).toBeNull();
  });

  it("renders downloads stack and trash when requested", () => {
    renderDock(root, state({ items: [item("a")], showDownloads: true, trash: { count: 1, bytes: 100 } }));
    const apps = root.querySelector(".island.apps")!;
    expect(apps.querySelector(".app.stack")).not.toBeNull();
    expect(apps.querySelector(".app.trash")).not.toBeNull();
    expect(apps.querySelector(".sep.utility-sep")).not.toBeNull();
  });
});

describe("isFullBleed", () => {
  const bitmap = (size: number, opaque: (x: number, y: number) => boolean) => {
    const d = new Uint8ClampedArray(size * size * 4);
    for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) d[(y * size + x) * 4 + 3] = opaque(x, y) ? 255 : 0;
    return d;
  };
  it("accepts an opaque square", () => {
    expect(isFullBleed(bitmap(32, () => true), 32)).toBe(true);
  });
  it("rejects a circle", () => {
    expect(isFullBleed(bitmap(32, (x, y) => (x - 15.5) ** 2 + (y - 15.5) ** 2 < 16 ** 2), 32)).toBe(false);
  });
  it("rejects a shape with one transparent corner", () => {
    expect(isFullBleed(bitmap(32, (x, y) => !(x > 24 && y < 8)), 32)).toBe(false);
  });
});

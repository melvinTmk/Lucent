// @vitest-environment jsdom
// Per-window events must be subscribed with this window as target: a global `listen()`
// registers an `Any` listener, which Tauri feeds with events emitted to every window.
import { describe, expect, it, vi } from "vitest";

const listen = vi.fn(async (..._args: unknown[]) => () => {});
const invoke = vi.fn(async (_cmd: string) =>
  _cmd === "get_dock_items" || _cmd === "get_tray_icons"
    ? []
    : _cmd === "get_system_state"
      ? { volume: null, network: { kind: "none", name: null } }
      : null,
);

vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "dock-2", onDragDropEvent: async () => () => {} }),
}));

describe("dock page subscriptions", () => {
  it("targets this window for tone/reveal and listens globally for items/settings/tray", async () => {
    document.body.innerHTML = '<div id="frame"><div id="dock"></div></div>';
    await import("./main");
    await vi.waitFor(() => expect(listen).toHaveBeenCalledTimes(18));
    const opts = Object.fromEntries(listen.mock.calls.map((c) => [c[0] as string, c[2]]));
    const own = { target: { kind: "WebviewWindow", label: "dock-2" } };
    expect(opts["dock://tone"]).toEqual(own);
    expect(opts["dock://reveal"]).toEqual(own);
    expect(opts["dock://items"]).toBeUndefined();
    expect(opts["settings://changed"]).toBeUndefined();
    expect(opts["tray://icons"]).toBeUndefined();
    expect(opts["system://volume"]).toBeUndefined();
    expect(opts["system://network"]).toBeUndefined();
    expect(listen.mock.calls.map((c) => c[0])).toContain("media://state");
    expect(opts["media://state"]).toBeUndefined();
    expect(listen.mock.calls.map((c) => c[0])).toContain("tray://icons");
    expect(listen.mock.calls.map((c) => c[0])).toContain("game://state");
    expect(opts["game://state"]).toBeUndefined();
    expect(listen.mock.calls.map((c) => c[0])).toContain("sysmon://stats");
    expect(opts["sysmon://stats"]).toBeUndefined();
    expect(listen.mock.calls.map((c) => c[0])).toContain("valo://match");
    expect(opts["valo://match"]).toBeUndefined();
    expect(listen.mock.calls.map((c) => c[0])).toContain("game://ask");
    expect(listen.mock.calls.map((c) => c[0])).toContain("cc://state");
    expect(listen.mock.calls.filter((c) => c[0] === "audio://devices")).toHaveLength(1);
    expect(opts["game://ask"]).toBeUndefined();
    expect(listen.mock.calls.map((c) => c[0])).toContain("audio://devices");
    expect(opts["audio://devices"]).toBeUndefined();
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("get_system_state", undefined));
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("get_tray_icons", undefined));
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("get_media", undefined));
  });
});

describe("media update during a reorder drag", () => {
  const handler = (event: string) => {
    const call = listen.mock.calls.find((c) => c[0] === event);
    return (payload: unknown) => (call![1] as (e: { payload: unknown }) => void)({ payload });
  };
  const pointer = (type: string, x: number, buttons: number) => {
    const Ctor = (globalThis as { PointerEvent?: typeof MouseEvent }).PointerEvent ?? MouseEvent;
    const e = new Ctor(type, { bubbles: true, cancelable: true, clientX: x, button: 0, buttons, pointerId: 1 } as MouseEventInit);
    if (!("pointerId" in e)) Object.defineProperty(e, "pointerId", { value: 1 });
    return e;
  };

  it("keeps the islands still until the drop, then shows the media", async () => {
    await import("./main");
    await vi.waitFor(() => expect(listen).toHaveBeenCalledTimes(18));
    const settings = {
      schemaVersion: 1,
      dock: {
        iconSize: 36, magnification: false, magnificationScale: 1.6, position: "bottom",
        revealDelayMs: 150, hideDelayMs: 400, monitors: "all", glass: "auto", pinned: [],
        showClock: true, showMedia: true, alwaysVisible: false,
      },
      animations: { speed: 1, style: "spring", cardDurationMs: 4000, reduceMotion: false },
      general: { launchAtStartup: true, hideWindowsTaskbar: true, language: "fr", checkUpdates: true },
      search: { hotkey: "Alt+Space", projectRoots: [], wslDistro: "", clipboardHistory: true, webUrl: "", textColor: "" },
    };
    const item = (id: string) => ({
      id, app: { kind: "exe", value: `c:\\${id}.exe` }, name: id, iconUrl: `http://icon.localhost/${id}.png`,
      pinned: true, isGame: false, windows: [],
    });
    handler("settings://changed")(settings);
    handler("dock://items")([item("a"), item("b")]);
    const a = document.querySelector<HTMLElement>('.app[data-id="a"]')!;
    a.dispatchEvent(pointer("pointerdown", 0, 1));
    a.dispatchEvent(pointer("pointermove", 20, 1));
    handler("media://state")({ title: "Song", artist: "Band", artUrl: null, playing: true, canPrev: true, canNext: true, canPlayPause: true });
    expect(document.querySelector(".island.media")).toBeNull();
    a.dispatchEvent(pointer("pointerup", 20, 0));
    expect(document.querySelector(".island.media")).not.toBeNull();
  });
});

describe("game mode", () => {
  const handler = (event: string) => {
    const call = listen.mock.calls.find((c) => c[0] === event);
    return (payload: unknown) => (call![1] as (e: { payload: unknown }) => void)({ payload });
  };

  it("marks a non-game monitor and keeps magnification off there", async () => {
    await import("./main");
    await vi.waitFor(() => expect(listen).toHaveBeenCalledTimes(18));
    const settings = {
      schemaVersion: 1,
      dock: {
        iconSize: 36, magnification: true, magnificationScale: 1.6, position: "bottom",
        revealDelayMs: 150, hideDelayMs: 400, monitors: "all", glass: "auto", pinned: [],
        showClock: false, showMedia: false, alwaysVisible: false,
      },
      animations: { speed: 1, style: "spring", cardDurationMs: 4000, reduceMotion: false },
      general: { launchAtStartup: true, hideWindowsTaskbar: true, language: "fr", checkUpdates: true },
      search: { hotkey: "Alt+Space", projectRoots: [], wslDistro: "", clipboardHistory: true, webUrl: "", textColor: "" },
    };
    handler("settings://changed")(settings);
    handler("dock://items")([
      { id: "a", app: { kind: "exe", value: "c:\\a.exe" }, name: "a", iconUrl: "http://icon.localhost/a.png", pinned: true, isGame: false, windows: [] },
    ]);
    // This page has no ?monitor=: its id is "". The game runs elsewhere.
    handler("game://state")({ active: true, name: "Valorant", monitorId: "\\\\.\\DISPLAY2", focused: true });
    expect(document.body.dataset.game).toBe("other");
    const app = document.querySelector<HTMLElement>('.app[data-id="a"]')!;
    app.dispatchEvent(new MouseEvent("pointermove", { bubbles: true, clientX: 0 }));
    await new Promise((r) => setTimeout(r, 50));
    expect(app.style.width).toBe("");

    handler("game://state")({ active: true, name: "Valorant", monitorId: "", focused: true });
    expect(document.body.dataset.game).toBe("game");
    handler("game://state")({ active: false, name: null, monitorId: null, focused: false });
    expect(document.body.hasAttribute("data-game")).toBe(false);
  });
});

describe("system pill", () => {
  const handler = (event: string) => {
    const call = listen.mock.calls.find((c) => c[0] === event);
    return (payload: unknown) => (call![1] as (e: { payload: unknown }) => void)({ payload });
  };

  it("shows the pill on a non-game monitor while the dock is hidden, and reports it", async () => {
    await import("./main");
    await vi.waitFor(() => expect(listen).toHaveBeenCalledTimes(18));
    const settings = {
      schemaVersion: 1,
      dock: {
        iconSize: 36, magnification: false, magnificationScale: 1.6, position: "bottom",
        revealDelayMs: 150, hideDelayMs: 400, monitors: "all", glass: "auto", pinned: [],
        showClock: false, showMedia: false, alwaysVisible: false,
      },
      animations: { speed: 1, style: "spring", cardDurationMs: 4000, reduceMotion: false },
      general: { launchAtStartup: true, hideWindowsTaskbar: true, language: "fr", checkUpdates: true },
      search: { hotkey: "Alt+Space", projectRoots: [], wslDistro: "", clipboardHistory: true, webUrl: "", textColor: "" },
      gameMode: { enabled: true, include: [], exclude: [], powerPlan: true, monitorPill: true, pingTarget: "1.1.1.1" },
    };
    handler("settings://changed")(settings);
    handler("dock://reveal")({ shown: false });
    handler("game://state")({ active: true, name: "Valorant", monitorId: "\\\\.\\DISPLAY2", focused: true });
    expect(document.querySelector(".island.pill")).not.toBeNull();
    const layout = () => (invoke.mock.calls as unknown[][]).filter((c) => c[0] === "report_dock_layout").slice(-1)[0];
    expect((layout()?.[1] as { pill: boolean }).pill).toBe(true);
    handler("sysmon://stats")({ cpu: 12, ram: { used: 4096, total: 16384 }, gpu: null, gpuTemp: null, pingMs: 9, wslRamMb: null });
    expect(document.querySelector(".pill-line")!.textContent).toBe("CPU 12 % · RAM 4,0/16 Go · 9 ms");
    // The dock reveals: it takes over, the pill goes.
    handler("dock://reveal")({ shown: true });
    expect(document.querySelector(".island.pill")).toBeNull();
    expect((layout()?.[1] as { pill: boolean }).pill).toBe(false);
    handler("dock://reveal")({ shown: false });
    expect(document.querySelector(".island.pill")).not.toBeNull();
    handler("settings://changed")({ ...settings, gameMode: { ...settings.gameMode, monitorPill: false } });
    expect(document.querySelector(".island.pill")).toBeNull();
    handler("settings://changed")(settings);
    handler("game://state")({ active: false, name: null, monitorId: null, focused: false });
    expect(document.querySelector(".island.pill")).toBeNull();
  });
});

describe("mic mute", () => {
  const handler = (event: string) => {
    const call = listen.mock.calls.find((c) => c[0] === event);
    return (payload: unknown) => (call![1] as (e: { payload: unknown }) => void)({ payload });
  };

  it("shows the red mic glyph and the « Micro coupé » / « Micro activé » card on change", async () => {
    await import("./main");
    await vi.waitFor(() => expect(listen).toHaveBeenCalledTimes(18));
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("get_media", undefined));
    expect(document.querySelector(".sys-mic")).toBeNull();
    handler("audio://devices")({ outputs: [], micMuted: true });
    expect(document.querySelector(".sys-mic")).not.toBeNull();
    expect(document.querySelector(".island.card.mic")!.textContent).toBe("Micro coupé");
    handler("audio://devices")({ outputs: [], micMuted: false });
    expect(document.querySelector(".sys-mic")).toBeNull();
    expect(document.querySelector(".island.card.mic")!.textContent).toBe("Micro activé");
  });
});

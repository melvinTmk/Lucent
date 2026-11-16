// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn(async (..._args: unknown[]) => undefined) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({ getCurrentWebviewWindow: () => ({ label: "dock-1" }) }));

import { renderMedia } from "./media";
import { renderDock, type DockViewState } from "./render";
import type { MediaState, Settings } from "../shared/types";

const ART = "data:image/png;base64,AAAA";

function media(over: Partial<MediaState> = {}): MediaState {
  return { title: "Song", artist: "Band", artUrl: ART, playing: true, canPrev: true, canNext: true, canPlayPause: true, ...over };
}

function settings(showMedia = true): Settings {
  return {
    schemaVersion: 1,
    dock: {
      iconSize: 36, magnification: true, magnificationScale: 1.6, position: "bottom",
      revealDelayMs: 150, hideDelayMs: 400, monitors: "all", glass: "auto", pinned: [],
      showClock: true, showMedia, alwaysVisible: false,
    },
    animations: { speed: 1, style: "spring", cardDurationMs: 4000, reduceMotion: false },
    general: { launchAtStartup: true, hideWindowsTaskbar: true, language: "fr", checkUpdates: true },
    search: { hotkey: "Alt+Space", projectRoots: [], wslDistro: "", clipboardHistory: true, webUrl: "", textColor: "" },
    gameMode: { enabled: true, include: [], exclude: [], powerPlan: true, monitorPill: true, pingTarget: "1.1.1.1" },
    valorant: { riotId: "", region: "eu", apiKey: "", postMatch: true, pinCard: false },
    shortcuts: { switcher: "Alt+²", overview: "Ctrl+Alt+Space", snapping: true, micMute: "Ctrl+Alt+M", displaySwitch: "Ctrl+Alt+O", snap: { left: "Ctrl+Alt+Left", right: "Ctrl+Alt+Right", max: "Ctrl+Alt+Up", center: "Ctrl+Alt+Down", third1: "Ctrl+Alt+D", third2: "Ctrl+Alt+F", third3: "Ctrl+Alt+G", prevMonitor: "Ctrl+Alt+Shift+Left", nextMonitor: "Ctrl+Alt+Shift+Right" } },
    hotCorners: { topLeft: "overview", topRight: "none", bottomLeft: "none", bottomRight: "none" },
    controlCenter: true,
    quickLook: true,
  displaySwitch: { monitor: "", here: 1, other: 3, otherLabel: "Autre PC", detach: true },
  };
}

function state(over: Partial<DockViewState> = {}): DockViewState {
  return {
    items: [], tray: [], system: { volume: null, network: { kind: "none", name: null } },
    media: media(), settings: settings(), tone: "light", now: new Date(2026, 10, 10, 9, 0), ...over,
  };
}

let island: HTMLElement;
beforeEach(() => {
  document.body.innerHTML = '<div id="dock"><div class="island media"></div></div>';
  island = document.querySelector(".island.media")!;
  invoke.mockClear();
});

const btn = (action: string) => island.querySelector<HTMLButtonElement>(`[data-media-action="${action}"]`)!;

describe("renderMedia", () => {
  it("renders title, artist and the artwork", () => {
    renderMedia(island, media());
    expect(island.querySelector(".media-title")!.textContent).toBe("Song");
    expect(island.querySelector(".media-title")!.getAttribute("title")).toBe("Song");
    expect(island.querySelector(".media-artist")!.textContent).toBe("Band");
    const img = island.querySelector<HTMLImageElement>(".media-art img")!;
    expect(img.getAttribute("src")).toBe(ART);
    expect(img.hidden).toBe(false);
  });

  it("a broken artwork falls back to the placeholder, reset on the next src", () => {
    renderMedia(island, media());
    const art = island.querySelector(".media-art")!;
    const img = art.querySelector("img")!;
    img.dispatchEvent(new Event("error"));
    expect(img.hidden).toBe(true);
    expect(art.classList.contains("has-art")).toBe(false);
    // A play/pause update with the same (broken) url keeps the placeholder.
    renderMedia(island, media({ playing: false }));
    expect(img.hidden).toBe(true);
    expect(art.classList.contains("has-art")).toBe(false);
    // A new artwork is tried again.
    renderMedia(island, media({ artUrl: "data:image/png;base64,BBBB" }));
    expect(img.hidden).toBe(false);
    expect(art.classList.contains("has-art")).toBe(true);
  });

  it("shows the placeholder without artwork", () => {
    renderMedia(island, media({ artUrl: null }));
    const img = island.querySelector<HTMLImageElement>(".media-art img");
    expect(img === null || img.hidden).toBe(true);
    expect(island.querySelector(".media-art .media-placeholder")).not.toBeNull();
  });

  it("toggles the play/pause glyph and label", () => {
    renderMedia(island, media({ playing: true }));
    expect(btn("play_pause").getAttribute("aria-label")).toBe("Pause");
    expect(btn("play_pause").dataset.glyph).toBe("pause");
    renderMedia(island, media({ playing: false }));
    expect(btn("play_pause").getAttribute("aria-label")).toBe("Lecture");
    expect(btn("play_pause").dataset.glyph).toBe("play");
  });

  it("disables unavailable controls", () => {
    renderMedia(island, media({ canNext: false }));
    expect(btn("next").disabled).toBe(true);
    expect(btn("previous").disabled).toBe(false);
    expect(btn("previous").getAttribute("aria-label")).toBe("Précédent");
    expect(btn("next").getAttribute("aria-label")).toBe("Suivant");
  });

  it("keeps the same img node when the artwork is unchanged", () => {
    renderMedia(island, media());
    const img = island.querySelector("img");
    renderMedia(island, media({ title: "Other" }));
    expect(island.querySelector("img")).toBe(img);
    expect(island.querySelectorAll("img").length).toBe(1);
  });

  it("sends media_control on a button click", () => {
    renderMedia(island, media());
    btn("next").click();
    expect(invoke).toHaveBeenLastCalledWith("media_control", { action: "next" });
    btn("play_pause").click();
    expect(invoke).toHaveBeenLastCalledWith("media_control", { action: "play_pause" });
  });
});

describe("renderDock media island", () => {
  let root: HTMLElement;
  beforeEach(() => {
    document.body.innerHTML = '<div id="dock"></div>';
    root = document.getElementById("dock")!;
  });

  it("puts the media island first", () => {
    renderDock(root, state());
    const islands = [...root.querySelectorAll(".island")].map((e) => e.classList[1]);
    expect(islands).toEqual(["media", "apps", "tray"]);
    expect(root.firstElementChild!.classList.contains("media")).toBe(true);
    expect(root.querySelector(".island.media")!.getAttribute("aria-label")).toBe("Lecture en cours");
  });

  it("has no media island when disabled or without a session", () => {
    renderDock(root, state());
    renderDock(root, state({ settings: settings(false) }));
    expect(root.querySelector(".island.media")).toBeNull();
    renderDock(root, state({ media: null }));
    expect(root.querySelector(".island.media")).toBeNull();
  });
});

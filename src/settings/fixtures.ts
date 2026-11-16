// Test fixture shared by the settings window tests.
import type { Settings } from "../shared/types";

export const baseSettings: Settings = {
  schemaVersion: 1,
  dock: {
    iconSize: 48, magnification: true, magnificationScale: 1.6, position: "bottom",
    revealDelayMs: 150, hideDelayMs: 400, monitors: "all", glass: "auto",
    pinned: [{ app: { kind: "exe", value: "c:\\x.exe" }, name: "X", isGame: false }],
    showClock: true,
    showMedia: true,
    alwaysVisible: false,
  },
  animations: { speed: 1, style: "spring", cardDurationMs: 4000, reduceMotion: false },
  general: { launchAtStartup: true, hideWindowsTaskbar: true, language: "fr" },
  search: {
    hotkey: "Alt+Space", projectRoots: [], wslDistro: "", clipboardHistory: true,
    webUrl: "https://www.google.com/search?q=%s", textColor: "",
  },
  gameMode: { enabled: true, include: [], exclude: [], powerPlan: true, monitorPill: true, pingTarget: "1.1.1.1" },
  valorant: { riotId: "", region: "eu", apiKey: "", postMatch: true, pinCard: false },
  shortcuts: { switcher: "Alt+²", overview: "Ctrl+Alt+Space", snapping: true, micMute: "Ctrl+Alt+M", displaySwitch: "Ctrl+Alt+O" },
  hotCorners: { topLeft: "overview", topRight: "none", bottomLeft: "none", bottomRight: "none" },
  controlCenter: true,
  quickLook: true,
  displaySwitch: { monitor: "", here: 1, other: 3, otherLabel: "Autre PC", detach: true },
};

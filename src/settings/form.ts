// Pure form <-> Settings mapping and the debounced saver of the settings window.
import type {
  AnimStyle,
  DockPosition,
  GlassMode,
  HotCornerAction,
  MonitorSelection,
  Settings,
  SnapShortcuts,
} from "../shared/types";

/** Flat view of every control. `cardDurationSec` is seconds (the UI unit), the model stores ms. */
export interface FormState {
  iconSize: number;
  magnification: boolean;
  magnificationScale: number;
  position: DockPosition;
  revealDelayMs: number;
  hideDelayMs: number;
  monitors: MonitorSelection;
  glass: GlassMode;
  showClock: boolean;
  showMedia: boolean;
  alwaysVisible: boolean;
  speed: number;
  style: AnimStyle;
  cardDurationSec: number;
  reduceMotion: boolean;
  launchAtStartup: boolean;
  hideWindowsTaskbar: boolean;
  language: string;
  searchHotkey: string;
  /** One folder per line (the model stores a list). */
  projectRoots: string;
  wslDistro: string;
  clipboardHistory: boolean;
  webUrl: string;
  textColor: string;
  // Game Mode
  gameModeEnabled: boolean;
  gameModeInclude: string[];
  gameModeExclude: string[];
  gameModePowerPlan: boolean;
  gameModeMonitorPill: boolean;
  gameModePingTarget: string;
  // Valorant
  valorantRiotId: string;
  valorantRegion: string;
  valorantApiKey: string;
  valorantPostMatch: boolean;
  valorantPinCard: boolean;
  // Raccourcis (M6)
  switcherHotkey: string;
  overviewHotkey: string;
  micMuteHotkey: string;
  snapping: boolean;
  cornerTopLeft: HotCornerAction;
  cornerTopRight: HotCornerAction;
  cornerBottomLeft: HotCornerAction;
  cornerBottomRight: HotCornerAction;
  controlCenter: boolean;
  quickLook: boolean;
  displayMonitor: string;
  displayHere: number;
  displayOther: number;
  displayOtherLabel: string;
  displayDetach: boolean;
  displaySwitchHotkey: string;
  snapCombos: SnapShortcuts;
}

export const CORNER_ACTIONS: HotCornerAction[] = ["none", "overview", "desktop", "lock", "search", "switcher"];
const corner = (v: string): HotCornerAction =>
  (CORNER_ACTIONS as string[]).includes(v) ? (v as HotCornerAction) : "none";

const clamp = (v: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, v));

export function settings_to_form(s: Settings): FormState {
  return {
    iconSize: s.dock.iconSize,
    magnification: s.dock.magnification,
    magnificationScale: s.dock.magnificationScale,
    position: s.dock.position,
    revealDelayMs: s.dock.revealDelayMs,
    hideDelayMs: s.dock.hideDelayMs,
    monitors: s.dock.monitors,
    glass: s.dock.glass,
    showClock: s.dock.showClock,
    showMedia: s.dock.showMedia,
    alwaysVisible: s.dock.alwaysVisible,
    speed: s.animations.speed,
    style: s.animations.style,
    cardDurationSec: s.animations.cardDurationMs / 1000,
    reduceMotion: s.animations.reduceMotion,
    launchAtStartup: s.general.launchAtStartup,
    hideWindowsTaskbar: s.general.hideWindowsTaskbar,
    language: s.general.language,
    searchHotkey: s.search.hotkey,
    projectRoots: s.search.projectRoots.join("\n"),
    wslDistro: s.search.wslDistro,
    clipboardHistory: s.search.clipboardHistory,
    webUrl: s.search.webUrl,
    textColor: s.search.textColor,
    gameModeEnabled: s.gameMode.enabled,
    gameModeInclude: [...s.gameMode.include],
    gameModeExclude: [...s.gameMode.exclude],
    gameModePowerPlan: s.gameMode.powerPlan,
    gameModeMonitorPill: s.gameMode.monitorPill,
    gameModePingTarget: s.gameMode.pingTarget,
    valorantRiotId: s.valorant.riotId,
    valorantRegion: s.valorant.region,
    valorantApiKey: s.valorant.apiKey,
    valorantPostMatch: s.valorant.postMatch,
    valorantPinCard: s.valorant.pinCard,
    switcherHotkey: s.shortcuts.switcher,
    overviewHotkey: s.shortcuts.overview,
    micMuteHotkey: s.shortcuts.micMute,
    snapping: s.shortcuts.snapping,
    cornerTopLeft: s.hotCorners.topLeft,
    cornerTopRight: s.hotCorners.topRight,
    cornerBottomLeft: s.hotCorners.bottomLeft,
    cornerBottomRight: s.hotCorners.bottomRight,
    controlCenter: s.controlCenter,
    quickLook: s.quickLook,
    displayMonitor: s.displaySwitch.monitor,
    displayHere: s.displaySwitch.here,
    displayOther: s.displaySwitch.other,
    displayOtherLabel: s.displaySwitch.otherLabel,
    displayDetach: s.displaySwitch.detach,
    displaySwitchHotkey: s.shortcuts.displaySwitch,
    snapCombos: { ...s.shortcuts.snap },
  };
}

const VALID_REGIONS = new Set(["eu", "na", "ap", "kr", "latam", "br"]);

/** Overlays the form on `base`; fields the form does not own (pinned, schemaVersion) are kept. */
export function form_to_settings(form: FormState, base: Settings): Settings {
  return {
    ...base,
    dock: {
      ...base.dock,
      iconSize: clamp(Math.round(form.iconSize), 32, 96),
      magnification: form.magnification,
      magnificationScale: clamp(form.magnificationScale, 1, 2.5),
      position: form.position,
      revealDelayMs: clamp(Math.round(form.revealDelayMs), 0, 1000),
      hideDelayMs: clamp(Math.round(form.hideDelayMs), 0, 2000),
      monitors: form.monitors,
      glass: form.glass,
      showClock: form.showClock,
      showMedia: form.showMedia,
      alwaysVisible: form.alwaysVisible,
    },
    animations: {
      ...base.animations,
      speed: clamp(form.speed, 0.5, 2),
      style: form.style,
      cardDurationMs: clamp(Math.round(form.cardDurationSec * 1000), 1000, 10000),
      reduceMotion: form.reduceMotion,
    },
    general: {
      ...base.general,
      launchAtStartup: form.launchAtStartup,
      hideWindowsTaskbar: form.hideWindowsTaskbar,
      language: form.language,
    },
    search: {
      ...base.search,
      hotkey: form.searchHotkey,
      projectRoots: form.projectRoots.split("\n").map((l) => l.trim()).filter((l) => l !== ""),
      wslDistro: form.wslDistro.trim(),
      clipboardHistory: form.clipboardHistory,
      webUrl: form.webUrl.trim(),
      textColor: form.textColor,
    },
    gameMode: {
      ...base.gameMode,
      enabled: form.gameModeEnabled,
      include: form.gameModeInclude.map((p) => p.trim().toLowerCase()).filter((p) => p !== ""),
      exclude: form.gameModeExclude.map((p) => p.trim().toLowerCase()).filter((p) => p !== ""),
      powerPlan: form.gameModePowerPlan,
      monitorPill: form.gameModeMonitorPill,
      pingTarget: form.gameModePingTarget.trim() || "1.1.1.1",
    },
    valorant: {
      ...base.valorant,
      riotId: form.valorantRiotId.trim(),
      region: VALID_REGIONS.has(form.valorantRegion) ? form.valorantRegion : "eu",
      apiKey: form.valorantApiKey.trim(),
      postMatch: form.valorantPostMatch,
      pinCard: form.valorantPinCard,
    },
    shortcuts: {
      ...base.shortcuts,
      switcher: form.switcherHotkey.trim(),
      overview: form.overviewHotkey.trim(),
      micMute: form.micMuteHotkey.trim(),
      displaySwitch: form.displaySwitchHotkey.trim(),
      snap: Object.fromEntries(Object.entries(form.snapCombos).map(([k, v]) => [k, v.trim()])) as SnapShortcuts,
      snapping: form.snapping,
    },
    hotCorners: {
      topLeft: corner(form.cornerTopLeft),
      topRight: corner(form.cornerTopRight),
      bottomLeft: corner(form.cornerBottomLeft),
      bottomRight: corner(form.cornerBottomRight),
    },
    controlCenter: form.controlCenter,
    quickLook: form.quickLook,
    displaySwitch: {
      monitor: form.displayMonitor.trim(),
      here: inputCode(form.displayHere, base.displaySwitch.here),
      other: inputCode(form.displayOther, base.displaySwitch.other),
      otherLabel: form.displayOtherLabel.trim() || "Autre PC",
      detach: form.displayDetach,
    },
  };
}

/** A VCP 0x60 input code: an integer 0..255, else `fallback`. */
export function inputCode(v: number, fallback: number): number {
  return Number.isInteger(v) && v >= 0 && v <= 255 ? v : fallback;
}

/** Validates "Pseudo#TAG" format (name and tag both non-empty after trimming). */
export function isValidRiotId(s: string): boolean {
  const trimmed = s.trim();
  const idx = trimmed.lastIndexOf("#");
  if (idx <= 0) return false;
  const name = trimmed.slice(0, idx).trim();
  const tag = trimmed.slice(idx + 1).trim();
  return name.length > 0 && tag.length > 0;
}

export function addInclude(form: FormState, exe: string): FormState {
  const p = exe.trim().toLowerCase();
  if (!p) return form;
  const gameModeInclude = form.gameModeInclude.includes(p)
    ? form.gameModeInclude
    : [...form.gameModeInclude, p];
  const gameModeExclude = form.gameModeExclude.filter((x) => x !== p);
  return { ...form, gameModeInclude, gameModeExclude };
}

export function removeInclude(form: FormState, exe: string): FormState {
  const p = exe.trim().toLowerCase();
  return { ...form, gameModeInclude: form.gameModeInclude.filter((x) => x !== p) };
}

export function addExclude(form: FormState, exe: string): FormState {
  const p = exe.trim().toLowerCase();
  if (!p) return form;
  const gameModeExclude = form.gameModeExclude.includes(p)
    ? form.gameModeExclude
    : [...form.gameModeExclude, p];
  const gameModeInclude = form.gameModeInclude.filter((x) => x !== p);
  return { ...form, gameModeInclude, gameModeExclude };
}

export function removeExclude(form: FormState, exe: string): FormState {
  const p = exe.trim().toLowerCase();
  return { ...form, gameModeExclude: form.gameModeExclude.filter((x) => x !== p) };
}

/**
 * Trailing debounce. The payload is built by `build()` when the timer fires (or on `flush`),
 * so it always uses the latest base (e.g. a pin changed from the dock menu meanwhile).
 */
export function createSaver(build: () => Settings, save: (s: Settings) => unknown, ms = 150) {
  let timer: ReturnType<typeof setTimeout> | null = null;
  const run = (): void => {
    timer = null;
    void save(build());
  };
  return {
    touch(): void {
      if (timer !== null) clearTimeout(timer);
      timer = setTimeout(run, ms);
    },
    /** Sends the pending change now (no-op when nothing is pending). */
    flush(): void {
      if (timer === null) return;
      clearTimeout(timer);
      run();
    },
    get pending(): boolean {
      return timer !== null;
    },
  };
}

// Mirrors of the Rust types (camelCase over the wire). Later tasks append their own.

export type AppId =
  | { kind: "exe"; value: string } // lowercased full path
  | { kind: "aumid"; value: string };

export type DockPosition = "bottom" | "left" | "right";
export type MonitorSelection = "all" | "primary" | { list: string[] };
export type GlassMode = "auto" | "light" | "dark";
/** `dock://tone` payload: glass tone actually shown (mode override already applied). */
export type GlassTone = "light" | "dark";
export type AnimStyle = "spring" | "sober";

export interface PinnedApp {
  app: AppId;
  name: string;
  isGame: boolean;
}

export interface DockSettings {
  iconSize: number;
  magnification: boolean;
  magnificationScale: number;
  position: DockPosition;
  revealDelayMs: number;
  hideDelayMs: number;
  monitors: MonitorSelection;
  glass: GlassMode;
  pinned: PinnedApp[];
  showClock: boolean;
  /** Media island (current system media session). */
  showMedia: boolean;
  /** Never auto-hide (except over fullscreen apps and games); reserves its screen strip. */
  alwaysVisible: boolean;
}

export interface AnimationSettings {
  speed: number;
  style: AnimStyle;
  cardDurationMs: number;
  reduceMotion: boolean;
}

export interface GeneralSettings {
  launchAtStartup: boolean;
  hideWindowsTaskbar: boolean;
  language: string;
  /** Look for a new version on GitHub (startup, then every few hours). */
  checkUpdates: boolean;
}

export type UpdatePhase = "idle" | "checking" | "upToDate" | "available" | "downloading" | "installing" | "failed";

/** `update://state` and the `update_*` commands. */
export interface UpdateStatus {
  phase: UpdatePhase;
  current: string;
  version: string | null;
  progress: number | null;
  error: string | null;
  /** « Plus tard » on the dock card (holds for this version). */
  dismissed: boolean;
}

export interface SearchSettings {
  /** Global hotkey, e.g. "Alt+Space". */
  hotkey: string;
  /** Empty = auto-detect. */
  projectRoots: string[];
  /** Empty = default WSL distribution. */
  wslDistro: string;
  clipboardHistory: boolean;
  /** `%s` is replaced by the query. */
  webUrl: string;
  /** "" = auto (follows the glass tone), else "#rrggbb". */
  textColor: string;
}

export interface Settings {
  schemaVersion: number;
  dock: DockSettings;
  animations: AnimationSettings;
  general: GeneralSettings;
  search: SearchSettings;
  gameMode: GameModeSettings;
  valorant: ValorantSettings;
  shortcuts: ShortcutSettings;
  hotCorners: HotCorners;
  /** Control Center panel (click on the dock's system island). */
  controlCenter: boolean;
  /** Quick Look (Shift+Espace on a search result). */
  quickLook: boolean;
  /** Monitor shared with another PC (input switch over DDC/CI). */
  displaySwitch: DisplaySwitchSettings;
}

/** `monitor`: EDID name ("24E4"), "" = off. `here` / `other`: VCP 0x60 input codes. */
export interface DisplaySwitchSettings {
  monitor: string;
  here: number;
  other: number;
  otherLabel: string;
  /** Take the monitor off this PC's desktop while it shows the other PC. */
  detach: boolean;
}

/** `list_ddc_monitors`: DDC/CI monitors and their current input (null: no answer). */
export interface DdcMonitor {
  name: string;
  input: number | null;
}

/** M6 global shortcuts, same syntax as `search.hotkey` plus arrows (Left/Gauche…) and OEM
 * characters (²…). "" disables one. Snapping combos are fixed (Ctrl+Alt+arrows, D/F/G,
 * Ctrl+Alt+Shift+Left/Right): only the toggle is a setting. */
export interface ShortcutSettings {
  /** App switcher (default "Alt+²"); + Shift = backwards. */
  switcher: string;
  /** Default "Ctrl+Alt+Space". */
  overview: string;
  snapping: boolean;
  /** Default "Ctrl+Alt+M". */
  micMute: string;
  /** Shared monitor: this PC ⇄ the other one. Default "Ctrl+Alt+O". */
  displaySwitch: string;
  /** Window snapping combos, used while `snapping` is on ("" disables one). */
  snap: SnapShortcuts;
}

export type SnapKey = "left" | "right" | "max" | "center" | "third1" | "third2" | "third3" | "prevMonitor" | "nextMonitor";
export type SnapShortcuts = Record<SnapKey, string>;

export const DEFAULT_SNAP: SnapShortcuts = {
  left: "Ctrl+Alt+Left", right: "Ctrl+Alt+Right", max: "Ctrl+Alt+Up", center: "Ctrl+Alt+Down",
  third1: "Ctrl+Alt+D", third2: "Ctrl+Alt+F", third3: "Ctrl+Alt+G",
  prevMonitor: "Ctrl+Alt+Shift+Left", nextMonitor: "Ctrl+Alt+Shift+Right",
};

export type HotCornerAction = "none" | "overview" | "desktop" | "lock" | "search" | "switcher";

/** Rust validates the values (unknown -> "none"). Default: topLeft "overview", others "none". */
export interface HotCorners {
  topLeft: HotCornerAction;
  topRight: HotCornerAction;
  bottomLeft: HotCornerAction;
  bottomRight: HotCornerAction;
}

/** Mirror of Rust `hotkey::HotkeyId`. */
export type HotkeyId =
  | "search"
  | "switcher"
  | "switcherBack"
  | "overview"
  | "snapLeft"
  | "snapRight"
  | "snapMax"
  | "snapCenter"
  | "snapThird1"
  | "snapThird2"
  | "snapThird3"
  | "movePrevMonitor"
  | "moveNextMonitor"
  | "micMute"
  | "displaySwitch";

/** `get_hotkey_statuses` / `hotkeys://status`: only wanted (enabled) ids are present. */
export type HotkeyStatuses = Partial<Record<HotkeyId, HotkeyStatus>>;

export interface GameModeSettings {
  enabled: boolean;
  /** Lowercased exe paths always treated as games. */
  include: string[];
  /** Lowercased exe paths never treated as games. */
  exclude: string[];
  powerPlan: boolean;
  monitorPill: boolean;
  pingTarget: string;
}

export interface ValorantSettings {
  riotId: string;
  /** eu | na | ap | kr | latam | br */
  region: string;
  apiKey: string;
  postMatch: boolean;
  /** Keep the last match's card in the dock at all times. */
  pinCard: boolean;
}

export interface GameState {
  active: boolean;
  name: string | null;
  monitorId: string | null;
  focused: boolean;
}

export interface SysStats {
  cpu: number;
  ram: { used: number; total: number };
  gpu: number | null;
  gpuTemp: number | null;
  pingMs: number | null;
  wslRamMb: number | null;
}

export interface DetectedGame {
  name: string;
  exe: string;
  source: "steam" | "epic" | "riot" | "ea" | "manual";
}

export interface ValoMatchCard {
  /** Plain-text summary: the card's layout when no picture could be loaded. */
  line: string;
  won: boolean | null;
  map?: string;
  mapId?: string;
  agent?: string;
  agentId?: string;
  competitive?: boolean;
  /** Rounds, the player's team first. */
  score?: [number, number];
  kda?: [number, number, number];
  hsPct?: number | null;
  acs?: number | null;
  rrChange?: number | null;
  /** French rank after the match ("Diamant 2"). */
  tier?: string | null;
  tierId?: number | null;
  /** RR inside the current tier; next rank and RR missing to reach it. */
  rr?: number | null;
  nextTier?: string | null;
  rrToNext?: number | null;
  /** `valo` scheme picture URLs; null when not downloaded (offline). */
  mapSplash?: string | null;
  agentPortrait?: string | null;
  tierIcon?: string | null;
}

export interface TrashStatus {
  count: number;
  bytes: number;
}

export interface StackFile {
  name: string;
  path: string;
  size: number;
  modified: number;
  isDir: boolean;
}

export interface WindowRef {
  hwnd: number;
  title: string;
}

export interface DockItem {
  id: string;
  app: AppId;
  name: string;
  iconUrl: string;
  pinned: boolean;
  isGame: boolean;
  windows: WindowRef[];
  badge?: string | null;
}

/** Physical pixels. */
export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface MonitorInfo {
  id: string; // device name, e.g. \\.\DISPLAY1
  rect: Rect;
  work: Rect;
  scale: number;
  primary: boolean;
}

export interface RevealPayload {
  shown: boolean;
}

/** `thumbs://show` payload (emitted to the `thumbs` window only). Rects are physical px
 * relative to the window; divide by `scale` for CSS px. */
export interface ThumbsShow {
  itemId: string;
  hwnds: number[];
  titles: string[];
  rects: Rect[];
  width: number;
  height: number;
  scale: number;
  titleH: number;
  /** Tone of the dock the popup belongs to. */
  tone: GlassTone;
}

/** One hosted notification-area icon (`tray://icons`). `iconUrl` changes when the icon does. */
export interface TrayIconView {
  key: string;
  tip: string;
  iconUrl: string;
  /** Shown inline ("show in taskbar" in Windows 11); otherwise behind the chevron. */
  promoted: boolean;
}

export type TrayButton = "left" | "right" | "double";

export type VolumeIcon = "mute" | "low" | "mid" | "high";

/** `system://volume`: default render endpoint, `level` in 0..1. */
export interface VolumeState {
  level: number;
  muted: boolean;
  icon: VolumeIcon;
}

/** `media://state` / `get_media`: the current system media session (null when none). */
export interface MediaState {
  title: string;
  artist: string;
  /** `data:image/png;base64,…` (96×96), null when the player gives no artwork. */
  artUrl: string | null;
  playing: boolean;
  canPrev: boolean;
  canNext: boolean;
  canPlayPause: boolean;
}

export type MediaAction = "play_pause" | "next" | "previous";

export type NetKind = "ethernet" | "wifi" | "none";

/** `system://network`: `name` is the Wi-Fi SSID, null when unknown / not Wi-Fi. */
export interface NetworkState {
  kind: NetKind;
  name: string | null;
}

/** `get_system_state`: the page pulls it on boot, then follows the events. */
export interface SystemState {
  volume: VolumeState | null;
  network: NetworkState;
  /** Default communications mic muted (then follows `audio://devices`). */
  micMuted?: boolean;
}

/** `get_hotkey_status` / `hotkey://status`: `ok` is false when another app owns the combo. */
export interface HotkeyStatus {
  combo: string;
  ok: boolean;
}

/** `search_resize` args: content height of the search page, CSS px. A type alias (not an
 * interface) so it stays assignable to Tauri's `InvokeArgs` record. */
export type SearchResize = { height: number };

export type EntryKind = "app" | "project" | "file" | "command" | "clipboard" | "emoji" | "valorant";

export interface SearchEntry {
  id: string;
  kind: EntryKind;
  title: string;
  subtitle: string;
  icon: string | null;
  keywords: string[];
  /** `valo` rows only: what the Valorant tiles draw. */
  valo?: ValoRow;
}

export type ValoRow =
  | {
      type: "rank";
      tier: string;
      rr: number;
      lastChange: number;
      tierIcon: string | null;
      /** Next rank and the RR still missing (Iron 1 .. Ascendant 3 only). */
      nextTier?: string | null;
      rrToNext?: number | null;
    }
  | {
      type: "match";
      won: boolean | null;
      score: [number, number];
      map: string;
      agent: string;
      kda: [number, number, number];
      hsPct: number | null;
      acs: number | null;
      competitive: boolean;
      mapImage: string | null;
      agentIcon: string | null;
    };

/** `indices`: char positions of the matched characters in `entry.title` (bold highlight). */
export interface SearchHit {
  entry: SearchEntry;
  score: number;
  indices: number[];
}

/** Display order: recent (empty query only), valorant (`valo` only), app, project, command, clipboard, emoji, file. */
export interface SearchGroup {
  key: "recent" | EntryKind;
  items: SearchHit[];
}

// ---- M6 contracts (overlays implemented by the feature tracks) ----

export interface SwitcherWindow {
  hwnd: number;
  title: string;
}

export interface SwitcherApp {
  /** Dock item id. */
  id: string;
  name: string;
  iconUrl: string;
  windows: SwitcherWindow[];
}

/** `switcher://show`: apps in MRU order, `index` = highlighted app. */
export interface SwitcherShow {
  apps: SwitcherApp[];
  index: number;
  /** Slot size per app, CSS px (shrinks when many apps must fit the monitor). */
  cell?: number;
}

/** `switcher://windows` (↓): live thumbnails of the selected app's windows. */
export interface SwitcherWindows {
  appIndex: number;
  hwnds: number[];
  titles: string[];
  /** Physical px relative to the switcher window. */
  rects: Rect[];
  /** Selected window. */
  index: number;
  scale: number;
  titleH: number;
}

export interface OverviewTile {
  hwnd: number;
  title: string;
  appName: string;
  iconUrl: string;
  /** Physical px relative to the overview window. */
  rect: Rect;
}

/** `overview://show`. */
export interface OverviewShow {
  monitorId: string;
  tiles: OverviewTile[];
  /** Monitor scale (physical px per CSS px). */
  scale?: number;
  /** No monitor has any window. */
  emptyHint?: boolean;
}

export interface AudioOutput {
  id: string;
  name: string;
  default: boolean;
}

/** `audio://devices`. */
export interface AudioDevices {
  outputs: AudioOutput[];
  micMuted: boolean;
}

/** `cc://state`: null members = not available on this machine. */
export interface ControlCenterState {
  wifi: { on: boolean; name: string | null } | null;
  bluetooth: { on: boolean } | null;
  darkMode: boolean;
  /** 0..100, null when the monitor has no software brightness. */
  brightness: number | null;
}

export type QuickLookKind = "image" | "pdf" | "text" | "video" | "none";

/** `quicklook_open` result. */
export interface QuickLookPreview {
  kind: QuickLookKind;
  url?: string;
  text?: string;
  /** Card shown for kind "none": display name, bytes, ms since epoch. */
  name?: string;
  size?: number;
  modified?: number;
}

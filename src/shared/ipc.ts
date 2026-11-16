// The only module allowed to import @tauri-apps/api. Names follow the plan's IPC contract.
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import type {
  AudioDevices,
  ControlCenterState,
  DetectedGame,
  DockItem,
  GameState,
  GlassTone,
  HotkeyStatus,
  HotkeyStatuses,
  OverviewShow,
  QuickLookPreview,
  SwitcherShow,
  SwitcherWindows,
  MediaAction,
  MediaState,
  MonitorInfo,
  NetworkState,
  Rect,
  RevealPayload,
  SearchGroup,
  SearchResize,
  Settings,
  SysStats,
  SystemState,
  ThumbsShow,
  TrayButton,
  TrayIconView,
  TrashStatus,
  StackFile,
  ValoMatchCard,
  VolumeState, DdcMonitor } from "./types";

/** Front -> Rust commands: args and result. Later tasks add their own entries. */
export interface Commands {
  get_settings: { args: Record<string, never>; result: Settings };
  save_settings: { args: { settings: Settings }; result: void };
  report_dock_layout: {
    /** `islands`: the glass shapes; `hit`: extra areas the window region must cover;
     *  `pill`: only the Game Mode system pill is shown (Rust keeps the hidden window on-screen). */
    args: { monitorId: string; width: number; height: number; islands: Rect[]; hit: Rect[]; pill: boolean };
    result: void;
  };
  get_dock_items: { args: Record<string, never>; result: DockItem[] };
  dock_click: { args: { itemId: string; monitorId: string }; result: void };
  dock_menu: { args: { itemId: string | null }; result: void };
  /** Drag-reorder; `beforeId` null = end of the section. Pins the moved item. */
  dock_reorder: { args: { itemId: string; beforeId: string | null }; result: void };
  thumbs_pick: { args: { hwnd: number }; result: void };
  /** Hover preview: open the thumbnails popup of running item `itemId` (Rust ignores it in
   *  Game Mode or while that dock is not shown). */
  dock_hover: { args: { itemId: string; monitorId: string }; result: void };
  /** Hover preview: the pointer left both the icon and the popup. */
  thumbs_close: { args: Record<string, never>; result: void };
  /** Thumbs page: the pointer entered / left the popup (relayed as `thumbs://pointer`). */
  thumbs_pointer: { args: { inside: boolean }; result: void };
  get_tray_icons: { args: Record<string, never>; result: TrayIconView[] };
  /** `x`, `y`: icon centre in physical px relative to the dock window. */
  tray_click: { args: { key: string; button: TrayButton; x: number; y: number }; result: void };
  get_system_state: { args: Record<string, never>; result: SystemState };
  /** `level` in 0..1. */
  set_volume: { args: { level: number }; result: void };
  toggle_mute: { args: Record<string, never>; result: void };
  get_media: { args: Record<string, never>; result: MediaState | null };
  media_control: { args: { action: MediaAction }; result: void };
  open_notifications: { args: Record<string, never>; result: void };
  open_network: { args: Record<string, never>; result: void };
  open_settings: { args: { tab?: string }; result: void };
  open_external: { args: { url: string }; result: void };
  list_monitors: { args: Record<string, never>; result: MonitorInfo[] };
  /** Quits Lucent (the taskbar is restored on exit). */
  quit_app: { args: Record<string, never>; result: void };
  get_hotkey_status: { args: Record<string, never>; result: HotkeyStatus };
  /** The thumbnails page is loaded and listening (first request waits for it). */
  thumbs_ready: { args: Record<string, never>; result: void };
  /** Échap in the search page: hide the search window. */
  search_hide: { args: Record<string, never>; result: void };
  /** Content height of the search page in CSS px (window: 680 wide, height clamped to 64..520). */
  search_resize: { args: SearchResize; result: void };
  /** Ranked groups for `q`; empty `q` gives the "recent" group. */
  search_query: { args: { q: string }; result: SearchGroup[] };
  /** Bumps history and opens the entry; the page hides the window afterwards. */
  search_open: { args: { id: string; q: string; shift: boolean; ctrl: boolean }; result: void };
  /** Answer to the "is this a game?" prompt. */
  game_answer: { args: { exe: string; isGame: boolean }; result: void };
  list_detected_games: { args: Record<string, never>; result: DetectedGame[] };
  get_game_state: { args: Record<string, never>; result: GameState };
  /** French result line. */
  wsl_free: { args: Record<string, never>; result: string };
  wsl_shutdown: { args: Record<string, never>; result: string };
  valo_test: { args: Record<string, never>; result: string };
  get_trash_status: { args: Record<string, never>; result: TrashStatus };
  empty_trash: { args: Record<string, never>; result: void };
  list_stack_files: { args: { folder: string; limit?: number }; result: StackFile[] };
  /** Every enabled hotkey's status (M6). */
  get_hotkey_statuses: { args: Record<string, never>; result: HotkeyStatuses };
  // ---- M6 contracts (Rust side added by the feature tracks) ----
  /** Activate the app (its most recent window, or `hwnd`) and close the switcher. */
  switcher_pick: { args: { appId: string; hwnd?: number }; result: void };
  overview_pick: { args: { hwnd: number }; result: void };
  overview_close: { args: Record<string, never>; result: void };
  cc_set_wifi: { args: { on: boolean }; result: void };
  cc_set_bluetooth: { args: { on: boolean }; result: void };
  cc_set_dark: { args: { on: boolean }; result: void };
  /** `value` in 0..100. */
  cc_set_brightness: { args: { value: number }; result: void };
  audio_set_output: { args: { id: string }; result: void };
  mic_toggle: { args: Record<string, never>; result: void };
  /** Shared monitor input: `toOther` absent = toggle. Answer: `display://switched`. */
  display_switch: { args: { toOther?: boolean | null }; result: void };
  list_ddc_monitors: { args: Record<string, never>; result: DdcMonitor[] };
  power_action: { args: { action: "lock" | "sleep" | "signout" | "restart" | "shutdown" }; result: void };
  /** Re-reads radios / theme / brightness (monitor under the cursor) and emits `cc://state`. */
  cc_refresh: { args: Record<string, never>; result: void };
  /** Current `audio://devices` payload (pulled on boot). */
  get_audio_devices: { args: Record<string, never>; result: AudioDevices };
  /** Preview of search result `id`. */
  quicklook_open: { args: { id: string }; result: QuickLookPreview };
  /** The preview closed: its `qlook://` token stops working. */
  quicklook_close: { args: Record<string, never>; result: void };
  /** Files dropped on dock item `itemId`: open them with that app. */
  /** Spring-loading: bring the app's most recent window to the front. */
  dock_spring: { args: { itemId: string }; result: void };
  dock_drop_open: { args: { itemId: string; paths: string[] }; result: void };
  /** Switcher page loaded and listening, or rendered the last `switcher://show` (shown then). */
  switcher_ready: { args: Record<string, never>; result: void };
  /** Overview page loaded and listening, or rendered the last `overview://show`. */
  overview_ready: { args: Record<string, never>; result: void };
}

/** Rust -> front events: payloads. Later tasks add their own entries. */
export interface Events {
  "settings://changed": Settings;
  "dock://items": DockItem[];
  "dock://reveal": RevealPayload;
  "dock://tone": GlassTone;
  "thumbs://show": ThumbsShow;
  "thumbs://hide": null;
  /** The pointer entered (true) / left the thumbnails popup (to every dock). */
  "thumbs://pointer": boolean;
  /** The thumbnails popup closed, whatever closed it (to every dock). */
  "thumbs://closed": null;
  "tray://icons": TrayIconView[];
  "system://volume": VolumeState;
  "system://network": NetworkState;
  /** null when there is no media session. */
  "media://state": MediaState | null;
  "hotkey://status": HotkeyStatus;
  /** Search window shown (emitted to the `search` window only): focus and select the input. */
  "search://shown": null;
  /** Server-side results changed (a `valo` fetch finished; search window only): re-run the query. */
  "search://refresh": null;
  "game://state": GameState;
  "sysmon://stats": SysStats;
  "valo://match": ValoMatchCard;
  "game://ask": { exe: string; name: string };
  "settings://tab": string;
  /** Every enabled hotkey's status, after each registration change (M6). */
  "hotkeys://status": HotkeyStatuses;
  // ---- M6 contracts ----
  "switcher://show": SwitcherShow;
  "switcher://move": { index: number };
  "switcher://hide": null;
  /** Windows row of the selected app (null: row closed). */
  "switcher://windows": SwitcherWindows | null;
  /** Selection moved inside the windows row. */
  "switcher://window-index": { index: number };
  "overview://show": OverviewShow;
  "overview://hide": null;
  "audio://devices": AudioDevices;
  "cc://state": ControlCenterState;
  "display://switched": { text: string; ok: boolean };
}

export function invoke<K extends keyof Commands>(
  cmd: K,
  ...args: Record<string, never> extends Commands[K]["args"]
    ? [Commands[K]["args"]?]
    : [Commands[K]["args"]]
): Promise<Commands[K]["result"]> {
  return tauriInvoke(cmd, args[0]);
}

/** Subscribes to an event; resolves to the unlisten function. */
export function on<E extends keyof Events>(
  event: E,
  handler: (payload: Events[E]) => void,
): Promise<() => void> {
  return listen<Events[E]>(event, (e) => handler(e.payload));
}

/**
 * Subscribes to an event sent to *this* window only (`emit_to(label, …)` in Rust).
 * The global `on()` registers an `Any` listener, which Tauri delivers every event to
 * whatever its target, so per-window events (`dock://tone`, `dock://reveal`) must use this.
 */
export function onWindow<E extends keyof Events>(
  event: E,
  handler: (payload: Events[E]) => void,
): Promise<() => void> {
  const label = getCurrentWebviewWindow().label;
  return listen<Events[E]>(event, (e) => handler(e.payload), { target: { kind: "WebviewWindow", label } });
}

/** OS file drag over this window (Tauri drag-drop events). `position`: physical px relative to the page. */
export type FileDrag =
  | { type: "enter"; paths: string[]; position: { x: number; y: number } }
  | { type: "over"; position: { x: number; y: number } }
  | { type: "drop"; paths: string[]; position: { x: number; y: number } }
  | { type: "leave" };

export function onFileDrag(handler: (e: FileDrag) => void): Promise<() => void> {
  return getCurrentWebviewWindow().onDragDropEvent((e) => handler(e.payload as FileDrag));
}

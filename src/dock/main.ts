// Dock page: one per monitor (`?monitor=<id>`). Renders the islands, magnifies icons,
// slides on `dock://reveal`, and reports its layout so Rust sizes the window and clips
// it (and the Acrylic) to the islands.
import { invoke, on, onFileDrag, onWindow } from "../shared/ipc";
import type { AudioDevices, ControlCenterState, DockItem, GameState, GlassTone, MediaState, Rect, Settings, SysStats, SystemState, TrashStatus, TrayIconView, UpdateStatus } from "../shared/types";
import { iconScale, maxRowGrowth } from "./magnify";
import { VALO_PORTRAIT_OVERFLOW, updateCardShown, type CardViewState } from "./card";
import { showPill } from "./pill";
import { renderDock } from "./render";
import { hitItem, SPRING_IDLE, stepSpring, type SpringState } from "./drop";
import { attachReorder, moveItem } from "./reorder";
import { closeVolumePopover } from "./system";
import { closeTrayPanel } from "./tray";
import { createBounceManager } from "./bounce";
import { closeDownloadsStack } from "./stack";
import { closeTrashMenu } from "./trash";
import { createHoverPreview } from "./hover";

const monitorId = new URLSearchParams(location.search).get("monitor") ?? "";
const frame = document.getElementById("frame")!;
const dock = document.getElementById("dock")!;

const bounce = createBounceManager({ onChange: () => render() });

/** Entry into / exit from magnification (`#frame.settling` transition is 200 ms). */
const SETTLE_IN_MS = 120;
const SETTLE_OUT_MS = 220;
/** Space between the tray island / pill and their panel (`.tray-panel`, `.pill-panel` in dock.css). */
const PANEL_GAP = 10;
/**
 * Room always kept above the row (CSS px) for the popups (tray overflow, volume, Control
 * Center, Downloads stack up to 420 px, trash menu). Opening one then only changes the
 * window region, like magnification: resizing a WebView2 window shows a stale frame for
 * a moment and the whole dock jumped.
 */
const PANEL_HEADROOM = 430 + PANEL_GAP;
/** Largest window growth seen for the current geometry settings: never shrinks on close. */
let sticky = { key: "", grow: 0, cross: 0 };
/** CSS px around the app buttons kept in the window region (dots, launch hop). */
const HIT_MARGIN = 10;

let settings: Settings | null = null;
let items: DockItem[] = [];
let trayIcons: TrayIconView[] = [];
let system: SystemState = { volume: null, network: { kind: "none", name: null } };
let media: MediaState | null = null;
let autoTone: GlassTone = "light";
/** Game Mode on this monitor: "game" (the game runs here), "other" (it runs elsewhere:
 *  no magnification, no animations), null (no game). */
let gameMode: "game" | "other" | null = null;
let game: GameState | null = null;
/** The dock is revealed (`dock://reveal`). */
let revealed = false;
/** Last `sysmon://stats` (sent only while a pill is shown somewhere). */
let stats: SysStats | null = null;
/** Active post-match or ask card. */
let currentCard: CardViewState | null = null;
/** Last match card, kept for the "always shown" setting. */
let lastMatch: CardViewState | null = null;

function pinOn(): boolean {
  return !!settings?.valorant?.postMatch && !!settings?.valorant?.pinCard;
}

/** Last `update://state`. */
let update: UpdateStatus | null = null;

/** The temporary card (mic, game prompt, fresh match) wins; then an update waiting for an
 *  answer; else the pinned last match. */
function shownCard(): { card: CardViewState | null; pinned: boolean } {
  if (currentCard === null && updateCardShown(update) && update) {
    return { card: { kind: "update", status: update }, pinned: true };
  }
  if (pinOn() && (currentCard === null || currentCard.kind === "match") && lastMatch) {
    return { card: lastMatch, pinned: true };
  }
  return { card: currentCard, pinned: false };
}
let trashStatus: TrashStatus | null = null;
/** Control Center content (`cc://state`, `audio://devices`). */
let ccState: ControlCenterState | null = null;
let audioDevices: AudioDevices | null = null;

// ---- Rendering ---------------------------------------------------------------------

function vertical(): boolean {
  return settings?.dock.position === "left" || settings?.dock.position === "right";
}

function tone(): GlassTone {
  const mode = settings?.dock.glass ?? "auto";
  return mode === "auto" ? autoTone : mode;
}

function magnifyEnabled(): boolean {
  const s = settings;
  return (
    !!s && gameMode !== "other" && s.dock.magnification && !s.animations.reduceMotion && s.dock.magnificationScale > 1
  );
}

/** Game Mode system pill on this monitor (the hidden dock shows only it). */
function pillOn(): boolean {
  return showPill({ game, monitorId, monitorPill: settings?.gameMode?.monitorPill ?? false, revealed });
}

function applyBodyState(s: Settings): void {
  const b = document.body;
  b.dataset.position = s.dock.position;
  b.dataset.anim = s.animations.style;
  b.toggleAttribute("data-reduce-motion", s.animations.reduceMotion);
  const speed = Math.max(0.1, s.animations.speed);
  const root = document.documentElement.style;
  root.setProperty("--speed", String(speed));
  root.setProperty("--slide-ms", s.animations.reduceMotion ? "0ms" : `${Math.round(240 / speed)}ms`);
  root.setProperty("--slide-ease", s.animations.style === "spring" ? "cubic-bezier(.2,.8,.2,1)" : "ease");
}

function render(): void {
  if (!settings) return;
  applyBodyState(settings);
  renderDock(dock, {
    items, tray: trayIcons, system, media, settings, tone: tone(), now: new Date(),
    pill: pillOn() ? { stats } : null,
    card: shownCard().card,
    cardPinned: shownCard().pinned,
    bouncing: (id) => bounce.isBouncing(id),
    trash: trashStatus,
    showDownloads: true,
    control: { cc: ccState, audio: audioDevices },
    onEmptyTrash: () => {
      trashStatus = { count: 0, bytes: 0 };
      render();
    },
    onDismissCard: () => {
      currentCard = null;
      render();
    },
  });
  measureRest();
  reportLayout();
  markDropTarget();
}

// ---- Geometry: window box sized for full magnification -----------------------------

let restCenters: number[] = [];

function appButtons(): HTMLElement[] {
  return [...dock.querySelectorAll<HTMLElement>(".island.apps > .app")];
}

/** Main-axis centre of an icon in #frame coordinates (icon → island → frame). */
function mainPos(el: HTMLElement): number {
  const isl = el.offsetParent as HTMLElement | null;
  const v = vertical();
  const own = v ? el.offsetTop + el.offsetHeight / 2 : el.offsetLeft + el.offsetWidth / 2;
  return own + (isl ? (v ? isl.offsetTop : isl.offsetLeft) : 0);
}

/**
 * Measures the row at rest (every icon at base size, even mid-magnification: inline sizes
 * are restored before the next paint), sizes #frame so the fully magnified row always
 * fits (the window never resizes while magnifying, only its region changes), then
 * records each icon's rest centre in frame coordinates.
 */
function measureRest(): void {
  if (!settings) return;
  const btns = appButtons();
  const saved = btns.map((b) => [b.style.width, b.style.height] as const);
  for (const b of btns) b.style.width = b.style.height = "";

  const size = settings.dock.iconSize;
  const scale = magnifyEnabled() ? settings.dock.magnificationScale : 1;
  const v = vertical();
  const restMain = v ? dock.offsetHeight : dock.offsetWidth;
  const restCross = v ? dock.offsetWidth : dock.offsetHeight;
  // Only distances between icons matter here, so measuring before resizing #frame is fine.
  let grow = Math.ceil(maxRowGrowth(btns.map(mainPos), size, scale)) + 4;
  let crossGrow = Math.ceil((scale - 1) * size) + 2;
  // An open tray overflow panel / volume popover / pill panel sticks out of the row
  // (towards the screen) and may overhang the row's start: the window must contain it,
  // or it would be clipped.
  for (const pop of dock.querySelectorAll<HTMLElement>(".tray-panel, .vol-pop, .cc-panel, .pill-panel, .stack-popover, .trash-menu")) {
    const host = pop.parentElement?.closest<HTMLElement>(".island.tray, .island.pill, .island.apps") ?? dock;
    if (!host) continue;
    const hostCross = v ? host.offsetWidth : host.offsetHeight;
    const out = (v ? pop.offsetWidth : pop.offsetHeight) + PANEL_GAP - (restCross - hostCross);
    crossGrow = Math.max(crossGrow, Math.ceil(out) + 2);
    // The popup is aligned on the island's end edge and extends towards the row's start.
    const at = frameOffset(host);
    const origin = frameOffset(dock);
    const hostEnd = v ? at.y - origin.y + host.offsetHeight : at.x - origin.x + host.offsetWidth;
    const overhang = (v ? pop.offsetHeight : pop.offsetWidth) - hostEnd;
    if (overhang > 0) grow = Math.max(grow, 2 * Math.ceil(overhang) + 4); // the row is centred
  }
  // The Valorant card's agent portrait rises above the card (bottom dock only).
  if (!v && dock.querySelector(".vc-agent")) crossGrow = Math.max(crossGrow, VALO_PORTRAIT_OVERFLOW + 2);
  // Popups open and close inside a window that keeps its size (see PANEL_HEADROOM).
  const key = JSON.stringify([size, scale, settings.dock.position]);
  if (sticky.key !== key) sticky = { key, grow: 0, cross: 0 };
  crossGrow = Math.max(crossGrow, PANEL_HEADROOM, sticky.cross);
  grow = Math.max(grow, sticky.grow);
  sticky.cross = crossGrow;
  sticky.grow = grow;
  frame.style.width = `${v ? restCross + crossGrow : restMain + grow}px`;
  frame.style.height = `${v ? restMain + grow : restCross + crossGrow}px`;
  restCenters = btns.map(mainPos);

  btns.forEach((b, i) => ([b.style.width, b.style.height] = saved[i]));
}

let lastLayout = "";

/** Offset of `el` within #frame, summed up the offsetParent chain (transform-free). */
function frameOffset(el: HTMLElement): { x: number; y: number } {
  let x = 0;
  let y = 0;
  for (let n: HTMLElement | null = el; n && n !== frame; n = n.offsetParent as HTMLElement | null) {
    x += n.offsetLeft;
    y += n.offsetTop;
  }
  return { x, y };
}

/** Bounding size + island rects in physical px, at rest: offset* values ignore the slide
 *  transform (Rust slides the native glass itself, on the compositor, with the same
 *  duration and curve). Sent only when something changed. */
function reportLayout(force = false): void {
  const dpr = window.devicePixelRatio;
  const width = Math.ceil(frame.offsetWidth * dpr);
  const height = Math.ceil(frame.offsetHeight * dpr);
  const islands: Rect[] = [...dock.querySelectorAll<HTMLElement>(".island")]
    .filter((el) => el.offsetWidth > 0 && el.offsetHeight > 0)
    .map((el) => {
      const o = frameOffset(el);
      return {
        x: Math.round(o.x * dpr),
        y: Math.round(o.y * dpr),
        w: Math.round(el.offsetWidth * dpr),
        h: Math.round(el.offsetHeight * dpr),
      };
    });
  // Magnified icons rise out of the glass: the window region must also cover them (and
  // their dots / launch hop), or they would be cut at the island edge.
  const hit: Rect[] = [];
  const btns = appButtons();
  if (btns.length > 0) {
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const b of btns) {
      const o = frameOffset(b);
      x0 = Math.min(x0, o.x);
      y0 = Math.min(y0, o.y);
      x1 = Math.max(x1, o.x + b.offsetWidth);
      y1 = Math.max(y1, o.y + b.offsetHeight);
    }
    const m = HIT_MARGIN;
    hit.push({
      x: Math.round((x0 - m) * dpr),
      y: Math.round((y0 - m) * dpr),
      w: Math.round((x1 - x0 + 2 * m) * dpr),
      h: Math.round((y1 - y0 + 2 * m) * dpr),
    });
  }
  for (const p of dock.querySelectorAll<HTMLElement>(".vc-agent")) {
    const o = frameOffset(p);
    hit.push({ x: Math.round(o.x * dpr), y: Math.round(o.y * dpr), w: Math.round(p.offsetWidth * dpr), h: Math.round(p.offsetHeight * dpr) });
  }
  const pill = dock.classList.contains("pill-mode");
  const key = JSON.stringify([width, height, islands, hit, pill]);
  if (!force && key === lastLayout) return;
  lastLayout = key;
  void invoke("report_dock_layout", { monitorId, width, height, islands, hit, pill });
}

/** Keeps the window region glued to the islands while CSS transitions resize them. */
let followUntil = 0;
function followLayout(ms: number): void {
  const running = followUntil !== 0;
  followUntil = Math.max(followUntil, performance.now() + ms);
  if (running) return;
  const step = () => {
    reportLayout();
    if (performance.now() < followUntil) requestAnimationFrame(step);
    else followUntil = 0;
  };
  requestAnimationFrame(step);
}

// ---- Magnification ---------------------------------------------------------------

let pointer: number | null = null;
let frameQueued = false;
let settleTimer = 0;

function settle(ms: number): void {
  frame.classList.add("settling");
  clearTimeout(settleTimer);
  settleTimer = window.setTimeout(() => frame.classList.remove("settling"), ms);
  followLayout(ms + 40);
}

function applyMagnification(): void {
  frameQueued = false;
  if (!settings || pointer === null) return;
  const size = settings.dock.iconSize;
  const max = settings.dock.magnificationScale;
  const at = pointer;
  appButtons().forEach((b, i) => {
    const px = `${(size * iconScale(at - (restCenters[i] ?? 0), size, max)).toFixed(1)}px`;
    if (b.style.width !== px) b.style.width = b.style.height = px;
  });
  reportLayout();
}

function onPointerMove(e: PointerEvent): void {
  if (!magnifyEnabled() || reorder.dragging()) return;
  const entering = pointer === null;
  pointer = vertical() ? e.clientY : e.clientX;
  if (entering) settle(SETTLE_IN_MS);
  if (!frameQueued) {
    frameQueued = true;
    requestAnimationFrame(applyMagnification);
  }
}

function resetMagnification(): void {
  if (pointer === null && !appButtons().some((b) => b.style.width)) return;
  pointer = null;
  settle(SETTLE_OUT_MS);
  for (const b of appButtons()) b.style.width = b.style.height = "";
}

dock.addEventListener("pointermove", (e) => {
  if ((e.target as Element).closest(".island.apps")) onPointerMove(e);
  else resetMagnification();
  // Button held (press before a reorder drag, click jitter): keep the hover state as is.
  if (e.buttons !== 0) return;
  syncHoverBlock();
  hover.icon(runningIconAt(e.target as Element));
});
dock.addEventListener("pointerleave", () => {
  resetMagnification();
  hover.icon(null);
});

// ---- Hover previews ------------------------------------------------------------------
// Resting on a running app opens its thumbnails popup (hover.ts holds the timing).

const hover = createHoverPreview({
  open: (itemId) => void invoke("dock_hover", { itemId, monitorId }),
  close: () => void invoke("thumbs_close"),
});

/** Id of the running app icon containing `el` (trash and stacks never preview). */
function runningIconAt(el: Element): string | null {
  const btn = el.closest<HTMLElement>(".island.apps > .app");
  const id = btn?.dataset.id;
  if (!btn || !id || btn.classList.contains("trash") || btn.classList.contains("stack")) return null;
  return (items.find((i) => i.id === id)?.windows.length ?? 0) > 0 ? id : null;
}

/** No preview while dragging (reorder or OS files), in Game Mode, or while the dock is hidden. */
function syncHoverBlock(): void {
  hover.setBlocked(reorder.dragging() || dragPos !== null || gameMode !== null || !revealed);
}

// ---- Reorder -----------------------------------------------------------------------

/** A media update arrived mid-drag: rendered when the drag ends. */
let mediaPending = false;

// The click that ends a drag is swallowed (capture phase) before the handler below.
const reorder = attachReorder(dock, {
  vertical,
  iconSize: () => settings?.dock.iconSize ?? 36,
  restCenter: (el) => restCenters[appButtons().indexOf(el)] ?? mainPos(el),
  onDragStart: () => {
    resetMagnification();
    hover.setBlocked(true);
  },
  // The DOM was reordered optimistically: rest centres follow it (render() re-measures).
  onDragEnd: (moved) => {
    if (mediaPending) {
      mediaPending = false;
      render();
    } else if (moved) measureRest();
  },
  // `items` follows the optimistic DOM move, so a render() before Rust re-emits
  // `dock://items` (settings://changed comes first, tray, volume…) keeps the new order.
  // A failed call restores the previous order (unless a newer list already arrived).
  save: (itemId, beforeId) => {
    const previous = items;
    const moved = moveItem(items, itemId, beforeId);
    items = moved;
    invoke("dock_reorder", { itemId, beforeId }).catch(() => {
      if (items !== moved) return; // a newer list arrived meanwhile
      items = previous;
      render();
    });
  },
});

// ---- OS file drops on icons ------------------------------------------------------------
// Tauri reports OS file drags (not pointer events: the reorder above is unaffected).

let dropTarget: string | null = null;
let spring: SpringState = SPRING_IDLE;
let dragPos: { x: number; y: number } | null = null;
let springTimer = 0;
const SPRING_POLL_MS = 100;

function markDropTarget(): void {
  for (const b of appButtons()) b.classList.toggle("drop-target", !!dropTarget && b.dataset.id === dropTarget);
}

function iconUnder(pos: { x: number; y: number }): string | null {
  const dpr = window.devicePixelRatio || 1;
  const rects = appButtons()
    .filter((b) => b.dataset.id && !b.classList.contains("trash") && !b.classList.contains("stack"))
    .map((b) => {
      const r = b.getBoundingClientRect();
      return { id: b.dataset.id ?? "", x: r.left, y: r.top, w: r.width, h: r.height };
    });
  return hitItem(rects, pos.x / dpr, pos.y / dpr);
}

function springTick(): void {
  const hover = dragPos ? iconUnder(dragPos) : null;
  if (hover !== dropTarget) {
    dropTarget = hover;
    markDropTarget();
  }
  const running = !!hover && (items.find((i) => i.id === hover)?.windows.length ?? 0) > 0;
  const r = stepSpring(spring, hover, running, performance.now());
  spring = r.state;
  if (r.fire) void invoke("dock_spring", { itemId: r.fire });
}

function endFileDrag(): void {
  clearInterval(springTimer);
  springTimer = 0;
  dragPos = null;
  syncHoverBlock();
  spring = SPRING_IDLE;
  if (dropTarget) {
    dropTarget = null;
    markDropTarget();
  }
}

void onFileDrag((e) => {
  if (gameMode) return endFileDrag();
  if (e.type === "leave") return endFileDrag();
  dragPos = e.position;
  syncHoverBlock();
  if (e.type === "drop") {
    const id = iconUnder(e.position);
    endFileDrag();
    if (id && e.paths.length > 0) {
      const item = items.find((i) => i.id === id);
      if (item && item.windows.length === 0) bounce.trigger(id);
      void invoke("dock_drop_open", { itemId: id, paths: e.paths });
    }
    return;
  }
  if (!springTimer) springTimer = window.setInterval(springTick, SPRING_POLL_MS);
  springTick();
});

// ---- Clicks, menus, launch feedback ------------------------------------------------

dock.addEventListener("click", (e) => {
  const btn = (e.target as Element).closest<HTMLElement>(".app");
  const id = btn?.dataset.id;
  if (!btn || !id) return;
  const item = items.find((i) => i.id === id);
  if (item && item.windows.length === 0) bounce.trigger(id);
  // Several windows: Rust opens (or keeps) the popup; otherwise it focuses / launches.
  hover.click(id, (item?.windows.length ?? 0) > 1);
  void invoke("dock_click", { itemId: id, monitorId });
});

document.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  const id = (e.target as Element).closest<HTMLElement>(".app")?.dataset.id ?? null;
  void invoke("dock_menu", { itemId: id });
});

// ---- Tray ------------------------------------------------------------------------

// The overflow panel opened or closed: re-measure so the window grows/shrinks around it.
dock.addEventListener("traylayout", () => render());

// The pill's detail panel opened or closed: same as the tray panel.
dock.addEventListener("pilllayout", () => render());

// Stack popover and trash context menu open/close
dock.addEventListener("stacklayout", () => render());
dock.addEventListener("trashlayout", () => render());

function closeTray(): void {
  const tray = document.getElementById("tray");
  if (tray) closeTrayPanel(tray);
  const sys = document.getElementById("sys");
  if (sys) closeVolumePopover(sys);
  closeTrashMenu();
  closeDownloadsStack();
}

// ---- Game Mode ---------------------------------------------------------------------

function applyGame(g: GameState | null): void {
  // The pill follows the mode ("other"), so a mode change below re-renders it.
  game = g;
  if (g?.active) {
    closeDownloadsStack();
    closeTrashMenu();
    bounce.destroy();
  }
  const mode = !g?.active ? null : g.monitorId === monitorId ? "game" : "other";
  if (mode === gameMode) return;
  gameMode = mode;
  syncHoverBlock();
  if (mode) document.body.dataset.game = mode;
  else delete document.body.dataset.game;
  if (!magnifyEnabled()) resetMagnification();
  // The window box shrinks/grows with the magnification headroom.
  render();
}

// ---- Clock ---------------------------------------------------------------------------

function scheduleClock(): void {
  const ms = 60_000 - (Date.now() % 60_000) + 50;
  window.setTimeout(() => {
    if (settings?.dock.showClock) render();
    scheduleClock();
  }, ms);
}

// ---- Boot ----------------------------------------------------------------------------

async function boot(): Promise<void> {
  let itemsFromEvent = false;
  let trayFromEvent = false;
  let volumeFromEvent = false;
  let networkFromEvent = false;
  let micFromEvent = false;
  let mediaFromEvent = false;
  let gameFromEvent = false;
  let audioFromEvent = false;
  // Subscribe first, then fetch: nothing emitted in between is lost, and a list that
  // arrived by event is never overwritten by the (older) fetched one.
  // App-wide events use `on`; events Rust sends to this dock window only use `onWindow`.
  await Promise.all([
    on("dock://items", (list) => {
      itemsFromEvent = true;
      reorder.cancel();
      items = list;
      bounce.update(items);
      render();
    }),
    on("settings://changed", (s) => {
      settings = s;
      if (!magnifyEnabled()) resetMagnification();
      render();
    }),
    onWindow("dock://tone", (t) => {
      autoTone = t;
      document.documentElement.dataset.tone = tone();
    }),
    on("thumbs://pointer", (inside) => hover.popup(inside)),
    on("thumbs://closed", () => hover.closed()),
    on("tray://icons", (list) => {
      trayFromEvent = true;
      trayIcons = list;
      render();
    }),
    on("system://volume", (volume) => {
      volumeFromEvent = true;
      system = { ...system, volume };
      render();
    }),
    // One subscription feeds both the tray mic indicator and the Control Center.
    on("audio://devices", (d) => {
      micFromEvent = true;
      audioFromEvent = true;
      audioDevices = d;
      const before = system.micMuted;
      system = { ...system, micMuted: d.micMuted };
      // A mute toggle (ours peeks the dock 1.5 s): « Micro coupé » / « Micro activé ».
      // Never over a pending game question.
      if (before !== undefined && before !== d.micMuted && currentCard?.kind !== "ask") currentCard = { kind: "mic", muted: d.micMuted };
      render();
    }),
    on("system://network", (network) => {
      networkFromEvent = true;
      system = { ...system, network };
      render();
    }),
    on("media://state", (m) => {
      mediaFromEvent = true;
      media = m;
      // The dock is centred: showing/resizing the media island mid-drag would shift the
      // apps island under the pointer (wrong drop slot). Render on drop instead.
      if (reorder.dragging()) {
        mediaPending = true;
        return;
      }
      render();
    }),
    on("game://state", (g) => {
      gameFromEvent = true;
      applyGame(g);
    }),
    on("cc://state", (c) => {
      ccState = c;
      render();
    }),
    on("sysmon://stats", (s) => {
      stats = s;
      if (pillOn()) render();
    }),
    on("update://state", (st) => {
      update = st;
      render();
    }),
    on("display://switched", (d) => {
      if (currentCard?.kind !== "ask") currentCard = { kind: "display", text: d.text, ok: d.ok };
      render();
    }),
    on("valo://match", (c) => {
      currentCard = { ...c, kind: "match" };
      lastMatch = currentCard;
      render();
    }),
    on("game://ask", (ask) => {
      currentCard = { kind: "ask", exe: ask.exe, name: ask.name };
      render();
    }),
    onWindow("dock://reveal", ({ shown }) => {
      if (!shown) {
        reorder.cancel();
        resetMagnification();
        closeTray();
        closeDownloadsStack();
      }
      dock.classList.toggle("shown", shown);
      if (shown) {
        invoke("get_trash_status")
          .then((st) => {
            trashStatus = st;
            render();
          })
          .catch(() => {});
      }
      // Hidden: the pill comes up (Game Mode); revealed: the dock takes over.
      const pillBefore = pillOn();
      revealed = shown;
      syncHoverBlock();
      if (pillOn() !== pillBefore) render();
    }),
  ]);
  const [s, list, tray, sysState, mediaState, gameNow, trashInit, audioInit] = await Promise.all([
    invoke("get_settings"),
    invoke("get_dock_items"),
    invoke("get_tray_icons"),
    invoke("get_system_state"),
    invoke("get_media"),
    invoke("get_game_state"),
    invoke("get_trash_status").catch(() => null),
    invoke("get_audio_devices").catch(() => null),
  ]);
  // A dock created after the check (new monitor) still shows a waiting update.
  invoke("update_status")
    .then((st) => {
      update ??= st;
      render();
    })
    .catch(() => {});
  if (!audioFromEvent) audioDevices = audioInit ?? null;
  trashStatus = trashInit;
  settings ??= s;
  if (!itemsFromEvent) items = list;
  if (!trayFromEvent) trayIcons = tray ?? [];
  if (!mediaFromEvent) media = mediaState ?? null;
  if (!gameFromEvent) applyGame(gameNow ?? null);
  if (sysState) {
    if (!volumeFromEvent) system = { ...system, volume: sysState.volume };
    if (!networkFromEvent) system = { ...system, network: sysState.network };
    if (!micFromEvent) system = { ...system, micMuted: sysState.micMuted ?? false };
  }
  render();
  scheduleClock();
}

// Resize also fires when the monitor's scale changes (devicePixelRatio).
window.addEventListener("resize", () => reportLayout(true));
void boot();

// Builds the dock islands from the view state. Elements are reused across renders
// (item buttons keyed by id, the `#tray` container never replaced) so icons don't
// reload and launch animations survive item updates.
import type { DockItem, GlassTone, MediaState, Settings, SysStats, SystemState, TrayIconView } from "../shared/types";
import { t } from "../shared/i18n";
import { disposeCard, renderCard, type CardViewState } from "./card";
import { renderMedia } from "./media";
import { disposePill, renderPill } from "./pill";
import { renderSystem } from "./system";
import { renderTray } from "./tray";
import { renderTrash } from "./trash";
import { renderDownloadsStack } from "./stack";
import { classifyShape } from "../shared/iconShape";
import type { AudioDevices, ControlCenterState, TrashStatus } from "../shared/types";

export { isFullBleed } from "../shared/iconShape";

export interface DockViewState {
  items: DockItem[];
  /** Hosted notification-area icons (`tray://icons`). */
  tray: TrayIconView[];
  /** Volume and network (`system://*`). */
  system: SystemState;
  /** Current media session (`media://state`), null when none. */
  media: MediaState | null;
  settings: Settings;
  tone: GlassTone;
  now: Date;
  /** Game Mode system pill (`showPill`), with the last `sysmon://stats`; absent/null: none. */
  pill?: { stats: SysStats | null } | null;
  /** Active match or prompt card; absent/null: none. */
  card?: CardViewState | null;
  /** The card is the pinned last match: no dismissal timer. */
  cardPinned?: boolean;
  onDismissCard?: () => void;
  bouncing?: (id: string) => boolean;
  trash?: TrashStatus | null;
  onEmptyTrash?: () => void;
  showDownloads?: boolean;
  /** Control Center content (`cc://state`, `audio://devices`); absent: not loaded yet. */
  control?: { cc: ControlCenterState | null; audio: AudioDevices | null };
}

function child<K extends keyof HTMLElementTagNameMap>(
  parent: Element,
  selector: string,
  tag: K,
  init: (el: HTMLElementTagNameMap[K]) => void,
): HTMLElementTagNameMap[K] {
  const found = parent.querySelector<HTMLElementTagNameMap[K]>(`:scope > ${selector}`);
  if (found) return found;
  const el = document.createElement(tag);
  init(el);
  parent.appendChild(el);
  return el;
}

function island(root: HTMLElement, kind: string, label: string): HTMLDivElement {
  return child(root, `.island.${kind}`, "div", (el) => {
    el.className = `island ${kind}`;
    el.setAttribute("role", "group");
    el.setAttribute("aria-label", label);
  });
}

function updateButton(btn: HTMLButtonElement, item: DockItem, iconSize: number, isBouncing = false): void {
  btn.setAttribute("aria-label", item.name);
  btn.classList.toggle("game", item.isGame);
  btn.classList.toggle("bouncing", isBouncing);
  // Read by the reorder controller: a drop cannot land past the running unpinned apps.
  btn.dataset.pinned = String(item.pinned);
  let img = btn.querySelector("img");
  if (!img) {
    img = document.createElement("img");
    img.alt = "";
    img.draggable = false;
    img.decoding = "async";
    // Icons come from the `icon` scheme (another origin): CORS lets the canvas read them.
    img.crossOrigin = "anonymous";
    const el = img;
    img.addEventListener("load", () => classifyShape(el, btn));
    btn.prepend(img);
  }
  if (img.getAttribute("src") !== item.iconUrl) img.setAttribute("src", item.iconUrl);
  img.width = iconSize;
  img.height = iconSize;
  const running = item.windows.length > 0;
  const dot = btn.querySelector(".dot");
  if (running && !dot) {
    const d = document.createElement("span");
    d.className = "dot";
    btn.appendChild(d);
  } else if (!running && dot) {
    dot.remove();
  }

  const badge = btn.querySelector(".badge-count");
  if (item.badge) {
    if (!badge) {
      const b = document.createElement("span");
      b.className = "badge-count";
      b.textContent = item.badge;
      btn.appendChild(b);
    } else if (badge.textContent !== item.badge) {
      badge.textContent = item.badge;
    }
  } else if (badge) {
    badge.remove();
  }
}

function renderApps(
  apps: HTMLElement,
  items: DockItem[],
  iconSize: number,
  isBouncing?: (id: string) => boolean,
  trash?: TrashStatus | null,
  onEmptyTrash?: () => void,
  showDownloads?: boolean,
): void {
  const existing = new Map<string, HTMLButtonElement>();
  for (const b of apps.querySelectorAll<HTMLButtonElement>(":scope > .app:not(.trash):not(.stack)")) existing.set(b.dataset.id ?? "", b);
  const sep = apps.querySelector<HTMLElement>(":scope > .sep:not(.utility-sep)") ?? Object.assign(document.createElement("div"), { className: "sep" });

  const regular = items.filter((i) => !i.isGame);
  const games = items.filter((i) => i.isGame);
  const order: HTMLElement[] = [];
  const button = (item: DockItem): HTMLButtonElement => {
    let b = existing.get(item.id);
    existing.delete(item.id);
    if (!b) {
      b = document.createElement("button");
      b.type = "button";
      b.className = "app";
      b.dataset.id = item.id;
      b.tabIndex = -1;
    }
    updateButton(b, item, iconSize, isBouncing ? isBouncing(item.id) : false);
    return b;
  };
  for (const i of regular) order.push(button(i));
  if (regular.length > 0 && games.length > 0) order.push(sep);
  for (const i of games) order.push(button(i));

  const hasUtilities = (showDownloads === true) || (trash !== undefined);
  const utilitySep = apps.querySelector<HTMLElement>(":scope > .sep.utility-sep");
  const trashBtn = apps.querySelector<HTMLElement>(":scope > .app.trash");
  const stackBtn = apps.querySelector<HTMLElement>(":scope > .app.stack");

  if (hasUtilities && (regular.length > 0 || games.length > 0)) {
    const sepEl = utilitySep ?? Object.assign(document.createElement("div"), { className: "sep utility-sep" });
    order.push(sepEl);
  } else if (utilitySep) {
    utilitySep.remove();
  }

  if (showDownloads) {
    order.push(renderDownloadsStack(apps));
  } else if (stackBtn) {
    stackBtn.remove();
  }

  if (trash !== undefined) {
    order.push(renderTrash(apps, trash, onEmptyTrash));
  } else if (trashBtn) {
    trashBtn.remove();
  }

  for (const b of existing.values()) b.remove();
  if (!order.includes(sep)) sep.remove();
  // Move only what is out of place (keeps nodes and their running animations).
  order.forEach((el, idx) => {
    if (apps.children[idx] !== el) apps.insertBefore(el, apps.children[idx] ?? null);
  });
}

export function renderDock(root: HTMLElement, s: DockViewState): void {
  const { dock, general } = s.settings;
  root.ownerDocument.documentElement.dataset.tone = s.tone;
  root.style.setProperty("--icon", `${dock.iconSize}px`);
  root.setAttribute("aria-label", t.dock.label);

  // Media island: only with a session and the setting on; otherwise removed (render()
  // then re-reports the layout, so the window region and the glass follow).
  let media: HTMLDivElement | null = null;
  if (dock.showMedia && s.media) {
    media = island(root, "media", t.media.media);
    renderMedia(media, s.media);
  } else {
    root.querySelector(":scope > .island.media")?.remove();
  }

  const apps = island(root, "apps", t.dock.apps);
  renderApps(apps, s.items, dock.iconSize, s.bouncing, s.trash, s.onEmptyTrash, s.showDownloads);

  const tray = island(root, "tray", t.dock.tray);
  const trayIcons = child(tray, "#tray", "div", (el) => (el.id = "tray"));
  renderTray(trayIcons, s.tray);

  // System icons + the single clock live in the tray island, after the tray icons.
  const sys = child(tray, "#sys", "div", (el) => (el.id = "sys"));
  renderSystem(sys, {
    volume: s.system.volume,
    network: s.system.network,
    showClock: dock.showClock,
    now: s.now,
    lang: general.language,
    controlCenter: s.settings.controlCenter ?? true,
    cc: s.control?.cc ?? null,
    audio: s.control?.audio ?? null,
    display: s.settings.displaySwitch?.monitor
      ? { monitor: s.settings.displaySwitch.monitor, label: s.settings.displaySwitch.otherLabel || "Autre PC" }
      : null,
    micMuted: s.system.micMuted ?? false,
  });

  // System pill: last, so apps/tray keep their place; in pill mode the other islands are
  // hidden (dock.css) and only the pill (and its panel) is reported to Rust.
  let pill: HTMLDivElement | null = null;
  if (s.pill) {
    pill = island(root, "pill", t.pill.label);
    renderPill(pill, s.pill.stats);
  } else {
    const old = root.querySelector<HTMLElement>(":scope > .island.pill");
    if (old) {
      disposePill(old);
      old.remove();
    }
  }
  root.classList.toggle("pill-mode", pill !== null);

  // Card island: match summary or game ask prompt.
  let card: HTMLDivElement | null = null;
  if (s.card) {
    card = island(root, "card", "Carte");
    renderCard(card, s.card, {
      durationMs: s.cardPinned ? undefined : s.settings.animations.cardDurationMs,
      onDismiss: s.onDismissCard,
    });
  } else {
    const old = root.querySelector<HTMLElement>(":scope > .island.card");
    if (old) {
      disposeCard();
      old.remove();
    }
  }

  // Island order: media, apps, tray, pill, card.
  [media, apps, tray, pill, card].filter((el): el is HTMLDivElement => el !== null).forEach((el, idx) => {
    if (root.children[idx] !== el) root.insertBefore(el, root.children[idx] ?? null);
  });
}

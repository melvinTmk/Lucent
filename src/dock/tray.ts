// Tray island content (inside the stable `#tray` container): promoted icons inline, the
// others behind a chevron that toggles a glass overflow panel (4 per row). Left, right
// and double clicks go to Rust as `tray_click` with the icon centre (physical px,
// relative to the dock window). Elements are reused across renders, keyed by icon key.
// The panel always ends with a Lucent cell (left click: settings; right click: the dock
// menu), so the chevron is always shown.
import { invoke } from "../shared/ipc";
import { t } from "../shared/i18n";
import type { TrayButton, TrayIconView } from "../shared/types";

/** Containers whose overflow panel is open. */
const openPanels = new WeakSet<HTMLElement>();
const lastIcons = new WeakMap<HTMLElement, TrayIconView[]>();
const wired = new WeakSet<HTMLElement>();
const outsideHandlers = new WeakMap<HTMLElement, (e: Event) => void>();

const CHEVRON_SVG =
  '<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M4 10l4-4 4 4" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>';

/** App icon served from `public/` (same origin as the dock page). */
const LUCENT_ICON = "/lucent.png";

function center(el: HTMLElement): { x: number; y: number } {
  const r = el.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  return { x: Math.round((r.left + r.width / 2) * dpr), y: Math.round((r.top + r.height / 2) * dpr) };
}

function send(btn: HTMLElement, button: TrayButton): void {
  const key = btn.dataset.key;
  if (!key) return;
  const { x, y } = center(btn);
  void invoke("tray_click", { key, button, x, y });
}

/** Tells the dock page the tray changed size (panel opened/closed) so it re-lays out. */
function notifyLayout(container: HTMLElement): void {
  container.dispatchEvent(new CustomEvent("traylayout", { bubbles: true }));
}

function setOpen(container: HTMLElement, open: boolean): void {
  if (open === openPanels.has(container)) return;
  const doc = container.ownerDocument;
  if (open) {
    openPanels.add(container);
    const onOutside = (e: Event) => {
      if (!container.contains(e.target as Node)) setOpen(container, false);
    };
    outsideHandlers.set(container, onOutside);
    doc.addEventListener("pointerdown", onOutside, true);
  } else {
    openPanels.delete(container);
    const h = outsideHandlers.get(container);
    if (h) doc.removeEventListener("pointerdown", h, true);
    outsideHandlers.delete(container);
  }
  renderTray(container, lastIcons.get(container) ?? []);
  notifyLayout(container);
}

/** Closes the overflow panel (e.g. when the dock slides away). */
export function closeTrayPanel(container: HTMLElement): void {
  setOpen(container, false);
}

function wire(container: HTMLElement): void {
  if (wired.has(container)) return;
  wired.add(container);
  container.addEventListener("click", (e) => {
    const target = e.target as Element;
    if (target.closest(".tray-chevron")) {
      setOpen(container, !openPanels.has(container));
      return;
    }
    if (target.closest(".tray-lucent")) {
      void invoke("open_settings");
      setOpen(container, false);
      return;
    }
    const btn = target.closest<HTMLElement>(".tray-icon");
    // The second click of a double click is sent as "double" by the dblclick handler.
    if (!btn || e.detail >= 2) return;
    const inPanel = !!btn.closest(".tray-panel");
    send(btn, "left");
    if (inPanel) setOpen(container, false);
  });
  container.addEventListener("dblclick", (e) => {
    const btn = (e.target as Element).closest<HTMLElement>(".tray-icon");
    if (btn) send(btn, "double");
  });
  container.addEventListener("contextmenu", (e) => {
    // Never the dock's own context menu over the tray.
    e.preventDefault();
    e.stopPropagation();
    if ((e.target as Element).closest(".tray-lucent")) {
      void invoke("dock_menu", { itemId: null });
      setOpen(container, false);
      return;
    }
    const btn = (e.target as Element).closest<HTMLElement>(".tray-icon");
    if (!btn) return;
    const inPanel = !!btn.closest(".tray-panel");
    send(btn, "right");
    if (inPanel) setOpen(container, false);
  });
}

function updateIcon(btn: HTMLButtonElement, icon: TrayIconView): void {
  if (btn.title !== icon.tip) btn.title = icon.tip;
  btn.setAttribute("aria-label", icon.tip);
  let img = btn.querySelector("img");
  if (!img) {
    img = document.createElement("img");
    img.alt = "";
    img.draggable = false;
    img.decoding = "async";
    btn.appendChild(img);
  }
  if (img.getAttribute("src") !== icon.iconUrl) img.setAttribute("src", icon.iconUrl);
}

/** Makes the `.tray-icon` children of `parent` match `icons` (in order), starting at `start`. */
function syncIcons(parent: HTMLElement, icons: TrayIconView[], start: number): void {
  const existing = new Map<string, HTMLButtonElement>();
  for (const b of parent.querySelectorAll<HTMLButtonElement>(":scope > .tray-icon")) existing.set(b.dataset.key ?? "", b);
  const order = icons.map((icon) => {
    let b = existing.get(icon.key);
    existing.delete(icon.key);
    if (!b) {
      b = document.createElement("button");
      b.type = "button";
      b.className = "tray-icon";
      b.dataset.key = icon.key;
      b.tabIndex = -1;
    }
    updateIcon(b, icon);
    return b;
  });
  for (const b of existing.values()) b.remove();
  order.forEach((el, i) => {
    const at = parent.children[start + i] ?? null;
    if (at !== el) parent.insertBefore(el, at);
  });
}

/** The panel's last cell: the Lucent app icon (opens the settings). */
function lucentCell(panel: HTMLElement): HTMLButtonElement {
  let b = panel.querySelector<HTMLButtonElement>(":scope > .tray-lucent");
  if (!b) {
    b = document.createElement("button");
    b.type = "button";
    b.className = "tray-lucent";
    b.tabIndex = -1;
    b.title = t.tray.lucentSettings;
    b.setAttribute("aria-label", t.tray.lucentSettings);
    const img = document.createElement("img");
    img.alt = "";
    img.draggable = false;
    img.decoding = "async";
    img.src = LUCENT_ICON;
    b.appendChild(img);
  }
  return b;
}

export function renderTray(container: HTMLElement, icons: TrayIconView[]): void {
  wire(container);
  lastIcons.set(container, icons);
  const inline = icons.filter((i) => i.promoted);
  const hidden = icons.filter((i) => !i.promoted);

  // Chevron first (as in Windows), then the promoted icons. Always present: the panel
  // holds at least the Lucent cell.
  let chevron = container.querySelector<HTMLButtonElement>(":scope > .tray-chevron");
  if (!chevron) {
    chevron = document.createElement("button");
    chevron.type = "button";
    chevron.className = "tray-chevron";
    chevron.tabIndex = -1;
    chevron.innerHTML = CHEVRON_SVG;
  }
  if (container.firstElementChild !== chevron) container.insertBefore(chevron, container.firstElementChild);
  const open = openPanels.has(container);
  chevron.setAttribute("aria-expanded", String(open));
  const label = open ? t.tray.hideHidden : t.tray.showHidden;
  chevron.setAttribute("aria-label", label);
  chevron.title = label;
  syncIcons(container, inline, 1);

  let panel = container.querySelector<HTMLElement>(":scope > .tray-panel");
  if (open) {
    if (!panel) {
      panel = document.createElement("div");
      panel.className = "island tray-panel";
      panel.setAttribute("role", "group");
      panel.setAttribute("aria-label", t.tray.hiddenIcons);
    }
    if (container.lastElementChild !== panel) container.appendChild(panel);
    syncIcons(panel, hidden, 0);
    const gd = lucentCell(panel);
    if (panel.lastElementChild !== gd) panel.appendChild(gd);
  } else {
    panel?.remove();
  }
}

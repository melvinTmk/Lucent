// Switcher island rendering (pure DOM): one slot per app, a selection plate that glides
// between slots, the selected app's name under the row, and (↓) one card per window around
// the live DWM thumbnails Rust draws over the window.
import { t } from "../shared/i18n";
import type { SwitcherShow, SwitcherWindows } from "../shared/types";

/** CSS px; mirror `switcher.rs` (PAD, CELL, ICON). */
export const PAD = 14;
export const CELL = 88;
const ICON_SHARE = 64 / 88;
/** Card margin around a window thumbnail. */
const CARD_PAD = 8;

export type Pick = (appId: string, hwnd?: number) => void;

interface State {
  show: SwitcherShow;
  cell: number;
}

const states = new WeakMap<HTMLElement, State>();

export function renderSwitcher(root: HTMLElement, show: SwitcherShow, pick: Pick): void {
  const cell = show.cell ?? CELL;
  states.set(root, { show, cell });
  root.setAttribute("role", "listbox");
  root.setAttribute("aria-label", t.switcher.label);
  root.classList.remove("expanded");
  root.style.setProperty("--cell", `${cell}px`);
  const sel = document.createElement("div");
  sel.className = "sel";
  const slots = show.apps.map((a, i) => {
    const slot = document.createElement("div");
    slot.className = "slot";
    slot.dataset.id = a.id;
    slot.setAttribute("role", "option");
    slot.setAttribute("aria-label", a.name);
    slot.style.left = `${PAD + i * cell}px`;
    slot.style.top = `${PAD}px`;
    slot.style.width = slot.style.height = `${cell}px`;
    const img = document.createElement("img");
    img.alt = "";
    img.draggable = false;
    img.style.width = img.style.height = `${Math.round(cell * ICON_SHARE)}px`;
    img.src = a.iconUrl;
    slot.append(img);
    slot.addEventListener("click", () => pick(a.id, undefined));
    return slot;
  });
  const name = document.createElement("div");
  name.className = "name";
  name.style.top = `${PAD + cell + 2}px`;
  const windows = document.createElement("div");
  windows.className = "windows";
  root.replaceChildren(sel, ...slots, name, windows);
  selectApp(root, show.index, true);
}

/** Moves the selection to app `index` (ignored when out of range). */
export function selectApp(root: HTMLElement, index: number, instant = false): void {
  const st = states.get(root);
  const app = st?.show.apps[index];
  if (!st || !app) return;
  st.show = { ...st.show, index };
  const sel = root.querySelector<HTMLElement>(".sel");
  if (sel) {
    sel.classList.toggle("instant", instant);
    sel.style.top = `${PAD}px`;
    sel.style.width = sel.style.height = `${st.cell}px`;
    sel.style.transform = `translateX(${PAD + index * st.cell}px)`;
  }
  root.querySelectorAll<HTMLElement>(".slot").forEach((s, i) => s.setAttribute("aria-selected", String(i === index)));
  const name = root.querySelector<HTMLElement>(".name");
  if (name) {
    name.textContent = app.name;
    // Centred under the icon, kept inside the island.
    const half = name.offsetWidth / 2;
    const width = root.clientWidth || PAD * 2 + st.show.apps.length * st.cell;
    const centre = PAD + (index + 0.5) * st.cell;
    const left = half > 0 ? Math.min(Math.max(centre, PAD + half), width - PAD - half) : centre;
    name.style.left = `${left}px`;
  }
}

/** Opens (or closes, `null`) the windows row of the selected app. */
export function renderWindows(root: HTMLElement, w: SwitcherWindows | null, pick: Pick): void {
  const row = root.querySelector<HTMLElement>(".windows");
  const st = states.get(root);
  if (!row) return;
  if (!w || !st) {
    row.replaceChildren();
    root.classList.remove("expanded");
    return;
  }
  const app = st.show.apps[w.appIndex];
  const cellH = Math.max(0, ...w.rects.map((r) => r.h)) + w.titleH;
  const cards = w.rects.map((r, i) => {
    const card = document.createElement("div");
    card.className = "win";
    card.setAttribute("role", "option");
    const title = w.titles[i]?.trim() || t.switcher.untitled;
    card.title = title;
    card.style.left = `${r.x / w.scale - CARD_PAD}px`;
    card.style.top = `${r.y / w.scale - CARD_PAD}px`;
    card.style.width = `${r.w / w.scale + 2 * CARD_PAD}px`;
    card.style.height = `${cellH / w.scale + CARD_PAD}px`;
    const label = document.createElement("div");
    label.className = "title";
    label.textContent = title;
    card.append(label);
    const hwnd = w.hwnds[i];
    card.addEventListener("click", (e) => {
      e.stopPropagation();
      if (app) pick(app.id, hwnd);
    });
    return card;
  });
  row.replaceChildren(...cards);
  root.classList.add("expanded");
  selectWindow(root, w.index);
}

export function selectWindow(root: HTMLElement, index: number): void {
  root.querySelectorAll<HTMLElement>(".win").forEach((c, i) => c.setAttribute("aria-selected", String(i === index)));
}

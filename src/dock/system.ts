// System icons at the end of the tray island (inside the stable `#sys` container):
// volume (click: slider popover, wheel: +-5 %, middle click: mute), network, the
// notifications bell and the clock (click: Control Center, see control.ts). Nodes are
// reused across renders. At most one popup (volume popover or Control Center) is open.
import { invoke } from "../shared/ipc";
import { t } from "../shared/i18n";
import type { AudioDevices, ControlCenterState, NetworkState, VolumeState } from "../shared/types";
import { renderControlCenter } from "./control";

export interface SystemView {
  volume: VolumeState | null;
  network: NetworkState;
  showClock: boolean;
  now: Date;
  lang: string;
  /** Clock click opens the Control Center (`settings.controlCenter`). */
  controlCenter?: boolean;
  /** Last `cc://state` / `audio://devices` (Control Center content). */
  cc?: ControlCenterState | null;
  audio?: AudioDevices | null;
  /** Shared monitor, when configured (Control Center switch row). */
  display?: { monitor: string; label: string } | null;
  /** Default communications mic muted: a red slashed mic before the volume icon. */
  micMuted?: boolean;
}

type Popup = "volume" | "cc";

/** The clock hover asks Rust for a fresh `cc://state` at most this often (prefetch). */
const PREFETCH_MS = 10_000;

const WHEEL_STEP = 0.05;

const svg = (body: string) =>
  `<svg viewBox="0 0 24 24" width="100%" height="100%" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`;

const SPEAKER = '<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z" fill="currentColor"/>';
const VOLUME_SVG = {
  mute: svg(`${SPEAKER}<path d="M16 9.5l5 5M21 9.5l-5 5"/>`),
  low: svg(`${SPEAKER}<path d="M15.5 10a3 3 0 0 1 0 4"/>`),
  mid: svg(`${SPEAKER}<path d="M15.5 9.5a4 4 0 0 1 0 5"/>`),
  high: svg(`${SPEAKER}<path d="M15.5 9.5a4 4 0 0 1 0 5"/><path d="M18 7a7.5 7.5 0 0 1 0 10"/>`),
} as const;
const NET_SVG = {
  wifi: svg('<path d="M2.5 9.5a14 14 0 0 1 19 0"/><path d="M5.5 13a9.5 9.5 0 0 1 13 0"/><path d="M8.7 16.3a5 5 0 0 1 6.6 0"/><circle cx="12" cy="19.5" r=".6" fill="currentColor"/>'),
  ethernet: svg('<rect x="4" y="4" width="16" height="9" rx="2"/><path d="M8 13v3M12 13v3M16 13v3M12 16v4M8 20h8"/>'),
  none: svg('<path d="M2.5 9.5a14 14 0 0 1 4-2.7M21.5 9.5a14 14 0 0 0-7-3.4"/><path d="M5.5 13a9.5 9.5 0 0 1 3-1.9M18.5 13a9.5 9.5 0 0 0-2.3-1.6"/><path d="M8.7 16.3a5 5 0 0 1 2-1.1"/><path d="M3 3l18 18"/>'),
} as const;
const MIC_MUTED_SVG = svg('<rect x="9" y="3" width="6" height="11" rx="3" fill="currentColor"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21M8.5 21h7"/><path d="M3.5 3.5l17 17"/>');
const BELL_SVG = svg('<path d="M6 16.5V11a6 6 0 0 1 12 0v5.5l1.5 2h-15z" fill="currentColor"/><path d="M10 21a2.2 2.2 0 0 0 4 0"/>');

const openPopups = new WeakMap<HTMLElement, Popup>();
const lastPrefetch = new WeakMap<HTMLElement, number>();
const lastView = new WeakMap<HTMLElement, SystemView>();
const wired = new WeakSet<HTMLElement>();
const outsideHandlers = new WeakMap<HTMLElement, (e: Event) => void>();

function notifyLayout(container: HTMLElement): void {
  container.dispatchEvent(new CustomEvent("traylayout", { bubbles: true }));
}

function refreshControlCenter(container: HTMLElement): void {
  lastPrefetch.set(container, performance.now());
  invoke("cc_refresh").catch(() => {});
}

function setOpen(container: HTMLElement, open: Popup | null): void {
  const current = openPopups.get(container) ?? null;
  if (open === current) return;
  const doc = container.ownerDocument;
  if (open) {
    openPopups.set(container, open);
    if (!outsideHandlers.has(container)) {
      const onOutside = (e: Event) => {
        if (!container.contains(e.target as Node)) setOpen(container, null);
      };
      outsideHandlers.set(container, onOutside);
      doc.addEventListener("pointerdown", onOutside, true);
    }
    if (open === "cc") refreshControlCenter(container);
  } else {
    openPopups.delete(container);
    const h = outsideHandlers.get(container);
    if (h) doc.removeEventListener("pointerdown", h, true);
    outsideHandlers.delete(container);
  }
  const v = lastView.get(container);
  if (v) renderSystem(container, v);
  notifyLayout(container);
}

/** Closes the volume popover or the Control Center (e.g. when the dock slides away). */
export function closeVolumePopover(container: HTMLElement): void {
  setOpen(container, null);
}

function toggle(container: HTMLElement, which: Popup): void {
  setOpen(container, openPopups.get(container) === which ? null : which);
}

function wheelLevel(from: number, deltaY: number): number {
  const step = deltaY < 0 ? WHEEL_STEP : -WHEEL_STEP;
  return Math.min(1, Math.max(0, Math.round((from + step) * 100) / 100));
}

function wire(container: HTMLElement): void {
  if (wired.has(container)) return;
  wired.add(container);
  container.addEventListener("click", (e) => {
    const target = e.target as Element;
    if (target.closest(".sys-mic")) void invoke("mic_toggle");
    else if (target.closest(".sys-volume")) toggle(container, "volume");
    else if (target.closest(".sys-clock")) {
      if (lastView.get(container)?.controlCenter) toggle(container, "cc");
    } else if (target.closest(".sys-network")) void invoke("open_network");
    else if (target.closest(".sys-bell")) void invoke("open_notifications");
  });
  // Hovering the clock prefetches the Control Center state (radios, DDC are slow to read),
  // so the panel opens with fresh values.
  container.addEventListener("pointerover", (e) => {
    if (!(e.target as Element).closest(".sys-clock") || !lastView.get(container)?.controlCenter) return;
    if (performance.now() - (lastPrefetch.get(container) ?? -Infinity) > PREFETCH_MS) refreshControlCenter(container);
  });
  // Ne pas déranger hands over to the Windows settings: the panel goes away.
  container.addEventListener("cc-close", () => setOpen(container, null));
  container.addEventListener("auxclick", (e) => {
    if (e.button === 1 && (e.target as Element).closest(".sys-volume")) {
      e.preventDefault();
      void invoke("toggle_mute");
    }
  });
  // Stops the middle-click autoscroll cursor.
  container.addEventListener("mousedown", (e) => {
    if (e.button === 1) e.preventDefault();
  });
  container.addEventListener("wheel", (e) => {
    if (!(e.target as Element).closest(".sys-volume, .vol-pop")) return;
    e.preventDefault();
    const level = wheelLevel(lastView.get(container)?.volume?.level ?? 0, e.deltaY);
    const v = lastView.get(container);
    if (v?.volume) lastView.set(container, { ...v, volume: { ...v.volume, level } });
    void invoke("set_volume", { level });
  });
  container.addEventListener("input", (e) => {
    const el = e.target as HTMLInputElement;
    if (!el.matches(".vol-pop input")) return;
    void invoke("set_volume", { level: Math.min(1, Math.max(0, Number(el.value) / 100)) });
  });
  // Never the dock's own context menu over the system icons.
  container.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    e.stopPropagation();
  });
}

function button(container: HTMLElement, cls: string): HTMLButtonElement {
  let b = container.querySelector<HTMLButtonElement>(`:scope > .${cls}`);
  if (!b) {
    b = document.createElement("button");
    b.type = "button";
    b.className = `sys-icon ${cls}`;
    b.tabIndex = -1;
  }
  return b;
}

function setHtml(el: HTMLElement, html: string): void {
  if (el.dataset.svg !== html) {
    el.innerHTML = html;
    el.dataset.svg = html;
  }
}

function label(el: HTMLElement, text: string): void {
  if (el.title !== text) el.title = text;
  el.setAttribute("aria-label", text);
}

function renderPopover(container: HTMLElement, vol: VolumeState | null, open: boolean): void {
  let pop = container.querySelector<HTMLElement>(":scope > .vol-pop");
  if (!open) {
    pop?.remove();
    return;
  }
  if (!pop) {
    pop = document.createElement("div");
    pop.className = "island vol-pop";
    pop.setAttribute("role", "group");
    pop.setAttribute("aria-label", t.system.volume);
    const slider = document.createElement("input");
    slider.type = "range";
    slider.min = "0";
    slider.max = "100";
    slider.step = "1";
    slider.setAttribute("aria-label", t.system.volume);
    pop.appendChild(slider);
    container.appendChild(pop);
  }
  const slider = pop.querySelector("input")!;
  const value = String(Math.round((vol?.level ?? 0) * 100));
  // Don't fight the user's drag.
  if (slider.value !== value && container.ownerDocument.activeElement !== slider) slider.value = value;
}

function renderControlPanel(container: HTMLElement, view: SystemView, open: boolean): void {
  let panel = container.querySelector<HTMLElement>(":scope > .cc-panel");
  if (!open) {
    panel?.remove();
    return;
  }
  if (!panel) {
    panel = document.createElement("div");
    panel.className = "island cc-panel";
    container.appendChild(panel);
  }
  renderControlCenter(panel, {
    cc: view.cc ?? null,
    audio: view.audio ?? null,
    volume: view.volume,
    network: view.network,
    display: view.display ?? null,
  });
}

function renderClock(container: HTMLElement, now: Date, lang: string, show: boolean, cc: boolean, open: boolean): void {
  let clock = container.querySelector<HTMLElement>(":scope > .sys-clock");
  if (!show) {
    clock?.remove();
    return;
  }
  if (!clock) {
    clock = document.createElement("time");
    clock.className = "sys-clock";
    container.appendChild(clock);
  }
  const locale = lang || "fr";
  const hm = new Intl.DateTimeFormat(locale, { hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(now);
  const day = new Intl.DateTimeFormat(locale, { weekday: "short", day: "numeric", month: "short" }).format(now);
  (clock as HTMLTimeElement).dateTime = now.toISOString();
  if (clock.textContent !== hm) clock.textContent = hm;
  if (clock.title !== day) clock.title = day;
  // Opens the Control Center: announced as a button.
  clock.classList.toggle("cc-enabled", cc);
  if (cc) {
    clock.setAttribute("role", "button");
    clock.setAttribute("aria-haspopup", "dialog");
    clock.setAttribute("aria-expanded", String(open));
    clock.setAttribute("aria-label", `${day} ${hm}, ${t.cc.open}`);
  } else {
    for (const a of ["role", "aria-haspopup", "aria-expanded", "aria-label"]) clock.removeAttribute(a);
  }
}

function renderMic(container: HTMLElement, vol: HTMLElement, muted: boolean): void {
  let mic = container.querySelector<HTMLButtonElement>(":scope > .sys-mic");
  if (!muted) {
    // Called from the dock's render, which measures and reports the layout itself.
    mic?.remove();
    return;
  }
  if (!mic) {
    mic = button(container, "sys-mic");
    container.insertBefore(mic, vol);
  }
  setHtml(mic, MIC_MUTED_SVG);
  mic.dataset.muted = "true";
  label(mic, t.mic.muted);
}

export function renderSystem(container: HTMLElement, view: SystemView): void {
  wire(container);
  lastView.set(container, view);
  // Control Center turned off (or the clock hidden) while open: close it.
  if (openPopups.get(container) === "cc" && (!view.controlCenter || !view.showClock)) {
    setOpen(container, null);
    return;
  }
  const popup = openPopups.get(container) ?? null;

  const vol = button(container, "sys-volume");
  const net = button(container, "sys-network");
  const bell = button(container, "sys-bell");
  for (const b of [vol, net, bell]) if (b.parentElement !== container) container.appendChild(b);

  const icon = view.volume?.icon ?? "mute";
  setHtml(vol, VOLUME_SVG[icon]);
  label(vol, view.volume ? `${view.volume.muted ? t.system.volumeMuted : t.system.volume} ${Math.round(view.volume.level * 100)} %` : t.system.volume);
  vol.setAttribute("aria-expanded", String(popup === "volume"));
  vol.dataset.icon = icon;

  const kind = view.network.kind;
  setHtml(net, NET_SVG[kind]);
  net.dataset.kind = kind;
  label(net, view.network.name || t.system.network[kind]);

  setHtml(bell, BELL_SVG);
  label(bell, t.system.notifications);

  renderMic(container, vol, view.micMuted === true);

  renderClock(container, view.now, view.lang, view.showClock, !!view.controlCenter, popup === "cc");
  renderPopover(container, view.volume, popup === "volume");
  renderControlPanel(container, view, popup === "cc");
}

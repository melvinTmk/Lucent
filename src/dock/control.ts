// Control Center (macOS style), opened from the dock clock: a glass panel above the tray
// island with a 2-column grid — connectivity module (Wi-Fi, Bluetooth, Micro) next to the
// Ne pas déranger and Mode sombre tiles — then the Écran (brightness) and Son (volume +
// output list) modules. Missing hardware (null members of `cc://state`) hides its tile.
// Toggles flip at once (pending value) until Rust confirms with a new state.
// Nodes are created once per panel and updated in place.
import { invoke } from "../shared/ipc";
import { t } from "../shared/i18n";
import { POWER_IDS, confirmText, isDestructive, isPowerId, powerLabel, runPower, type PowerId } from "../shared/power";
import type { AudioDevices, ControlCenterState, NetworkState, VolumeState } from "../shared/types";

export interface ControlView {
  /** Last `cc://state` (null until the first one arrives). */
  cc: ControlCenterState | null;
  /** Last `audio://devices`. */
  audio: AudioDevices | null;
  volume: VolumeState | null;
  /** Live network state: its Wi-Fi name is fresher than the one in `cc`. */
  network: NetworkState;
  /** Shared monitor (Réglages › Écran partagé) when configured: a switch row. */
  display?: { monitor: string; label: string } | null;
}

type ToggleKey = "wifi" | "bluetooth" | "dark" | "mic";

/** An optimistic value wins over the received state for this long at most. */
const PENDING_MS = 4000;
/** A slider the user moved recently is not overwritten by incoming states. */
const DRAG_GUARD_MS = 900;

interface PanelState {
  view: ControlView;
  pending: Map<string, { value: unknown; until: number }>;
  lastInput: Map<string, number>;
  /** Destructive power action awaiting its inline confirmation. */
  confirming: PowerId | null;
}

const states = new WeakMap<HTMLElement, PanelState>();

const svg = (body: string) =>
  `<svg viewBox="0 0 24 24" width="100%" height="100%" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`;

const ICONS = {
  wifi: svg('<path d="M2.5 9.5a14 14 0 0 1 19 0"/><path d="M5.5 13a9.5 9.5 0 0 1 13 0"/><path d="M8.7 16.3a5 5 0 0 1 6.6 0"/><circle cx="12" cy="19.5" r=".8" fill="currentColor"/>'),
  bluetooth: svg('<path d="M7 7l10 10-5 4V3l5 4L7 17"/>'),
  mic: svg('<rect x="9" y="3" width="6" height="11" rx="3" fill="currentColor" stroke="none"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21"/>'),
  micOff: svg('<path d="M15 9.5V6a3 3 0 0 0-5.8-1.1M9 9v2a3 3 0 0 0 4.6 2.5"/><path d="M5.5 11a6.5 6.5 0 0 0 10.7 5M18.5 11a6.5 6.5 0 0 1-.6 2.7M12 17.5V21M3 3l18 18"/>'),
  moon: svg('<path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z" fill="currentColor" stroke="none"/>'),
  dark: svg('<circle cx="12" cy="12" r="8.5"/><path d="M12 3.5a8.5 8.5 0 0 1 0 17z" fill="currentColor"/>'),
  sun: svg('<circle cx="12" cy="12" r="3.6" fill="currentColor" stroke="none"/><path d="M12 3v1.8M12 19.2V21M3 12h1.8M19.2 12H21M5.6 5.6l1.3 1.3M17.1 17.1l1.3 1.3M5.6 18.4l1.3-1.3M17.1 6.9l1.3-1.3"/>'),
  speaker: svg('<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z" fill="currentColor"/><path d="M15.5 9.5a4 4 0 0 1 0 5"/>'),
  output: svg('<rect x="6" y="3" width="12" height="18" rx="2.5"/><circle cx="12" cy="14.5" r="3"/><circle cx="12" cy="7.5" r=".8" fill="currentColor"/>'),
  check: svg('<path d="M5 12.5l4.5 4.5L19 7.5"/>'),
  lock: svg('<rect x="5" y="11" width="14" height="9.5" rx="2.5"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>'),
  sleep: svg('<path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z"/>'),
  restart: svg('<path d="M19.5 12a7.5 7.5 0 1 1-2.4-5.5"/><path d="M19.5 4v4.5H15"/>'),
  shutdown: svg('<path d="M12 3v8.5"/><path d="M6.8 6.6a8 8 0 1 0 10.4 0"/>'),
  screen: svg('<rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/><path d="M9.5 10h5M12.5 8l2 2-2 2"/>'),
  signout: svg('<path d="M10 4H6.5A2.5 2.5 0 0 0 4 6.5v11A2.5 2.5 0 0 0 6.5 20H10"/><path d="M15 8l4 4-4 4M19 12H9.5"/>'),
} as const;

// ---- Pending (optimistic) values -------------------------------------------------------

function effective<T>(st: PanelState, key: string, actual: T): T {
  const p = st.pending.get(key);
  if (!p) return actual;
  if (performance.now() > p.until || p.value === actual) {
    st.pending.delete(key);
    return actual;
  }
  return p.value as T;
}

function setPending(st: PanelState, key: string, value: unknown): void {
  st.pending.set(key, { value, until: performance.now() + PENDING_MS });
}

/** Received toggle values; undefined when the tile is hidden. */
function actualToggle(v: ControlView, key: ToggleKey): boolean | undefined {
  switch (key) {
    case "wifi":
      return v.cc?.wifi?.on;
    case "bluetooth":
      return v.cc?.bluetooth?.on;
    case "dark":
      return v.cc?.darkMode ?? false;
    case "mic":
      return !(v.audio?.micMuted ?? false);
  }
}

function toggleValue(st: PanelState, key: ToggleKey): boolean | undefined {
  const actual = actualToggle(st.view, key);
  return actual === undefined ? undefined : effective(st, key, actual);
}

// ---- Events --------------------------------------------------------------------------

function onToggle(panel: HTMLElement, st: PanelState, key: ToggleKey | "dnd"): void {
  if ((key as string) === "screen") {
    void invoke("display_switch", {});
    panel.dispatchEvent(new CustomEvent("cc-close", { bubbles: true }));
    return;
  }
  if (key === "dnd") {
    void invoke("open_external", { url: "ms-settings:notifications" });
    panel.dispatchEvent(new CustomEvent("cc-close", { bubbles: true }));
    return;
  }
  const current = toggleValue(st, key);
  if (current === undefined) return;
  const next = !current;
  setPending(st, key, next);
  const fail = () => {
    st.pending.delete(key);
    if (panel.isConnected) renderControlCenter(panel, st.view);
  };
  if (key === "wifi") invoke("cc_set_wifi", { on: next }).catch(fail);
  else if (key === "bluetooth") invoke("cc_set_bluetooth", { on: next }).catch(fail);
  else if (key === "dark") invoke("cc_set_dark", { on: next }).catch(fail);
  else invoke("mic_toggle").catch(fail);
  renderControlCenter(panel, st.view);
}

/** Power row: lock and sleep act at once; the destructive ones switch to an inline confirm. */
function onPower(panel: HTMLElement, st: PanelState, target: Element): boolean {
  const btn = target.closest<HTMLElement>("[data-power]");
  const id = btn?.dataset.power ?? "";
  if (btn && isPowerId(id)) {
    if (isDestructive(id)) {
      st.confirming = id;
      renderControlCenter(panel, st.view);
    } else {
      void runPower(id);
      panel.dispatchEvent(new CustomEvent("cc-close", { bubbles: true }));
    }
    return true;
  }
  if (target.closest(".cc-power-cancel")) {
    st.confirming = null;
    renderControlCenter(panel, st.view);
    return true;
  }
  if (target.closest(".cc-power-go")) {
    const go = st.confirming;
    st.confirming = null;
    if (go) void runPower(go);
    panel.dispatchEvent(new CustomEvent("cc-close", { bubbles: true }));
    return true;
  }
  return false;
}

function wire(panel: HTMLElement, st: PanelState): void {
  panel.addEventListener("click", (e) => {
    const target = e.target as Element;
    if (onPower(panel, st, target)) return;
    const toggle = target.closest<HTMLElement>("[data-tile]");
    if (toggle) return onToggle(panel, st, toggle.dataset.tile as ToggleKey | "dnd");
    const out = target.closest<HTMLElement>(".cc-output");
    const id = out?.dataset.id;
    if (out && id) {
      setPending(st, "output", id);
      invoke("audio_set_output", { id }).catch(() => {
        st.pending.delete("output");
        if (panel.isConnected) renderControlCenter(panel, st.view);
      });
      renderControlCenter(panel, st.view);
    }
  });
  panel.addEventListener("input", (e) => {
    const input = e.target as HTMLInputElement;
    const slider = input.closest<HTMLElement>("[data-slider]");
    if (!slider) return;
    const value = Math.min(100, Math.max(0, Math.round(Number(input.value))));
    st.lastInput.set(slider.dataset.slider ?? "", performance.now());
    fill(slider, value);
    if (slider.dataset.slider === "brightness") void invoke("cc_set_brightness", { value });
    else void invoke("set_volume", { level: value / 100 });
  });
}

// ---- DOM helpers -----------------------------------------------------------------------

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  return e;
}

/** Child of `parent` matching `sel`, created by `make` when missing. */
function ensure<T extends HTMLElement>(parent: HTMLElement, sel: string, make: () => T): T {
  return parent.querySelector<T>(`:scope > ${sel}`) ?? parent.appendChild(make());
}

function setText(e: HTMLElement, text: string): void {
  if (e.textContent !== text) e.textContent = text;
}

function setIcon(e: HTMLElement, html: string): void {
  if (e.dataset.svg !== html) {
    e.innerHTML = html;
    e.dataset.svg = html;
  }
}

/** Places `children` (nulls skipped) in order inside `parent`, removing the others. */
function place(parent: HTMLElement, children: (HTMLElement | null)[]): void {
  const keep = children.filter((c): c is HTMLElement => c !== null);
  for (const c of [...parent.children]) if (!keep.includes(c as HTMLElement)) c.remove();
  keep.forEach((c, i) => {
    if (parent.children[i] !== c) parent.insertBefore(c, parent.children[i] ?? null);
  });
}

function makeToggle(cls: string, key: string): HTMLButtonElement {
  const b = el("button", cls);
  b.type = "button";
  b.tabIndex = -1;
  b.dataset.tile = key;
  const text = el("span", "cc-text");
  text.append(el("span", "cc-label"), el("span", "cc-sub"));
  b.append(el("span", "cc-glyph"), text);
  return b;
}

/** Updates a tile (or connectivity row): glyph, label, subtitle, pressed state. */
function updateToggle(b: HTMLElement, on: boolean, icon: string, label: string, sub: string | null): void {
  b.setAttribute("aria-pressed", String(on));
  b.classList.toggle("on", on);
  setIcon(b.querySelector<HTMLElement>(".cc-glyph")!, icon);
  setText(b.querySelector<HTMLElement>(".cc-label")!, label);
  const s = b.querySelector<HTMLElement>(".cc-sub")!;
  setText(s, sub ?? "");
  s.hidden = sub === null;
  b.setAttribute("aria-label", sub ? `${label}, ${sub}` : label);
}

function fill(slider: HTMLElement, value: number): void {
  const p = `${value}%`;
  if (slider.style.getPropertyValue("--p") === p) return;
  slider.style.setProperty("--p", p);
  // Unitless 0..1 for the CSS fill, which ends under the knob's centre.
  slider.querySelector("input")?.style.setProperty("--pn", String(value / 100));
}

function makeSlider(key: string, label: string, icon: string): HTMLElement {
  const wrap = el("div", "cc-slider");
  wrap.dataset.slider = key;
  const input = el("input", "");
  input.type = "range";
  input.min = "0";
  input.max = "100";
  input.step = "1";
  input.tabIndex = -1;
  input.setAttribute("aria-label", label);
  const glyph = el("span", "cc-slider-glyph");
  glyph.innerHTML = icon;
  wrap.append(input, glyph);
  return wrap;
}

function updateSlider(st: PanelState, wrap: HTMLElement, value: number): void {
  const input = wrap.querySelector("input")!;
  const recent = performance.now() - (st.lastInput.get(wrap.dataset.slider ?? "") ?? -Infinity) < DRAG_GUARD_MS;
  if (recent || wrap.ownerDocument.activeElement === input) return;
  const v = Math.round(Math.min(100, Math.max(0, value)));
  if (input.value !== String(v)) input.value = String(v);
  fill(wrap, v);
}

function titledModule(cls: string, title: string, slider: HTMLElement): HTMLElement {
  const m = el("div", `cc-module ${cls}`);
  setText(m.appendChild(el("div", "cc-title")), title);
  m.appendChild(slider);
  return m;
}

// ---- Render ----------------------------------------------------------------------------

export function renderControlCenter(panel: HTMLElement, view: ControlView): void {
  let st = states.get(panel);
  if (!st) {
    st = { view, pending: new Map(), lastInput: new Map(), confirming: null };
    states.set(panel, st);
    wire(panel, st);
    panel.setAttribute("role", "dialog");
    panel.setAttribute("aria-label", t.cc.title);
  }
  st.view = view;

  const grid = ensure(panel, ".cc-grid", () => el("div", "cc-grid"));

  // Connectivity module: Wi-Fi, Bluetooth (when present), Micro.
  const connect = ensure(grid, ".cc-connect", () => el("div", "cc-module cc-connect"));
  const row = (key: ToggleKey) =>
    connect.querySelector<HTMLElement>(`:scope > [data-tile="${key}"]`) ?? makeToggle("cc-row", key);

  let wifi: HTMLElement | null = null;
  const wifiOn = toggleValue(st, "wifi");
  if (wifiOn !== undefined) {
    wifi = row("wifi");
    const live = view.network.kind === "wifi" ? view.network.name : null;
    const name = live ?? view.cc?.wifi?.name ?? null;
    updateToggle(wifi, wifiOn, ICONS.wifi, t.cc.wifi, wifiOn ? (name ?? t.cc.notConnected) : t.cc.off);
  }
  let bt: HTMLElement | null = null;
  const btOn = toggleValue(st, "bluetooth");
  if (btOn !== undefined) {
    bt = row("bluetooth");
    updateToggle(bt, btOn, ICONS.bluetooth, t.cc.bluetooth, btOn ? t.cc.on : t.cc.off);
  }
  const mic = row("mic");
  const micOn = toggleValue(st, "mic") ?? true;
  updateToggle(mic, micOn, micOn ? ICONS.mic : ICONS.micOff, t.cc.mic, micOn ? t.cc.on : t.cc.micMuted);
  place(connect, [wifi, bt, mic]);

  // Right column: Ne pas déranger (opens the Windows settings), Mode sombre.
  const side = ensure(grid, ".cc-side", () => el("div", "cc-side"));
  const dnd = ensure(side, '[data-tile="dnd"]', () => makeToggle("cc-module cc-tile", "dnd"));
  updateToggle(dnd, false, ICONS.moon, t.cc.dnd, null);
  const dark = ensure(side, '[data-tile="dark"]', () => makeToggle("cc-module cc-tile", "dark"));
  updateToggle(dark, toggleValue(st, "dark") ?? false, ICONS.dark, t.cc.dark, null);
  place(side, [dnd, dark]);
  place(grid, [connect, side]);

  // Écran: brightness of the monitor under the cursor (hidden without DDC/CI).
  // + the shared monitor's input switch (Réglages › Écran partagé).
  let display: HTMLElement | null = null;
  const brightness = view.cc?.brightness ?? null;
  const shared = view.display ?? null;
  if (brightness !== null || shared) {
    display =
      panel.querySelector<HTMLElement>(":scope > .cc-display") ??
      titledModule("cc-display", t.cc.display, makeSlider("brightness", t.cc.brightness, ICONS.sun));
    const slider = display.querySelector<HTMLElement>("[data-slider]")!;
    slider.hidden = brightness === null;
    if (brightness !== null) updateSlider(st, slider, brightness);
    let screen = display.querySelector<HTMLElement>(':scope > [data-tile="screen"]');
    if (shared) {
      if (!screen) screen = display.appendChild(makeToggle("cc-row", "screen"));
      updateToggle(screen, false, ICONS.screen, `${t.cc.switchTo} ${shared.label}`, `${t.cc.display} ${shared.monitor}`);
    } else screen?.remove();
  }

  // Son: volume + the output list (only when there is a choice).
  const sound =
    panel.querySelector<HTMLElement>(":scope > .cc-sound") ??
    titledModule("cc-sound", t.cc.sound, makeSlider("volume", t.system.volume, ICONS.speaker));
  const volSlider = sound.querySelector<HTMLElement>(":scope > [data-slider]")!;
  updateSlider(st, volSlider, (view.volume?.muted ? 0 : (view.volume?.level ?? 0)) * 100);
  const outputs = view.audio?.outputs ?? [];
  let list: HTMLElement | null = null;
  if (outputs.length >= 2) {
    list = sound.querySelector<HTMLElement>(":scope > .cc-outputs");
    if (!list) {
      list = el("div", "cc-outputs");
      list.setAttribute("role", "radiogroup");
      list.setAttribute("aria-label", t.cc.output);
    }
    const chosen = effective(st, "output", outputs.find((o) => o.default)?.id ?? "");
    const existing = new Map([...list.querySelectorAll<HTMLElement>(":scope > .cc-output")].map((r) => [r.dataset.id ?? "", r]));
    const rows = outputs.map((o) => {
      let r = existing.get(o.id);
      if (!r) {
        r = el("button", "cc-output");
        (r as HTMLButtonElement).type = "button";
        r.tabIndex = -1;
        r.dataset.id = o.id;
        r.setAttribute("role", "radio");
        r.append(el("span", "cc-out-glyph"), el("span", "cc-out-name"), el("span", "cc-out-check"));
        setIcon(r.querySelector<HTMLElement>(".cc-out-glyph")!, ICONS.output);
        setIcon(r.querySelector<HTMLElement>(".cc-out-check")!, ICONS.check);
      }
      setText(r.querySelector<HTMLElement>(".cc-out-name")!, o.name);
      if (r.title !== o.name) r.title = o.name;
      const on = o.id === chosen;
      r.setAttribute("aria-checked", String(on));
      r.classList.toggle("on", on);
      return r;
    });
    place(list, rows);
  }
  place(sound, [sound.querySelector<HTMLElement>(":scope > .cc-title"), volSlider, list]);

  place(panel, [grid, display, sound, renderPower(panel, st)]);
}

function renderPower(panel: HTMLElement, st: PanelState): HTMLElement {
  const box = ensure(panel, ".cc-power", () => {
    const b = el("div", "cc-power");
    b.setAttribute("role", "group");
    b.setAttribute("aria-label", t.power.group);
    return b;
  });
  const row = ensure(box, ".cc-power-row", () => {
    const r = el("div", "cc-power-row");
    for (const id of POWER_IDS) {
      const b = el("button", "cc-power-btn");
      b.type = "button";
      b.tabIndex = -1;
      b.dataset.power = id;
      b.title = powerLabel(id);
      b.setAttribute("aria-label", powerLabel(id));
      setIcon(b, ICONS[id]);
      r.append(b);
    }
    return r;
  });
  const confirm = ensure(box, ".cc-power-confirm", () => {
    const c = el("div", "cc-power-confirm");
    const actions = el("div", "cc-power-actions");
    const cancel = el("button", "cc-power-cancel");
    cancel.type = "button";
    cancel.tabIndex = -1;
    cancel.textContent = t.power.cancel;
    const go = el("button", "cc-power-go");
    go.type = "button";
    go.tabIndex = -1;
    actions.append(cancel, go);
    c.append(el("p", "cc-power-text"), actions);
    return c;
  });
  const id = st.confirming;
  row.hidden = id !== null;
  confirm.hidden = id === null;
  if (id !== null) {
    setText(confirm.querySelector<HTMLElement>(".cc-power-text")!, confirmText(id) ?? "");
    setText(confirm.querySelector<HTMLElement>(".cc-power-go")!, powerLabel(id));
  }
  return box;
}

// Game Mode system pill (`.island.pill`): while a game runs on another monitor and this
// dock is hidden, the dock window shows only this compact line (CPU, GPU, RAM, ping,
// WSL). A click opens a small glass panel above it (same pattern as the tray overflow
// panel): details plus the WSL actions, « Arrêter WSL… » asking for confirmation first.
// The dock window never takes focus; Rust keeps it on-screen clipped to these islands and
// samples `sysmon://stats` only while a pill is shown.
import { invoke } from "../shared/ipc";
import { t } from "../shared/i18n";
import type { GameState, SysStats } from "../shared/types";

/** How long the result of a WSL action stays visible. */
const RESULT_MS = 4000;

const SEP = " · ";

/** One decimal, French comma: 11.24 → « 11,2 ». */
function dec1(x: number): string {
  return x.toFixed(1).replace(".", ",");
}

function gib(mib: number): number {
  return mib / 1024;
}

function gpuText(s: SysStats): string | null {
  const parts: string[] = [];
  if (s.gpu !== null) parts.push(`${Math.round(s.gpu)} %`);
  if (s.gpuTemp !== null) parts.push(`${Math.round(s.gpuTemp)}°`);
  return parts.length > 0 ? parts.join(" ") : null;
}

function ramText(s: SysStats): string {
  return `${dec1(gib(s.ram.used))}/${Math.round(gib(s.ram.total))} Go`;
}

/** « CPU 23 % · GPU 41 % 62° · RAM 11,2/32 Go · 18 ms » (+ « · WSL 3,1 Go »); what is not
 *  measured is left out. `null`: no sample yet. */
export function formatStats(s: SysStats | null): string {
  if (!s) return t.pill.waiting;
  const parts = [`CPU ${Math.round(s.cpu)} %`];
  const gpu = gpuText(s);
  if (gpu) parts.push(`GPU ${gpu}`);
  parts.push(`RAM ${ramText(s)}`);
  if (s.pingMs !== null) parts.push(`${Math.round(s.pingMs)} ms`);
  if (s.wslRamMb !== null) parts.push(`WSL ${dec1(gib(s.wslRamMb))} Go`);
  return parts.join(SEP);
}

export interface PillInput {
  game: GameState | null;
  /** This dock's monitor (`?monitor=`). */
  monitorId: string;
  /** `settings.gameMode.monitorPill`. */
  monitorPill: boolean;
  /** The dock is revealed: it takes over, no pill. */
  revealed: boolean;
}

/** Mirror of `dock_host::show_pill` in Rust. */
export function showPill(p: PillInput): boolean {
  return !!p.game?.active && p.monitorPill && !p.revealed && p.game.monitorId !== p.monitorId;
}

// ---- Rendering ---------------------------------------------------------------------

interface PillState {
  open: boolean;
  confirming: boolean;
  busy: boolean;
  result: string;
  resultTimer: number;
  stats: SysStats | null;
  onOutside: ((e: Event) => void) | null;
}

const states = new WeakMap<HTMLElement, PillState>();

function stateOf(el: HTMLElement): PillState {
  let s = states.get(el);
  if (!s) {
    s = { open: false, confirming: false, busy: false, result: "", resultTimer: 0, stats: null, onOutside: null };
    states.set(el, s);
    wire(el, s);
  }
  return s;
}

/** Tells the dock page the pill changed size (panel opened/closed) so it re-lays out. */
function notifyLayout(el: HTMLElement): void {
  el.dispatchEvent(new CustomEvent("pilllayout", { bubbles: true }));
}

function setOpen(el: HTMLElement, s: PillState, open: boolean): void {
  if (open === s.open) return;
  s.open = open;
  const doc = el.ownerDocument;
  if (open) {
    s.onOutside = (e: Event) => {
      if (!el.contains(e.target as Node)) setOpen(el, s, false);
    };
    doc.addEventListener("pointerdown", s.onOutside, true);
  } else {
    if (s.onOutside) doc.removeEventListener("pointerdown", s.onOutside, true);
    s.onOutside = null;
    s.confirming = false;
  }
  renderPill(el, s.stats);
  notifyLayout(el);
}

/** Closes the detail panel. */
export function closePillPanel(el: HTMLElement): void {
  const s = states.get(el);
  if (s) setOpen(el, s, false);
}

/** The pill is being removed (the dock reveals, the game ends): drops its document
 *  listener and timer without re-rendering or notifying. */
export function disposePill(el: HTMLElement): void {
  const s = states.get(el);
  if (!s) return;
  if (s.onOutside) el.ownerDocument.removeEventListener("pointerdown", s.onOutside, true);
  clearTimeout(s.resultTimer);
  states.delete(el);
}

function run(el: HTMLElement, s: PillState, cmd: "wsl_free" | "wsl_shutdown"): void {
  if (s.busy) return;
  s.busy = true;
  s.confirming = false;
  renderPill(el, s.stats);
  invoke(cmd)
    .then((msg) => showResult(el, s, msg))
    .catch((e: unknown) => showResult(el, s, String(e)))
    .finally(() => {
      s.busy = false;
      if (states.get(el) === s) renderPill(el, s.stats);
    });
}

function showResult(el: HTMLElement, s: PillState, msg: string): void {
  if (states.get(el) !== s) return; // the pill went away meanwhile
  clearTimeout(s.resultTimer);
  s.result = msg;
  s.resultTimer = window.setTimeout(() => {
    s.result = "";
    renderPill(el, s.stats);
  }, RESULT_MS);
  renderPill(el, s.stats);
}

function wire(el: HTMLElement, s: PillState): void {
  el.addEventListener("click", (e) => {
    const target = e.target as Element;
    if (target.closest(".pill-line")) setOpen(el, s, !s.open);
    else if (target.closest(".pill-free")) run(el, s, "wsl_free");
    else if (target.closest(".pill-stop")) {
      s.confirming = true;
      renderPill(el, s.stats);
    } else if (target.closest(".pill-cancel")) {
      s.confirming = false;
      renderPill(el, s.stats);
    } else if (target.closest(".pill-shutdown")) run(el, s, "wsl_shutdown");
  });
  // Never the dock's context menu over the pill.
  el.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    e.stopPropagation();
  });
}

function make<K extends keyof HTMLElementTagNameMap>(tag: K, cls: string, text = ""): HTMLElementTagNameMap[K] {
  const n = document.createElement(tag);
  n.className = cls;
  if (text) n.textContent = text;
  if (n instanceof HTMLButtonElement) {
    n.type = "button";
    n.tabIndex = -1;
  }
  return n;
}

function buildPanel(): HTMLElement {
  const panel = make("div", "island pill-panel");
  panel.setAttribute("role", "group");
  panel.setAttribute("aria-label", t.pill.details);
  const rows = make("dl", "pill-rows");
  for (const key of ["cpu", "gpu", "ram", "ping", "wsl"] as const) {
    const dt = make("dt", "", t.pill[key]);
    const dd = make("dd", `pill-val pill-${key}`);
    rows.append(dt, dd);
  }
  const actions = make("div", "pill-actions");
  actions.append(make("button", "pill-btn pill-free", t.pill.wslFree), make("button", "pill-btn pill-stop", t.pill.wslStop));
  const confirm = make("div", "pill-confirm");
  confirm.hidden = true;
  const buttons = make("div", "pill-actions");
  buttons.append(make("button", "pill-btn pill-cancel", t.pill.cancel), make("button", "pill-btn danger pill-shutdown", t.pill.stop));
  confirm.append(make("p", "pill-confirm-text", t.pill.wslConfirm), buttons);
  const result = make("p", "pill-result");
  result.setAttribute("role", "status");
  panel.append(rows, actions, confirm, result);
  return panel;
}

function setText(n: Element | null, text: string): void {
  if (n && n.textContent !== text) n.textContent = text;
}

function fillPanel(panel: HTMLElement, s: PillState): void {
  const st = s.stats;
  const none = t.pill.none;
  setText(panel.querySelector(".pill-cpu"), st ? `${Math.round(st.cpu)} %` : none);
  setText(panel.querySelector(".pill-gpu"), (st && gpuText(st)) ?? none);
  setText(panel.querySelector(".pill-ram"), st ? ramText(st) : none);
  setText(panel.querySelector(".pill-ping"), st?.pingMs != null ? `${Math.round(st.pingMs)} ms` : none);
  setText(panel.querySelector(".pill-wsl"), st?.wslRamMb != null ? `${dec1(gib(st.wslRamMb))} Go` : none);
  const actions = panel.querySelector<HTMLElement>(":scope > .pill-actions");
  const confirm = panel.querySelector<HTMLElement>(".pill-confirm");
  if (actions) actions.hidden = s.confirming;
  if (confirm) confirm.hidden = !s.confirming;
  for (const b of panel.querySelectorAll<HTMLButtonElement>("button")) b.disabled = s.busy;
  setText(panel.querySelector(".pill-result"), s.result);
}

/** Renders the pill line into `el` (`.island.pill`) and, when open, its panel. Nodes are
 *  kept across renders (stats arrive every second; a click must not land on a node that
 *  was just replaced). */
export function renderPill(el: HTMLElement, stats: SysStats | null): void {
  const s = stateOf(el);
  s.stats = stats;
  let line = el.querySelector<HTMLButtonElement>(":scope > .pill-line");
  if (!line) {
    line = make("button", "pill-line");
    line.setAttribute("aria-haspopup", "true");
    el.prepend(line);
  }
  line.setAttribute("aria-expanded", String(s.open));
  setText(line, formatStats(stats));
  let panel = el.querySelector<HTMLElement>(":scope > .pill-panel");
  if (s.open) {
    if (!panel) {
      panel = buildPanel();
      el.appendChild(panel);
    }
    fillPanel(panel, s);
  } else {
    panel?.remove();
  }
}

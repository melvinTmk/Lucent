// Hotkey capture field of the "Recherche" and "Raccourcis" tabs. Mirrors the rules of Rust
// `hotkey::parse`: at least one of Ctrl/Alt/Win, key among A-Z, 0-9, Space, F1-F24, arrows
// (Left/Up/Right/Down) or one OEM character (², ù…); formatted as Ctrl+Alt+Shift+Win+Key.
import { invoke } from "../shared/ipc";
import { t } from "../shared/i18n";
import type { HotkeyId, HotkeyStatuses } from "../shared/types";

export type Capture =
  | { kind: "wait" } // a lone modifier: keep listening
  | { kind: "cancel" }
  | { kind: "invalid" }
  | { kind: "ok"; value: string };

const ARROWS: Record<string, string> = { ArrowLeft: "Left", ArrowUp: "Up", ArrowRight: "Right", ArrowDown: "Down" };

/** Letters and digits follow the layout (`e.key`: RegisterHotKey takes a layout-dependent VK,
 * so AZERTY's "A" key, code KeyQ, must give A). Space, arrows and F-keys are layout
 * independent; digits fall back to the code when `e.key` is a symbol (AZERTY top row: "&" is
 * Digit1). Any other single printable character is an OEM key kept as typed (², ù): Rust
 * resolves it on the active layout. Dead keys (`e.key` "Dead") are rejected. */
function keyName(e: KeyboardEvent): string | null {
  if (/^[a-zA-Z0-9]$/.test(e.key)) return e.key.toUpperCase();
  if (e.code === "Space") return "Space";
  const arrow = ARROWS[e.code] ?? ARROWS[e.key];
  if (arrow) return arrow;
  let m = /^F([0-9]{1,2})$/.exec(e.code);
  if (m && Number(m[1]) >= 1 && Number(m[1]) <= 24) return `F${Number(m[1])}`;
  m = /^Digit([0-9])$/.exec(e.code);
  if (m) return m[1]!;
  // One printable character, never "+" (the separator).
  if ([...e.key].length === 1 && e.key.trim() !== "" && e.key !== "+") return e.key;
  return null;
}

export function captureFromEvent(e: KeyboardEvent): Capture {
  if (e.key === "Escape") return { kind: "cancel" };
  if (["Control", "Alt", "Shift", "Meta", "AltGraph"].includes(e.key)) return { kind: "wait" };
  const key = keyName(e);
  if (!key || !(e.ctrlKey || e.altKey || e.metaKey)) return { kind: "invalid" };
  const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Win"];
  return { kind: "ok", value: [...mods.filter(Boolean), key].join("+") };
}

/** Shows/hides the "taken by another app" message under the field (by field id). */
export function setHotkeyTaken(id: string, taken: boolean, message: string = t.settings.search.hotkeyTaken): void {
  const el = document.getElementById(`${id}-taken`);
  if (el) el.textContent = taken ? message : "";
}

/** Combos of `ids` that failed to register (another app owns them), in `ids` order. */
export function takenCombos(statuses: HotkeyStatuses, ids: HotkeyId[]): string[] {
  return ids.flatMap((id) => {
    const st = statuses[id];
    return st && !st.ok ? [st.combo] : [];
  });
}

export interface HotkeyFieldOptions {
  /** Retour arrière / Suppr (no modifier) clears the field: "" disables the shortcut. */
  allowEmpty?: boolean;
  /** The combination must hold Alt (the app switcher closes on the Alt release). */
  requireAlt?: boolean;
  /** Message shown when `requireAlt` rejects a combination. */
  requireAltMessage?: string;
  /** Factory value: a ↺ button puts it back when the field differs. */
  defaultValue?: string;
  /** Name of another Lucent shortcut already using `combo`, or null. */
  conflict?: (combo: string) => string | null;
}

const KEY_LABELS: Record<string, string> = {
  Shift: "Maj", Win: "⊞ Win", Left: "←", Right: "→", Up: "↑", Down: "↓", Space: "Espace",
};

/** "Ctrl+Alt+Left" → keycaps ["Ctrl", "Alt", "←"]. */
export function keycaps(combo: string): string[] {
  return combo ? combo.split("+").map((k) => KEY_LABELS[k] ?? k) : [];
}

function renderCaps(box: HTMLElement, combo: string, placeholder: string): void {
  box.replaceChildren();
  const caps = keycaps(combo);
  if (caps.length === 0) {
    const ph = document.createElement("span");
    ph.className = "hk-placeholder";
    ph.textContent = placeholder;
    box.append(ph);
    return;
  }
  for (const c of caps) {
    const k = document.createElement("kbd");
    k.textContent = c;
    box.append(k);
  }
}

/** Held modifiers while recording ("Ctrl+Alt" before the main key arrives). */
function heldModifiers(e: KeyboardEvent): string {
  return [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Win"].filter(Boolean).join("+");
}

const sameCombo = (a: string, b: string) => a.trim().toLowerCase() === b.trim().toLowerCase();

/**
 * Shortcut field: the combination drawn as keycaps; click it (or focus it) and press the new
 * combination. Esc cancels, Retour arrière disables (when allowed). While it records, every
 * global hotkey is released (`hotkeys_pause`) so a combination Lucent already owns reaches
 * the page instead of firing.
 */
export function hotkeyField(
  id: string,
  value: string,
  onChange: (v: string) => void,
  opts: HotkeyFieldOptions = {},
): HTMLElement {
  const s = t.settings.shortcuts;
  const wrap = document.createElement("div");
  wrap.className = "hotkey";
  const row = document.createElement("div");
  row.className = "hk-row";
  const caps = document.createElement("div");
  caps.className = "hk-caps";
  // The real focus target (keyboard, screen readers, tests); drawn over by the keycaps.
  const input = document.createElement("input");
  input.type = "text";
  input.id = id;
  input.readOnly = true;
  input.value = value;
  input.className = "hk-input";
  input.setAttribute("aria-label", s.recordHint);
  const reset = document.createElement("button");
  reset.type = "button";
  reset.className = "hk-btn hk-reset";
  reset.textContent = "↺";
  reset.title = s.resetDefault;
  const clear = document.createElement("button");
  clear.type = "button";
  clear.className = "hk-btn hk-clear";
  clear.textContent = "×";
  clear.title = s.disable;
  const err = document.createElement("span");
  err.className = "hotkey-error";
  err.setAttribute("role", "alert");
  const taken = document.createElement("span");
  taken.className = "hotkey-error";
  taken.id = `${id}-taken`;
  taken.setAttribute("role", "alert");
  let current = value;

  const refresh = (live?: string) => {
    const recording = wrap.classList.contains("recording");
    renderCaps(caps, live ?? current, recording ? s.recording : s.disabled);
    reset.hidden = opts.defaultValue === undefined || sameCombo(current, opts.defaultValue);
    clear.hidden = !opts.allowEmpty || current === "";
  };
  const commit = (v: string) => {
    current = v;
    input.value = v;
    err.textContent = "";
    onChange(v);
    refresh();
  };

  caps.addEventListener("mousedown", (e) => {
    e.preventDefault();
    input.focus();
  });
  input.addEventListener("focus", () => {
    wrap.classList.add("recording");
    err.textContent = "";
    refresh("");
    void invoke("hotkeys_pause", { paused: true }).catch(() => {});
  });
  input.addEventListener("blur", () => {
    wrap.classList.remove("recording");
    input.value = current;
    refresh();
    void invoke("hotkeys_pause", { paused: false }).catch(() => {});
  });
  reset.addEventListener("click", () => {
    if (opts.defaultValue === undefined) return;
    const other = opts.conflict?.(opts.defaultValue);
    if (other) {
      err.textContent = `${s.usedBy} « ${other} »`;
      return;
    }
    commit(opts.defaultValue);
  });
  clear.addEventListener("click", () => commit(""));
  input.addEventListener("keyup", (e) => {
    if (wrap.classList.contains("recording")) refresh(heldModifiers(e));
  });
  input.addEventListener("keydown", (e) => {
    if (e.key === "Tab") return; // keep keyboard navigation
    e.preventDefault();
    const bare = !(e.ctrlKey || e.altKey || e.metaKey || e.shiftKey);
    if (opts.allowEmpty && bare && (e.key === "Backspace" || e.key === "Delete")) {
      commit("");
      input.blur();
      return;
    }
    const c = captureFromEvent(e);
    if (c.kind === "wait") {
      refresh(heldModifiers(e));
      return;
    }
    if (c.kind === "cancel") {
      input.value = current;
      err.textContent = "";
      input.blur();
    } else if (c.kind === "invalid") {
      err.textContent = t.settings.search.hotkeyInvalid;
    } else if (opts.requireAlt && !c.value.split("+").includes("Alt")) {
      err.textContent = opts.requireAltMessage ?? t.settings.search.hotkeyInvalid;
    } else {
      const other = sameCombo(c.value, current) ? null : opts.conflict?.(c.value);
      if (other) {
        err.textContent = `${s.usedBy} « ${other} »`;
        refresh("");
        return;
      }
      commit(c.value);
      input.blur();
    }
  });
  row.append(caps, input, reset, clear);
  wrap.append(row, err, taken);
  refresh();
  return wrap;
}

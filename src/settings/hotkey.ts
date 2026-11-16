// Hotkey capture field of the "Recherche" and "Raccourcis" tabs. Mirrors the rules of Rust
// `hotkey::parse`: at least one of Ctrl/Alt/Win, key among A-Z, 0-9, Space, F1-F24, arrows
// (Left/Up/Right/Down) or one OEM character (², ù…); formatted as Ctrl+Alt+Shift+Win+Key.
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
}

/** Read-only input: focus it and press a combination. */
export function hotkeyField(
  id: string,
  value: string,
  onChange: (v: string) => void,
  opts: HotkeyFieldOptions = {},
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "hotkey";
  const input = document.createElement("input");
  input.type = "text";
  input.id = id;
  input.readOnly = true;
  input.value = value;
  const err = document.createElement("span");
  err.className = "hotkey-error";
  err.setAttribute("role", "alert");
  const taken = document.createElement("span");
  taken.className = "hotkey-error";
  taken.id = `${id}-taken`;
  taken.setAttribute("role", "alert");
  let current = value;
  input.addEventListener("keydown", (e) => {
    if (e.key === "Tab") return; // keep keyboard navigation
    e.preventDefault();
    const bare = !(e.ctrlKey || e.altKey || e.metaKey || e.shiftKey);
    if (opts.allowEmpty && bare && (e.key === "Backspace" || e.key === "Delete")) {
      current = "";
      input.value = "";
      err.textContent = "";
      onChange("");
      return;
    }
    const c = captureFromEvent(e);
    if (c.kind === "wait") return;
    if (c.kind === "cancel") {
      input.value = current;
      err.textContent = "";
      input.blur();
    } else if (c.kind === "invalid") {
      err.textContent = t.settings.search.hotkeyInvalid;
    } else if (opts.requireAlt && !c.value.split("+").includes("Alt")) {
      err.textContent = opts.requireAltMessage ?? t.settings.search.hotkeyInvalid;
    } else {
      current = c.value;
      input.value = c.value;
      err.textContent = "";
      onChange(c.value);
    }
  });
  wrap.append(input, err, taken);
  return wrap;
}

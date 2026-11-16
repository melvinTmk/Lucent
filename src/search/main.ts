// Search window page. Rust (search_window.rs) sizes/places the window, draws the native
// glass and shows it; this page holds the input and the results list, queries on every
// keystroke, drives the keyboard, reports its content height and asks Rust to hide.
import { invoke, on, onWindow } from "../shared/ipc";
import { t } from "../shared/i18n";
import type { SearchGroup, Settings } from "../shared/types";
import { layout, moveSelection, renderResults, revealSelected, type Dir, type Layout } from "./render";
import { TEXT_COLOR_VARS, textColorVars } from "./textColor";
import { createQuickLook } from "./quicklook";
import { createConfirmGate, isDestructive } from "../shared/power";

const app = document.getElementById("app") as HTMLElement;
const panel = document.createElement("div");
panel.id = "panel";
panel.className = "island";
panel.setAttribute("role", "search");
panel.setAttribute("aria-label", t.search.label);

const input = document.createElement("input");
input.id = "query";
input.type = "text";
input.spellcheck = false;
input.autocomplete = "off";
input.placeholder = t.search.placeholder;
input.setAttribute("aria-label", t.search.label);
input.setAttribute("role", "combobox");
input.setAttribute("aria-controls", "results");
input.setAttribute("aria-expanded", "false");

const results = document.createElement("div");
results.id = "results";
results.setAttribute("role", "listbox");
results.setAttribute("aria-label", t.search.results);

const preview = document.createElement("div");
preview.id = "preview";
preview.hidden = true;
preview.setAttribute("aria-label", t.search.quickLook.preview);

panel.append(input, results, preview);
app.append(panel);

// ---------- results state ----------

let view: Layout = layout([]);
/** Query the shown results answer (may lag behind the input while a response is pending). */
let viewQuery = "";
let selected = 0;
/** Sequence numbers: a response older than the last applied one is dropped. */
let sent = 0;
let applied = 0;
/** Quick Look (Maj+Espace): the preview replaces the list while open. */
let quickLookEnabled = true;
const quickLook = createQuickLook(preview, (open) => {
  panel.classList.toggle("quicklook", open);
  if (ready) reportHeight();
});
/** True once `search://shown` is listened to: no height may be reported before. */
let ready = false;

function render(reveal: boolean): void {
  renderResults(results, viewQuery, view, selected);
  const has = view.hits.length > 0;
  input.setAttribute("aria-expanded", String(has));
  if (has) input.setAttribute("aria-activedescendant", `opt-${selected}`);
  else input.removeAttribute("aria-activedescendant");
  if (reveal) revealSelected(results, selected);
  if (ready) reportHeight();
}

/** Enter pressed while the shown results lag behind the input: opens the first row of the
 *  response to query `seq` once it is applied. A newer query (keystroke, Échap) cancels it. */
let pendingOpen: { seq: number; shift: boolean; ctrl: boolean } | null = null;
/** An open is in flight (until `search_hide` resolves): further opens are ignored. */
let opening = false;

function query(): void {
  pendingOpen = null;
  quickLook.close();
  const seq = ++sent;
  const q = input.value;
  invoke("search_query", { q })
    .then((groups: SearchGroup[]) => {
      if (seq <= applied) return;
      applied = seq;
      view = layout(Array.isArray(groups) ? groups : []);
      viewQuery = q;
      selected = 0;
      results.scrollTop = 0;
      render(false);
      if (pendingOpen?.seq === seq) {
        const { shift, ctrl } = pendingOpen;
        pendingOpen = null;
        void openSelected(shift, ctrl);
      }
    })
    .catch((e: unknown) => {
      if (pendingOpen?.seq === seq) pendingOpen = null;
      console.error("lucent: search_query failed", e);
    });
}

/** Enter: never acts on results of an older query (search_open launches apps, kills
 *  processes): when a query is in flight, the open waits for the matching response. */
function requestOpen(shift: boolean, ctrl: boolean): void {
  if (opening) return;
  if (sent === applied && viewQuery !== input.value) query(); // input changed without an event
  if (sent > applied) {
    pendingOpen = { seq: sent, shift, ctrl };
    return;
  }
  void openSelected(shift, ctrl);
}

/** Restart / shut down / sign out rows: the first Enter arms the row, a second one within
 *  CONFIRM_MS acts. The row's subtitle asks for it meanwhile. */
const CONFIRM_MS = 4000;
const gate = createConfirmGate(CONFIRM_MS);
let confirmTimer: ReturnType<typeof setTimeout> | undefined;

function needsConfirm(id: string): boolean {
  return id.startsWith("cmd:power:") && isDestructive(id.slice("cmd:power:".length));
}

function armConfirm(hit: Layout["hits"][number]): void {
  const original = hit.entry.subtitle;
  hit.entry.subtitle = t.power.confirmAgain;
  render(false);
  clearTimeout(confirmTimer);
  confirmTimer = setTimeout(() => {
    gate.reset();
    if (hit.entry.subtitle === t.power.confirmAgain) {
      hit.entry.subtitle = original;
      render(false);
    }
  }, CONFIRM_MS);
}

async function openSelected(shift: boolean, ctrl: boolean): Promise<void> {
  const hit = view.hits[selected];
  if (!hit || opening) return;
  if (needsConfirm(hit.entry.id)) {
    if (!gate.press(hit.entry.id)) {
      armConfirm(hit);
      return;
    }
    clearTimeout(confirmTimer);
  }
  opening = true;
  try {
    // Rust's opener never hides the window: hide once it has run (clipboard/emoji copy too).
    // `q` is the query the shown rows answer: Rust recomputes dynamic results from it.
    await invoke("search_open", { id: hit.entry.id, q: viewQuery, shift, ctrl }).catch((e: unknown) =>
      console.error("lucent: search_open failed", e),
    );
    await invoke("search_hide");
  } finally {
    opening = false;
  }
}

const ARROWS: Record<string, Dir> = { ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right" };

input.addEventListener("input", query);

input.addEventListener("keydown", (e) => {
  if (e.isComposing) return;
  const dir = ARROWS[e.key];
  if (e.key === " " && e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey) {
    e.preventDefault(); // Space alone types a space; Maj+Espace toggles the preview
    if (!e.repeat && (quickLookEnabled || quickLook.isOpen())) void quickLook.toggle(view.hits[selected]?.entry);
  } else if (e.key === "Escape" && quickLook.isOpen()) {
    e.preventDefault();
    quickLook.close();
  } else if (dir) {
    const next = moveSelection(view, selected, dir);
    if (next === null) return; // ←/→ outside the emoji grid: move the caret
    e.preventDefault();
    selected = next;
    render(true);
    void quickLook.refresh(view.hits[selected]?.entry);
  } else if (e.key === "Enter") {
    e.preventDefault();
    if (!e.repeat) requestOpen(e.shiftKey, e.ctrlKey);
  } else if (e.key === "Escape") {
    e.preventDefault();
    // Échap clears a non-empty input first; on an empty one it hides.
    if (input.value !== "") {
      input.value = "";
      query();
    } else {
      void invoke("search_hide");
    }
  }
});

function optionIndex(target: EventTarget | null): number | null {
  const opt = (target as Element | null)?.closest?.<HTMLElement>("[data-index]");
  return opt ? Number(opt.dataset.index) : null;
}

// Clicks keep the focus in the input (the keyboard stays live).
results.addEventListener("mousedown", (e) => e.preventDefault());
results.addEventListener("click", (e) => {
  const idx = optionIndex(e.target);
  if (idx === null) return;
  selected = idx;
  render(false);
  void openSelected(e.shiftKey, e.ctrlKey);
});
// Hover selects, but only on a real pointer move (not when rows scroll under a still cursor).
let lastPointer = "";
results.addEventListener("mousemove", (e) => {
  const at = `${e.screenX},${e.screenY}`;
  if (at === lastPointer) return;
  lastPointer = at;
  const idx = optionIndex(e.target);
  if (idx === null || idx === selected) return;
  selected = idx;
  render(false);
});

let lastHeight = -1;
function reportHeight(): void {
  // Layout height: unlike getBoundingClientRect it ignores the opening scale transform.
  const height = panel.offsetHeight;
  if (height === lastHeight) return;
  lastHeight = height;
  void invoke("search_resize", { height });
}

function applySettings(s: Settings | null): void {
  if (!s) return;
  quickLookEnabled = s.quickLook;
  if (!quickLookEnabled) quickLook.close();
  document.body.toggleAttribute("data-reduce-motion", s.animations.reduceMotion);
  const mode = s.dock.glass;
  const dark = mode === "dark" || (mode === "auto" && window.matchMedia?.("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.tone = dark ? "dark" : "light";
  const vars = textColorVars(s.search.textColor);
  for (const k of TEXT_COLOR_VARS) {
    if (vars) document.documentElement.style.setProperty(k, vars[k]);
    else document.documentElement.style.removeProperty(k);
  }
}

/** Rust recomputed server-side rows (a `valo` fetch finished): re-run the shown query,
 *  unless an open is pending or running (a new query would cancel it). */
function refresh(): void {
  if (pendingOpen || opening) return;
  query();
}

function shown(): void {
  // Replay the opening animation on every show.
  panel.classList.remove("opening");
  void panel.offsetWidth;
  panel.classList.add("opening");
  input.focus();
  input.select();
  // History may have changed since the last show (recent group).
  query();
}

// Listen first: Rust reveals a fresh window on its first `search_resize`, then emits
// `search://shown`, so no height may be reported (observer included) before this resolves.
void Promise.all([
  onWindow("search://shown", shown),
  onWindow("search://refresh", refresh),
  on("settings://changed", applySettings),
]).then(async () => {
  ready = true;
  applySettings(await invoke("get_settings").catch(() => null));
  input.focus();
  reportHeight();
  query();
  if (typeof ResizeObserver !== "undefined") {
    new ResizeObserver(reportHeight).observe(panel);
  }
});

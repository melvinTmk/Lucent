// Settings window: macOS System Settings-like sidebar with one pane per tab. Every control updates
// the form, which is mapped onto the last known Settings and saved (debounced 150 ms); the
// dock follows through `settings://changed`.
import { invoke, on } from "../shared/ipc";
import { t } from "../shared/i18n";
import type { DdcMonitor, DetectedGame, HotCornerAction, HotkeyId, HotkeyStatuses, MonitorInfo, Settings } from "../shared/types";
import { hotkeyField, setHotkeyTaken, takenCombos } from "./hotkey";
import {
  CORNER_ACTIONS,
  addExclude,
  addInclude,
  createSaver,
  form_to_settings,
  isValidRiotId,
  removeExclude,
  removeInclude,
  settings_to_form,
  type FormState,
} from "./form";

export type Tab = "dock" | "animations" | "search" | "shortcuts" | "game" | "valorant" | "general";
export const TABS: Tab[] = ["dock", "animations", "search", "shortcuts", "game", "valorant", "general"];

let base: Settings;
let form: FormState;
let monitors: MonitorInfo[] = [];
let detectedGames: DetectedGame[] | null = null;
let detectedLoading = false;
let tab: Tab = "dock";
let hotkeyOk = true;
let hotkeyStatuses: HotkeyStatuses = {};

const saver = createSaver(
  () => form_to_settings(form, base),
  (s) => invoke("save_settings", { settings: s }),
);
const root = document.getElementById("app") as HTMLElement;

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  cls?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function change(patch: Partial<FormState>): void {
  form = { ...form, ...patch };
  saver.touch();
}

function row(label: string, control: HTMLElement, id?: string): HTMLElement {
  const r = el("div", "row");
  const l = el("label", "label", label);
  if (id) l.htmlFor = id;
  r.append(l, control);
  return r;
}

function slider(
  id: string, min: number, max: number, step: number, value: number,
  fmt: (v: number) => string, set: (v: number) => void, disabled = false,
): HTMLElement {
  const wrap = el("div", "slider");
  const input = el("input");
  input.type = "range"; input.id = id;
  input.min = String(min); input.max = String(max); input.step = String(step);
  input.value = String(value); input.disabled = disabled;
  const out = el("span", "value", fmt(value));
  input.addEventListener("input", () => {
    const v = Number(input.value);
    out.textContent = fmt(v);
    set(v);
  });
  wrap.append(input, out);
  return wrap;
}

function toggle(id: string, value: boolean, set: (v: boolean) => void): HTMLElement {
  const input = el("input", "switch");
  input.type = "checkbox"; input.id = id; input.checked = value;
  input.setAttribute("role", "switch");
  input.addEventListener("change", () => set(input.checked));
  return input;
}

function segmented<V extends string>(
  options: [V, string][], value: V, set: (v: V) => void,
): HTMLElement {
  const g = el("div", "segmented");
  g.setAttribute("role", "radiogroup");
  for (const [v, label] of options) {
    const b = el("button", v === value ? "seg on" : "seg", label);
    b.type = "button";
    b.setAttribute("role", "radio");
    b.setAttribute("aria-checked", String(v === value));
    b.addEventListener("click", () => {
      g.querySelectorAll(".seg").forEach((x) => {
        x.classList.remove("on");
        x.setAttribute("aria-checked", "false");
      });
      b.classList.add("on");
      b.setAttribute("aria-checked", "true");
      set(v);
    });
    g.append(b);
  }
  return g;
}

function monitorLabel(m: MonitorInfo, i: number): string {
  return `${t.settings.screenName} ${i + 1}${m.primary ? ` (${t.settings.primaryTag})` : ""}`;
}

function screensControl(): HTMLElement {
  const wrap = el("div", "screens");
  const mode = form.monitors === "all" ? "all" : form.monitors === "primary" ? "primary" : "list";
  const list = el("div", "monlist");
  const fill = (): void => {
    list.replaceChildren();
    if (typeof form.monitors === "string") return;
    const chosen = new Set(form.monitors.list);
    monitors.forEach((m, i) => {
      const lab = el("label", "check");
      const cb = el("input");
      cb.type = "checkbox"; cb.checked = chosen.has(m.id);
      cb.addEventListener("change", () => {
        if (cb.checked) chosen.add(m.id); else chosen.delete(m.id);
        change({ monitors: { list: [...chosen] } });
      });
      lab.append(cb, el("span", undefined, monitorLabel(m, i)));
      list.append(lab);
    });
  };
  wrap.append(
    segmented<"all" | "primary" | "list">(
      [["all", t.settings.screensAll], ["primary", t.settings.screensPrimary], ["list", t.settings.screensList]],
      mode,
      (v) => {
        if (v === "list") {
          const ids = typeof form.monitors === "string"
            ? monitors.filter((m) => form.monitors === "all" || m.primary).map((m) => m.id)
            : form.monitors.list;
          change({ monitors: { list: ids } });
        } else change({ monitors: v });
        fill();
      },
    ),
    list,
  );
  fill();
  return wrap;
}

function paneDock(): HTMLElement[] {
  const s = t.settings;
  return [
    row(s.iconSize, slider("iconSize", 32, 96, 1, form.iconSize, (v) => `${v} px`, (v) => change({ iconSize: v })), "iconSize"),
    row(s.magnification, toggle("magn", form.magnification, (v) => { change({ magnification: v }); render(); }), "magn"),
    row(s.magnificationScale, slider("mscale", 1, 2.5, 0.1, form.magnificationScale, (v) => `×${v.toFixed(1)}`, (v) => change({ magnificationScale: v }), !form.magnification), "mscale"),
    row(s.position, segmented([["bottom", s.positions.bottom], ["left", s.positions.left], ["right", s.positions.right]], form.position, (v) => change({ position: v }))),
    row(s.alwaysVisible, toggle("always", form.alwaysVisible, (v) => { change({ alwaysVisible: v }); render(); }), "always"),
    hint(s.alwaysVisibleHint),
    // Reveal / hide delays mean nothing while the Dock never hides.
    row(s.revealDelay, slider("reveal", 0, 1000, 10, form.revealDelayMs, (v) => `${v} ${s.ms}`, (v) => change({ revealDelayMs: v }), form.alwaysVisible), "reveal"),
    row(s.hideDelay, slider("hide", 0, 2000, 10, form.hideDelayMs, (v) => `${v} ${s.ms}`, (v) => change({ hideDelayMs: v }), form.alwaysVisible), "hide"),
    row(s.screens, screensControl()),
    row(s.glass, segmented([["auto", s.glassModes.auto], ["light", s.glassModes.light], ["dark", s.glassModes.dark]], form.glass, (v) => change({ glass: v }))),
    row(s.clock, toggle("clock", form.showClock, (v) => change({ showClock: v })), "clock"),
    row(s.showMedia, toggle("media", form.showMedia, (v) => change({ showMedia: v })), "media"),
  ];
}

function paneAnimations(): HTMLElement[] {
  const s = t.settings;
  return [
    row(s.speed, slider("speed", 0.5, 2, 0.1, form.speed, (v) => `×${v.toFixed(1)}`, (v) => change({ speed: v })), "speed"),
    row(s.style, segmented([["spring", s.styles.spring], ["sober", s.styles.sober]], form.style, (v) => change({ style: v }))),
    row(s.cardDuration, slider("card", 1, 10, 1, form.cardDurationSec, (v) => `${v} ${s.sec}`, (v) => change({ cardDurationSec: v })), "card"),
    row(s.reduceMotion, toggle("reduce", form.reduceMotion, (v) => change({ reduceMotion: v })), "reduce"),
  ];
}

function textInput(id: string, value: string, placeholder: string, set: (v: string) => void): HTMLElement {
  const input = el("input", "text");
  input.type = "text"; input.id = id; input.value = value; input.placeholder = placeholder;
  input.spellcheck = false;
  input.addEventListener("input", () => set(input.value));
  return input;
}

function colorControl(id: string, value: string, set: (v: string) => void): HTMLElement {
  const s = t.settings.search;
  const wrap = el("div", "colors");
  wrap.setAttribute("role", "radiogroup");
  wrap.setAttribute("aria-label", s.textColor);
  const presets: [string, string, string][] = [
    ["", s.textColorAuto, "auto"], ["#ffffff", s.textColorWhite, "white"], ["#16161a", s.textColorBlack, "black"],
  ];
  const picker = el("input", "color-picker");
  picker.type = "color"; picker.id = id; picker.title = s.textColorCustom;
  picker.value = value || "#ffffff";
  const buttons: HTMLButtonElement[] = [];
  const mark = (v: string): void => {
    presets.forEach(([pv], i) => {
      buttons[i].setAttribute("aria-checked", String(pv === v));
    });
  };
  presets.forEach(([pv, label, kind]) => {
    const b = el("button", `swatch swatch-${kind}`);
    b.type = "button"; b.title = label; b.setAttribute("role", "radio");
    b.setAttribute("aria-label", label);
    if (pv) b.style.background = pv;
    b.addEventListener("click", () => {
      mark(pv);
      picker.value = pv || "#ffffff";
      set(pv);
    });
    buttons.push(b);
    wrap.append(b);
  });
  picker.addEventListener("input", () => {
    const v = picker.value.toLowerCase();
    mark(v);
    set(v);
  });
  wrap.append(picker);
  mark(value);
  return wrap;
}

function paneSearch(): HTMLElement[] {
  const s = t.settings.search;
  const roots = el("textarea", "text");
  roots.id = "roots"; roots.rows = 3; roots.spellcheck = false;
  roots.value = form.projectRoots; roots.placeholder = s.projectRootsPlaceholder;
  roots.addEventListener("input", () => change({ projectRoots: roots.value }));
  const hotkey = hotkeyField("hotkey", form.searchHotkey, (v) => change({ searchHotkey: v }));
  queueMicrotask(() => setHotkeyTaken("hotkey", !hotkeyOk));
  return [
    row(s.hotkey, hotkey, "hotkey"),
    row(s.textColor, colorControl("textcolor", form.textColor, (v) => change({ textColor: v })), "textcolor"),
    row(s.projectRoots, roots, "roots"),
    row(s.wslDistro, textInput("wsl", form.wslDistro, s.wslDistroPlaceholder, (v) => change({ wslDistro: v })), "wsl"),
    row(s.clipboardHistory, toggle("clip", form.clipboardHistory, (v) => change({ clipboardHistory: v })), "clip"),
    row(s.webUrl, textInput("weburl", form.webUrl, "https://…/search?q=%s", (v) => change({ webUrl: v })), "weburl"),
  ];
}

/** Raccourcis tab: editable field id -> registry ids whose status shows under it. */
const SHORTCUT_STATUS: [string, HotkeyId[]][] = [
  ["sc-switcher", ["switcher", "switcherBack"]],
  ["sc-overview", ["overview"]],
  ["sc-mic", ["micMute"]],
  ["sc-display", ["displaySwitch"]],
  ["sc-snapping", ["snapLeft", "snapRight", "snapMax", "snapCenter", "snapThird1", "snapThird2", "snapThird3", "movePrevMonitor", "moveNextMonitor"]],
];

/** « Déjà utilisé par une autre app » under each field whose combo(s) failed to register. */
function applyHotkeyStatuses(): void {
  const msg = t.settings.shortcuts.taken;
  for (const [id, ids] of SHORTCUT_STATUS) {
    const taken = takenCombos(hotkeyStatuses, ids);
    // Several combos behind one control (switcher + Maj, snapping): name the taken ones.
    const many = ids.length > 1;
    setHotkeyTaken(id, taken.length > 0, many ? `${msg} : ${taken.join(", ")}` : msg);
  }
}

function hint(text: string): HTMLElement {
  return el("div", "hint", text);
}

function cornerSelect(id: string, value: HotCornerAction, set: (v: HotCornerAction) => void): HTMLElement {
  const sel = el("select");
  sel.id = id;
  const labels = t.settings.shortcuts.actions;
  for (const a of CORNER_ACTIONS) sel.append(new Option(labels[a], a));
  sel.value = value;
  sel.addEventListener("change", () => set(sel.value as HotCornerAction));
  return sel;
}

function numberInput(id: string, value: number, set: (v: number) => void): HTMLInputElement {
  const input = el("input", "text");
  input.type = "number"; input.id = id; input.min = "0"; input.max = "255"; input.step = "1";
  input.value = String(value);
  input.style.width = "5em";
  input.addEventListener("input", () => set(Number(input.value)));
  return input;
}

/** Écran partagé: monitor list (filled once the DDC worker answered), the two codes. */
function displaySection(): HTMLElement[] {
  const s = t.settings.shortcuts;
  let monitors: DdcMonitor[] = [];
  const sel = el("select");
  sel.id = "ds-monitor";
  const fill = () => {
    const keep = form.displayMonitor;
    sel.replaceChildren(new Option(s.displayNone, ""));
    for (const m of monitors) {
      const extra = m.input === null ? "" : ` — ${s.displayCurrent} ${m.input}`;
      sel.append(new Option(`${m.name}${extra}`, m.name));
    }
    if (keep && !monitors.some((m) => m.name === keep)) sel.append(new Option(keep, keep));
    sel.value = keep;
  };
  fill();
  sel.addEventListener("change", () => change({ displayMonitor: sel.value }));
  void invoke("list_ddc_monitors").then((list) => {
    monitors = list ?? [];
    fill();
  });

  const here = numberInput("ds-here", form.displayHere, (v) => change({ displayHere: v }));
  const useCurrent = el("button", "btn", s.displayUseCurrent);
  useCurrent.type = "button";
  useCurrent.addEventListener("click", () => {
    const m = monitors.find((x) => x.name === sel.value);
    if (m?.input == null) return;
    here.value = String(m.input);
    change({ displayHere: m.input });
  });
  const hereWrap = el("div", "test-row");
  hereWrap.append(here, useCurrent);

  const test = el("button", "btn", s.displayTest);
  test.type = "button";
  test.id = "ds-test";
  test.addEventListener("click", () => {
    saver.flush();
    void invoke("display_switch", {});
  });
  return [
    el("h2", "section", s.displayTitle),
    hint(s.displayHint),
    row(s.displayMonitor, sel, "ds-monitor"),
    row(s.displayHere, hereWrap, "ds-here"),
    row(s.displayOther, numberInput("ds-other", form.displayOther, (v) => change({ displayOther: v })), "ds-other"),
    row(s.displayLabel, textInput("ds-label", form.displayOtherLabel, "Autre PC", (v) => change({ displayOtherLabel: v })), "ds-label"),
    row(s.displayDetach, toggle("ds-detach", form.displayDetach, (v) => change({ displayDetach: v })), "ds-detach"),
    hint(s.displayDetachHint),
    row("", test),
  ];
}

function paneShortcuts(): HTMLElement[] {
  const s = t.settings.shortcuts;
  const field = (id: string, value: string, set: (v: string) => void, help: string, requireAlt = false): HTMLElement => {
    const wrap = el("div", "field-col");
    wrap.append(hotkeyField(id, value, set, { allowEmpty: true, requireAlt, requireAltMessage: s.switcherNeedsAlt }), hint(help));
    return wrap;
  };
  const snapping = el("div", "field-col");
  const snapTaken = el("span", "hotkey-error");
  snapTaken.id = "sc-snapping-taken";
  snapTaken.setAttribute("role", "alert");
  snapping.append(toggle("sc-snapping", form.snapping, (v) => change({ snapping: v })), hint(s.snappingHint), snapTaken);
  const corners: [keyof typeof s.corners, keyof FormState][] = [
    ["topLeft", "cornerTopLeft"], ["topRight", "cornerTopRight"],
    ["bottomLeft", "cornerBottomLeft"], ["bottomRight", "cornerBottomRight"],
  ];
  queueMicrotask(applyHotkeyStatuses);
  return [
    row(s.switcher, field("sc-switcher", form.switcherHotkey, (v) => change({ switcherHotkey: v }), `${s.switcherHint} ${s.clearHint}.`, true), "sc-switcher"),
    row(s.overview, field("sc-overview", form.overviewHotkey, (v) => change({ overviewHotkey: v }), s.clearHint), "sc-overview"),
    row(s.micMute, field("sc-mic", form.micMuteHotkey, (v) => change({ micMuteHotkey: v }), s.clearHint), "sc-mic"),
    row(s.displaySwitch, field("sc-display", form.displaySwitchHotkey, (v) => change({ displaySwitchHotkey: v }), s.clearHint), "sc-display"),
    row(s.snapping, snapping, "sc-snapping"),
    ...displaySection(),
    el("h2", "section", s.cornersTitle),
    ...corners.map(([c, k]) =>
      row(s.corners[c], cornerSelect(`corner-${c}`, form[k] as HotCornerAction, (v) => change({ [k]: v } as Partial<FormState>)), `corner-${c}`),
    ),
    row(s.controlCenter, toggle("sc-cc", form.controlCenter, (v) => change({ controlCenter: v })), "sc-cc"),
    row(s.quickLook, toggle("sc-quicklook", form.quickLook, (v) => change({ quickLook: v })), "sc-quicklook"),
  ];
}

function quitButton(): HTMLElement {
  const b = el("button", "quit", t.settings.quit);
  b.type = "button";
  b.id = "quit";
  b.addEventListener("click", () => {
    saver.flush(); // a pending change is saved before the app goes away
    void invoke("quit_app");
  });
  return b;
}

function loadDetectedGames(onDone: () => void): void {
  if (detectedGames !== null || detectedLoading) return;
  detectedLoading = true;
  invoke("list_detected_games")
    .then((games) => {
      detectedGames = games;
      detectedLoading = false;
      onDone();
    })
    .catch((e) => {
      console.error("list_detected_games failed", e);
      detectedGames = [];
      detectedLoading = false;
      onDone();
    });
}

function detectedControl(): HTMLElement {
  const box = el("div", "list-box");
  box.id = "detected-games-list";
  if (detectedGames === null) {
    box.append(el("div", "list-empty", t.settings.game.loading));
    loadDetectedGames(() => render());
    return box;
  }
  if (detectedGames.length === 0) {
    box.append(el("div", "list-empty", t.settings.game.noDetected));
    return box;
  }
  for (const g of detectedGames) {
    const r = el("div", "game-row");
    const name = el("span", "game-name", g.name);
    name.title = `${g.name} (${g.exe})`;
    const sourceLabel = (t.settings.game.sources as Record<string, string>)[g.source] ?? g.source;
    const badge = el("span", `badge badge-${g.source}`, sourceLabel);
    r.append(name, badge);
    box.append(r);
  }
  return box;
}

function exeListControl(
  items: string[],
  onRemove: (path: string) => void,
  onAdd: (path: string) => void,
  listId: string,
): HTMLElement {
  const wrap = el("div", "exe-control");
  const box = el("div", "list-box");
  box.id = listId;
  if (items.length === 0) {
    box.append(el("div", "list-empty", t.settings.game.noDetected));
  } else {
    for (const exe of items) {
      const r = el("div", "exe-row");
      const path = el("span", "exe-path", exe);
      path.title = exe;
      const del = el("button", "btn-remove", "×");
      del.type = "button";
      del.setAttribute("aria-label", `${t.settings.game.remove} ${exe}`);
      del.addEventListener("click", () => onRemove(exe));
      r.append(path, del);
      box.append(r);
    }
  }
  const addRow = el("div", "add-row");
  const input = el("input", "text add-input");
  input.type = "text";
  input.placeholder = t.settings.game.addPlaceholder;
  input.spellcheck = false;
  const addBtn = el("button", "btn", t.settings.game.add);
  addBtn.type = "button";
  const doAdd = () => {
    const v = input.value.trim();
    if (v) {
      onAdd(v);
      input.value = "";
    }
  };
  addBtn.addEventListener("click", doAdd);
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") doAdd();
  });
  addRow.append(input, addBtn);
  wrap.append(box, addRow);
  return wrap;
}

function dndRow(): HTMLElement {
  const r = el("div", "row");
  const l = el("label", "label", t.settings.game.dndNotice);
  const b = el("button", "btn", t.settings.game.dndButton);
  b.type = "button";
  b.id = "dnd-settings-btn";
  b.addEventListener("click", () => {
    void invoke("open_external", { url: "ms-settings:notifications" });
  });
  r.append(l, b);
  return r;
}

function paneGame(): HTMLElement[] {
  const s = t.settings.game;
  return [
    row(s.enabled, toggle("game-enabled", form.gameModeEnabled, (v) => change({ gameModeEnabled: v })), "game-enabled"),
    row(s.detectedTitle, detectedControl()),
    row(
      s.includeTitle,
      exeListControl(
        form.gameModeInclude,
        (exe) => {
          form = removeInclude(form, exe);
          saver.touch();
          render();
        },
        (exe) => {
          form = addInclude(form, exe);
          saver.touch();
          render();
        },
        "include-list",
      ),
    ),
    row(
      s.excludeTitle,
      exeListControl(
        form.gameModeExclude,
        (exe) => {
          form = removeExclude(form, exe);
          saver.touch();
          render();
        },
        (exe) => {
          form = addExclude(form, exe);
          saver.touch();
          render();
        },
        "exclude-list",
      ),
    ),
    row(s.powerPlan, toggle("game-power", form.gameModePowerPlan, (v) => change({ gameModePowerPlan: v })), "game-power"),
    row(s.monitorPill, toggle("game-pill", form.gameModeMonitorPill, (v) => change({ gameModeMonitorPill: v })), "game-pill"),
    row(s.pingTarget, textInput("game-ping", form.gameModePingTarget, "1.1.1.1", (v) => change({ gameModePingTarget: v })), "game-ping"),
    dndRow(),
  ];
}

function paneValorant(): HTMLElement[] {
  const s = t.settings.valorant;

  const riotWrap = el("div", "field-col");
  const riotInput = el("input", "text");
  riotInput.type = "text";
  riotInput.id = "valo-riot-id";
  riotInput.value = form.valorantRiotId;
  riotInput.placeholder = s.riotIdPlaceholder;
  riotInput.spellcheck = false;
  const hint = el("div", "hint-error", s.riotIdHint);
  const updateHint = (val: string) => {
    const show = val.trim().length > 0 && !isValidRiotId(val);
    hint.style.display = show ? "block" : "none";
  };
  updateHint(form.valorantRiotId);
  riotInput.addEventListener("input", () => {
    updateHint(riotInput.value);
    change({ valorantRiotId: riotInput.value });
  });
  riotWrap.append(riotInput, hint);

  const keyWrap = el("div", "field-col");
  const keyInput = el("input", "text");
  keyInput.type = "password";
  keyInput.id = "valo-api-key";
  keyInput.value = form.valorantApiKey;
  keyInput.placeholder = s.apiKeyPlaceholder;
  keyInput.autocomplete = "off";
  keyInput.spellcheck = false;
  keyInput.addEventListener("input", () => change({ valorantApiKey: keyInput.value }));
  const link = el("a", "link", s.apiKeyLink);
  link.href = "#";
  link.addEventListener("click", (e) => {
    e.preventDefault();
    void invoke("open_external", { url: "https://docs.henrikdev.xyz/authentication-and-authorization" });
  });
  keyWrap.append(keyInput, link);

  const testWrap = el("div", "test-row");
  const testBtn = el("button", "btn", s.test);
  testBtn.type = "button";
  testBtn.id = "valo-test-btn";
  const status = el("span", "test-status");
  status.id = "valo-test-status";
  testBtn.addEventListener("click", async () => {
    saver.flush();
    testBtn.disabled = true;
    status.textContent = s.testing;
    try {
      const res = await invoke("valo_test");
      status.textContent = res;
    } catch (err) {
      status.textContent = String(err);
    } finally {
      testBtn.disabled = false;
    }
  });
  testWrap.append(testBtn, status);

  return [
    row(s.riotId, riotWrap, "valo-riot-id"),
    row(
      s.region,
      segmented<"eu" | "na" | "ap" | "kr" | "latam" | "br">(
        [
          ["eu", "EU"],
          ["na", "NA"],
          ["ap", "AP"],
          ["kr", "KR"],
          ["latam", "LATAM"],
          ["br", "BR"],
        ],
        form.valorantRegion as any,
        (v) => change({ valorantRegion: v }),
      ),
    ),
    row(s.apiKey, keyWrap, "valo-api-key"),
    row(s.postMatch, toggle("valo-postmatch", form.valorantPostMatch, (v) => change({ valorantPostMatch: v })), "valo-postmatch"),
    row(s.pinCard, toggle("valo-pincard", form.valorantPinCard, (v) => change({ valorantPinCard: v })), "valo-pincard"),
    row(s.test, testWrap),
  ];
}

function paneGeneral(): HTMLElement[] {
  const s = t.settings;
  const lang = el("select");
  lang.id = "lang";
  lang.append(new Option(s.languages.fr, "fr"));
  lang.value = "fr";
  lang.addEventListener("change", () => change({ language: lang.value }));
  return [
    row(s.launchAtStartup, toggle("startup", form.launchAtStartup, (v) => change({ launchAtStartup: v })), "startup"),
    row(s.hideTaskbar, toggle("taskbar", form.hideWindowsTaskbar, (v) => change({ hideWindowsTaskbar: v })), "taskbar"),
    row(s.language, lang, "lang"),
    row(s.quitHint, quitButton()),
  ];
}

function render(): void {
  const side = el("nav", "sidebar");
  side.setAttribute("aria-label", t.settings.title);
  for (const id of TABS) {
    const b = el("button", id === tab ? "tab on" : "tab", t.settings.tabs[id]);
    b.type = "button";
    b.addEventListener("click", () => { tab = id; render(); });
    side.append(b);
  }
  const main = el("main", "content");
  main.append(el("h1", undefined, t.settings.tabs[tab]));
  const card = el("div", "card");
  const panes: Record<Tab, () => HTMLElement[]> = {
    dock: paneDock,
    animations: paneAnimations,
    search: paneSearch,
    shortcuts: paneShortcuts,
    game: paneGame,
    valorant: paneValorant,
    general: paneGeneral,
  };
  card.append(...panes[tab]());
  main.append(card);
  const prev = root.querySelector<HTMLElement>(".content");
  const scroll = prev ? prev.scrollTop : 0;
  root.replaceChildren(side, main);
  main.scrollTop = scroll;
}

function same(a: FormState, b: FormState): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

async function boot(): Promise<void> {
  base = await invoke("get_settings");
  form = settings_to_form(base);
  try {
    monitors = await invoke("list_monitors");
  } catch (e) {
    console.error("list_monitors failed", e);
  }
  const urlParams = new URLSearchParams(window.location.search);
  const initialTab = urlParams.get("tab") as Tab | null;
  if (initialTab && TABS.includes(initialTab)) {
    tab = initialTab;
  }
  await on("settings://tab", (targetTab) => {
    if (TABS.includes(targetTab as Tab)) {
      tab = targetTab as Tab;
      render();
    }
  });
  await on("hotkey://status", (st) => {
    hotkeyOk = st.ok;
    setHotkeyTaken("hotkey", !st.ok);
  });
  await on("hotkeys://status", (st) => {
    hotkeyStatuses = st;
    applyHotkeyStatuses();
  });
  try {
    hotkeyOk = (await invoke("get_hotkey_status")).ok;
  } catch (e) {
    console.error("get_hotkey_status failed", e);
  }
  try {
    hotkeyStatuses = (await invoke("get_hotkey_statuses")) ?? {};
  } catch (e) {
    console.error("get_hotkey_statuses failed", e);
  }
  document.title = t.settings.title;
  render();
  await on("settings://changed", (s) => {
    base = s;
    const incoming = settings_to_form(s);
    // Our own saves echo back: only re-render for genuine external changes.
    if (!saver.pending && !same(incoming, form)) {
      form = incoming;
      render();
    }
  });
}

// Closing within the debounce window must not lose the last change.
window.addEventListener("pagehide", () => saver.flush());
window.addEventListener("beforeunload", () => saver.flush());

void boot();

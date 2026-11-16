// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { form_to_settings, settings_to_form } from "./form";
import { captureFromEvent, hotkeyField, takenCombos } from "./hotkey";
import { baseSettings } from "./fixtures";
import type { HotCornerAction, Settings } from "../shared/types";

const { invoke, listen, handlers, statuses } = vi.hoisted(() => {
  const handlers: Record<string, (payload: unknown) => void> = {};
  const statuses: { value: unknown } = { value: {} };
  return {
    handlers,
    statuses,
    invoke: vi.fn(async (cmd: string): Promise<unknown> => {
      if (cmd === "get_settings") return (await import("./fixtures")).baseSettings;
      if (cmd === "list_monitors") return [];
      if (cmd === "get_hotkey_status") return { combo: "Alt+Space", ok: true };
      if (cmd === "get_hotkey_statuses") return statuses.value;
      return undefined;
    }),
    listen: vi.fn(async (name: string, cb: (e: { payload: unknown }) => void) => {
      handlers[name] = (payload) => cb({ payload });
      return () => {};
    }),
  };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "settings" }),
}));

const key = (init: KeyboardEventInit) => new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });

describe("shortcuts form mapping", () => {
  it("exposes every M6 setting as a flat field", () => {
    const f = settings_to_form(baseSettings);
    expect(f.switcherHotkey).toBe("Alt+²");
    expect(f.overviewHotkey).toBe("Ctrl+Alt+Space");
    expect(f.micMuteHotkey).toBe("Ctrl+Alt+M");
    expect(f.snapping).toBe(true);
    expect([f.cornerTopLeft, f.cornerTopRight, f.cornerBottomLeft, f.cornerBottomRight]).toEqual(["overview", "none", "none", "none"]);
    expect(f.controlCenter).toBe(true);
    expect(f.quickLook).toBe(true);
  });

  it("writes edited fields back and trims combos", () => {
    const f = {
      ...settings_to_form(baseSettings),
      switcherHotkey: " Alt+Q ", overviewHotkey: "", micMuteHotkey: "Ctrl+Alt+K", snapping: false,
      cornerTopRight: "lock" as HotCornerAction, cornerBottomLeft: "desktop" as HotCornerAction,
      controlCenter: false, quickLook: false,
    };
    const out = form_to_settings(f, baseSettings);
    expect(out.shortcuts).toEqual({ switcher: "Alt+Q", overview: "", snapping: false, micMute: "Ctrl+Alt+K", displaySwitch: "Ctrl+Alt+O", snap: baseSettings.shortcuts.snap });
    expect(out.hotCorners).toEqual({ topLeft: "overview", topRight: "lock", bottomLeft: "desktop", bottomRight: "none" });
    expect(out.controlCenter).toBe(false);
    expect(out.quickLook).toBe(false);
  });

  it("turns an unknown corner action into none (Rust does the same)", () => {
    const f = { ...settings_to_form(baseSettings), cornerTopLeft: "explode" as HotCornerAction };
    expect(form_to_settings(f, baseSettings).hotCorners.topLeft).toBe("none");
  });

  it("round-trips the defaults untouched", () => {
    expect(form_to_settings(settings_to_form(baseSettings), baseSettings)).toEqual(baseSettings);
  });
});

describe("capture of M6 keys", () => {
  it("captures arrows, Espace and OEM keys like ²", () => {
    expect(captureFromEvent(key({ key: "ArrowLeft", code: "ArrowLeft", ctrlKey: true, altKey: true }))).toEqual({ kind: "ok", value: "Ctrl+Alt+Left" });
    expect(captureFromEvent(key({ key: "ArrowRight", code: "ArrowRight", ctrlKey: true, altKey: true, shiftKey: true }))).toEqual({ kind: "ok", value: "Ctrl+Alt+Shift+Right" });
    expect(captureFromEvent(key({ key: "ArrowUp", code: "ArrowUp", metaKey: true }))).toEqual({ kind: "ok", value: "Win+Up" });
    expect(captureFromEvent(key({ key: "ArrowDown", code: "ArrowDown", altKey: true }))).toEqual({ kind: "ok", value: "Alt+Down" });
    expect(captureFromEvent(key({ key: " ", code: "Space", ctrlKey: true, altKey: true }))).toEqual({ kind: "ok", value: "Ctrl+Alt+Space" });
    expect(captureFromEvent(key({ key: "²", code: "Backquote", altKey: true }))).toEqual({ kind: "ok", value: "Alt+²" });
    expect(captureFromEvent(key({ key: "ù", code: "Quote", ctrlKey: true }))).toEqual({ kind: "ok", value: "Ctrl+ù" });
    // AZERTY top row still gives the digit, not the symbol.
    expect(captureFromEvent(key({ key: "&", code: "Digit1", altKey: true }))).toEqual({ kind: "ok", value: "Alt+1" });
    expect(captureFromEvent(key({ key: "Dead", code: "BracketLeft", altKey: true }))).toEqual({ kind: "invalid" });
    expect(captureFromEvent(key({ key: "²", code: "Backquote" }))).toEqual({ kind: "invalid" });
  });

  it("Retour arrière clears a field that may be empty (disabled shortcut) only", () => {
    const clear = vi.fn();
    const input = hotkeyField("a", "Ctrl+Alt+M", clear, { allowEmpty: true }).querySelector("input")!;
    input.dispatchEvent(key({ key: "Backspace", code: "Backspace" }));
    expect(input.value).toBe("");
    expect(clear).toHaveBeenCalledWith("");
    const keep = vi.fn();
    const input2 = hotkeyField("b", "Alt+Space", keep).querySelector("input")!;
    input2.dispatchEvent(key({ key: "Backspace", code: "Backspace" }));
    expect(input2.value).toBe("Alt+Space");
    expect(keep).not.toHaveBeenCalled();
  });

  it("the switcher field rejects a combination without Alt", () => {
    const set = vi.fn();
    const wrap = hotkeyField("sw", "Alt+²", set, { allowEmpty: true, requireAlt: true, requireAltMessage: "Le changeur d'apps doit utiliser Alt" });
    const input = wrap.querySelector("input")!;
    input.dispatchEvent(key({ key: "q", code: "KeyQ", ctrlKey: true }));
    expect(set).not.toHaveBeenCalled();
    expect(input.value).toBe("Alt+²");
    expect(wrap.querySelector(".hotkey-error")!.textContent).toBe("Le changeur d'apps doit utiliser Alt");
    input.dispatchEvent(key({ key: "q", code: "KeyQ", altKey: true }));
    expect(set).toHaveBeenCalledWith("Alt+Q");
  });
});

describe("keycap field", () => {
  it("draws the combination as keycaps and records on focus (hotkeys paused meanwhile)", () => {
    invoke.mockClear();
    const set = vi.fn();
    const wrap = hotkeyField("k", "Ctrl+Alt+Left", set, { defaultValue: "Ctrl+Alt+Left" });
    document.body.append(wrap);
    expect([...wrap.querySelectorAll("kbd")].map((k) => k.textContent)).toEqual(["Ctrl", "Alt", "←"]);
    const input = wrap.querySelector("input")!;
    input.focus();
    expect(wrap.classList.contains("recording")).toBe(true);
    expect(invoke).toHaveBeenCalledWith("hotkeys_pause", { paused: true });
    input.dispatchEvent(key({ key: "Control", code: "ControlLeft", ctrlKey: true }));
    expect([...wrap.querySelectorAll("kbd")].map((k) => k.textContent)).toEqual(["Ctrl"]);
    input.dispatchEvent(key({ key: "h", code: "KeyH", ctrlKey: true, shiftKey: true }));
    expect(set).toHaveBeenCalledWith("Ctrl+Shift+H");
    expect(wrap.classList.contains("recording")).toBe(false);
    expect(invoke).toHaveBeenCalledWith("hotkeys_pause", { paused: false });
    expect([...wrap.querySelectorAll("kbd")].map((k) => k.textContent)).toEqual(["Ctrl", "Maj", "H"]);
    // Changed from the default: ↺ shows and puts it back.
    const reset = wrap.querySelector<HTMLButtonElement>(".hk-reset")!;
    expect(reset.hidden).toBe(false);
    reset.click();
    expect(set).toHaveBeenLastCalledWith("Ctrl+Alt+Left");
    expect(reset.hidden).toBe(true);
    wrap.remove();
  });

  it("refuses a combination another Lucent shortcut uses, and × disables", () => {
    const set = vi.fn();
    const wrap = hotkeyField("c", "Ctrl+Alt+M", set, { allowEmpty: true, conflict: (c) => (c === "Alt+Space" ? "Recherche" : null) });
    const input = wrap.querySelector("input")!;
    input.dispatchEvent(key({ key: " ", code: "Space", altKey: true }));
    expect(set).not.toHaveBeenCalled();
    expect(wrap.querySelector(".hotkey-error")!.textContent).toBe("Déjà utilisé par « Recherche »");
    wrap.querySelector<HTMLButtonElement>(".hk-clear")!.click();
    expect(set).toHaveBeenCalledWith("");
    expect(wrap.querySelector(".hk-placeholder")!.textContent).toBe("Désactivé");
  });
});

describe("takenCombos", () => {
  it("lists the combos another app owns, for the given ids only", () => {
    const st = {
      switcher: { combo: "Alt+²", ok: false },
      switcherBack: { combo: "Alt+Shift+²", ok: true },
      snapLeft: { combo: "Ctrl+Alt+Left", ok: false },
      snapThird1: { combo: "Ctrl+Alt+D", ok: false },
    };
    expect(takenCombos(st, ["switcher", "switcherBack"])).toEqual(["Alt+²"]);
    expect(takenCombos(st, ["snapLeft", "snapRight", "snapThird1"])).toEqual(["Ctrl+Alt+Left", "Ctrl+Alt+D"]);
    expect(takenCombos(st, ["overview"])).toEqual([]);
  });
});

describe("Raccourcis tab", () => {
  beforeEach(() => {
    vi.resetModules();
    document.body.innerHTML = '<div id="app"></div>';
    invoke.mockClear();
    statuses.value = { switcher: { combo: "Alt+²", ok: false }, overview: { combo: "Ctrl+Alt+Space", ok: true }, snapThird2: { combo: "Ctrl+Alt+F", ok: false } };
  });
  afterEach(() => vi.useRealTimers());

  async function openTab(): Promise<void> {
    await import("./main");
    await vi.waitFor(() => expect(document.querySelectorAll(".tab").length).toBe(7));
    const tab = Array.from(document.querySelectorAll<HTMLButtonElement>(".tab")).find((b) => b.textContent === "Raccourcis")!;
    expect(tab).toBeDefined();
    tab.click();
    await vi.waitFor(() => expect(document.querySelector("#sc-switcher")).not.toBeNull());
  }

  it("shows every M6 control with its current value", async () => {
    await openTab();
    expect(document.querySelector<HTMLInputElement>("#sc-switcher")!.value).toBe("Alt+²");
    expect(document.querySelector<HTMLInputElement>("#sc-overview")!.value).toBe("Ctrl+Alt+Space");
    expect(document.querySelector<HTMLInputElement>("#sc-mic")!.value).toBe("Ctrl+Alt+M");
    expect(document.querySelector<HTMLInputElement>("#sc-snapping")!.checked).toBe(true);
    expect(document.querySelector<HTMLInputElement>("#sc-cc")!.checked).toBe(true);
    expect(document.querySelector<HTMLInputElement>("#sc-quicklook")!.checked).toBe(true);
    const tl = document.querySelector<HTMLSelectElement>("#corner-topLeft")!;
    expect(tl.value).toBe("overview");
    expect(Array.from(tl.options).map((o) => o.text)).toEqual([
      "Aucun", "Vue des fenêtres", "Bureau", "Verrouiller", "Recherche", "Changeur d'apps",
    ]);
    for (const c of ["topRight", "bottomLeft", "bottomRight"]) {
      expect(document.querySelector<HTMLSelectElement>(`#corner-${c}`)!.value).toBe("none");
    }
  });

  it("shows « Déjà utilisé par une autre app » per id and follows hotkeys://status", async () => {
    await openTab();
    expect(invoke).toHaveBeenCalledWith("get_hotkey_statuses", undefined);
    expect(document.getElementById("sc-switcher-taken")!.textContent).toContain("Déjà utilisé par une autre app");
    expect(document.getElementById("sc-overview-taken")!.textContent).toBe("");
    expect(document.getElementById("sc-snapping-taken")!.textContent).toContain("Ctrl+Alt+F");
    handlers["hotkeys://status"]!({ switcher: { combo: "Alt+²", ok: true }, micMute: { combo: "Ctrl+Alt+M", ok: false } });
    expect(document.getElementById("sc-switcher-taken")!.textContent).toBe("");
    expect(document.getElementById("sc-mic-taken")!.textContent).toContain("Déjà utilisé par une autre app");
    expect(document.getElementById("sc-snapping-taken")!.textContent).toBe("");
  });

  it("lists the nine snapping shortcuts, editable, and blocks a duplicate", async () => {
    await openTab();
    const left = document.querySelector<HTMLInputElement>("#sc-snap-left")!;
    expect(left.value).toBe("Ctrl+Alt+Left");
    expect(document.querySelectorAll('[id^="sc-snap-"]').length).toBeGreaterThanOrEqual(9);
    // Ctrl+Alt+M belongs to « Couper le micro ».
    left.dispatchEvent(key({ key: "m", code: "KeyM", ctrlKey: true, altKey: true }));
    expect(left.closest(".hotkey")!.querySelector(".hotkey-error")!.textContent).toContain("Couper le micro");
    left.dispatchEvent(key({ key: "h", code: "KeyH", metaKey: true, altKey: true }));
    await vi.waitFor(() => expect(invoke.mock.calls.some((c) => c[0] === "save_settings")).toBe(true));
    const saved = (invoke.mock.calls.filter((c) => c[0] === "save_settings").slice(-1)[0] as unknown as [string, { settings: Settings }])[1].settings;
    expect(saved.shortcuts.snap.left).toBe("Alt+Win+H");
  });

  it("saves a hot corner, a toggle and a captured shortcut", async () => {
    await openTab();
    const tr = document.querySelector<HTMLSelectElement>("#corner-topRight")!;
    tr.value = "lock";
    tr.dispatchEvent(new Event("change"));
    const cc = document.querySelector<HTMLInputElement>("#sc-cc")!;
    cc.checked = false;
    cc.dispatchEvent(new Event("change"));
    document.querySelector<HTMLInputElement>("#sc-overview")!.dispatchEvent(key({ key: "ArrowUp", code: "ArrowUp", ctrlKey: true, altKey: true, shiftKey: true }));
    await vi.waitFor(() => expect(invoke.mock.calls.some((c) => c[0] === "save_settings")).toBe(true));
    const saved = (invoke.mock.calls.filter((c) => c[0] === "save_settings").slice(-1)[0] as unknown as [string, { settings: Settings }])[1].settings;
    expect(saved.hotCorners.topRight).toBe("lock");
    expect(saved.controlCenter).toBe(false);
    expect(saved.shortcuts.overview).toBe("Ctrl+Alt+Shift+Up");
  });
});

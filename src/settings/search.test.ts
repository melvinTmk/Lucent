// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { form_to_settings, settings_to_form } from "./form";
import { hotkeyField, setHotkeyTaken } from "./hotkey";
import { baseSettings } from "./fixtures";

function press(input: HTMLElement, init: KeyboardEventInit): void {
  input.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
}

describe("search tab", () => {
  it("search_tab_saves_hotkey_from_keydown", () => {
    let form = settings_to_form(baseSettings);
    const saved = vi.fn();
    const field = hotkeyField("hk", form.searchHotkey, (v) => {
      form = { ...form, searchHotkey: v };
      saved(form_to_settings(form, baseSettings));
    });
    const input = field.querySelector("input")!;
    press(input, { key: "Control", code: "ControlLeft", ctrlKey: true });
    expect(saved).not.toHaveBeenCalled();
    press(input, { key: "K", code: "KeyK", ctrlKey: true, shiftKey: true });
    expect(input.value).toBe("Ctrl+Shift+K");
    expect(saved.mock.calls[0]![0].search.hotkey).toBe("Ctrl+Shift+K");
  });

  it("rejects combos the backend would reject and Escape cancels", () => {
    const onChange = vi.fn();
    const field = hotkeyField("hk", "Alt+Space", onChange);
    const input = field.querySelector("input")!;
    press(input, { key: "A", code: "KeyA", shiftKey: true });
    press(input, { key: "Space", code: "Space" });
    press(input, { key: "Alt", code: "AltLeft", altKey: true });
    press(input, { key: "x", code: "KeyX", altKey: true, shiftKey: false });
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(input.value).toBe("Alt+X");
    press(input, { key: "Escape", code: "Escape" });
    expect(input.value).toBe("Alt+X");
    press(input, { key: "F24", code: "F24", metaKey: true, altKey: true, ctrlKey: true, shiftKey: true });
    expect(input.value).toBe("Ctrl+Alt+Shift+Win+F24");
    press(input, { key: "Escape", code: "Escape" });
    expect(input.value).toBe("Ctrl+Alt+Shift+Win+F24");
  });

  it("uses the layout letter, not the physical key (AZERTY)", () => {
    const onChange = vi.fn();
    const input = hotkeyField("hk", "Alt+Space", onChange).querySelector("input")!;
    press(input, { key: "a", code: "KeyQ", ctrlKey: true });
    expect(input.value).toBe("Ctrl+A");
    press(input, { key: "&", code: "Digit1", altKey: true });
    expect(input.value).toBe("Alt+1");
    press(input, { key: " ", code: "Space", altKey: true });
    expect(input.value).toBe("Alt+Space");
  });

  it("maps project roots one per line, trimmed, empty lines dropped", () => {
    const f = { ...settings_to_form(baseSettings), projectRoots: "  C:\\a \n\n D:\\b\r\n  \n" };
    expect(form_to_settings(f, baseSettings).search.projectRoots).toEqual(["C:\\a", "D:\\b"]);
    expect(settings_to_form({ ...baseSettings, search: { ...baseSettings.search, projectRoots: ["x", "y"] } }).projectRoots).toBe("x\ny");
  });

  it("roundtrips untouched", () => {
    expect(form_to_settings(settings_to_form(baseSettings), baseSettings)).toEqual(baseSettings);
  });

  it("shows the taken message under the field only when registration failed", () => {
    const field = hotkeyField("hk", "Alt+Space", vi.fn());
    document.body.append(field);
    const taken = document.getElementById("hk-taken")!;
    expect(taken.textContent).toBe("");
    setHotkeyTaken("hk", true);
    expect(taken.textContent).toContain("Raccourci indisponible");
    setHotkeyTaken("hk", false);
    expect(taken.textContent).toBe("");
    field.remove();
  });
});

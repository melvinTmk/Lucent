import { describe, expect, it } from "vitest";
import { form_to_settings, settings_to_form } from "./form";
import { baseSettings } from "./fixtures";
import type { Settings } from "../shared/types";

describe("M6 settings round-trip", () => {
  it("keeps shortcuts, hot corners and feature toggles through the form", () => {
    const s: Settings = {
      ...baseSettings,
      shortcuts: { switcher: "", overview: "Ctrl+Alt+Space", snapping: false, micMute: "Ctrl+Alt+M", displaySwitch: "Ctrl+Alt+O" },
      hotCorners: { topLeft: "none", topRight: "lock", bottomLeft: "desktop", bottomRight: "search" },
      controlCenter: false,
      quickLook: false,
    };
    const out = form_to_settings(settings_to_form(s), s);
    expect(out.shortcuts).toEqual(s.shortcuts);
    expect(out.hotCorners).toEqual(s.hotCorners);
    expect(out.controlCenter).toBe(false);
    expect(out.quickLook).toBe(false);
  });
});

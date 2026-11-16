import { describe, expect, it } from "vitest";
import { textColorVars } from "./textColor";
import { form_to_settings, settings_to_form } from "../settings/form";
import { baseSettings } from "../settings/fixtures";

describe("textColorVars", () => {
  it("returns null for auto or invalid colours", () => {
    expect(textColorVars("")).toBeNull();
    expect(textColorVars("red")).toBeNull();
    expect(textColorVars("#12")).toBeNull();
  });
  it("derives the ink tokens from a hex colour", () => {
    expect(textColorVars("#ffaa00")).toEqual({
      "--ink": "#ffaa00",
      "--ink-soft": "rgb(255 170 0 / 0.65)",
      "--tile-ink": "rgb(255 170 0 / 0.65)",
    });
  });
});

describe("textColor form round-trip", () => {
  it.each(["#123456", ""])("keeps %j", (c) => {
    const s = { ...baseSettings, search: { ...baseSettings.search, textColor: c } };
    expect(form_to_settings(settings_to_form(s), s).search.textColor).toBe(c);
  });
});

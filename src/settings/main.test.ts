// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
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
import type { Settings } from "../shared/types";
import { baseSettings } from "./fixtures";

const { invoke, listen } = vi.hoisted(() => ({
  invoke: vi.fn(async (cmd: string): Promise<unknown> => {
    if (cmd === "get_settings") return baseSettings;
    if (cmd === "list_monitors") return [];
    if (cmd === "get_hotkey_status") return { ok: true };
    if (cmd === "list_detected_games") {
      return [
        { name: "Portal 2", exe: "c:\\steam\\portal2.exe", source: "steam" },
        { name: "Valorant", exe: "c:\\riot\\valo.exe", source: "riot" },
      ];
    }
    if (cmd === "valo_test") return "Platine 2 - 45 RR";
    if (cmd === "update_status") {
      return { phase: "idle", current: "0.3.0", version: null, progress: null, error: null, dismissed: false };
    }
    if (cmd === "update_check") {
      return { phase: "available", current: "0.3.0", version: "0.4.0", progress: null, error: null, dismissed: false };
    }
    return undefined;
  }),
  listen: vi.fn(async () => () => {}),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "settings" }),
}));

const base: Settings = baseSettings;
const f = settings_to_form(base);

describe("form_to_settings", () => {
  it("slider writes iconSize", () => expect(form_to_settings({ ...f, iconSize: 64 }, base).dock.iconSize).toBe(64));
  it("keeps unrelated fields from base", () => {
    const out = form_to_settings({ ...f, glass: "dark" }, base);
    expect(out.dock.pinned).toEqual(base.dock.pinned);
    expect(out.schemaVersion).toBe(1);
    expect(out.dock.glass).toBe("dark");
  });
  it("maps showMedia both ways", () => {
    expect(f.showMedia).toBe(true);
    expect(form_to_settings({ ...f, showMedia: false }, base).dock.showMedia).toBe(false);
    expect(settings_to_form({ ...base, dock: { ...base.dock, showMedia: false } }).showMedia).toBe(false);
  });
  it("roundtrips untouched", () => expect(form_to_settings(f, base)).toEqual(base));
  it("converts card seconds to ms and clamps", () => {
    expect(form_to_settings({ ...f, cardDurationSec: 7 }, base).animations.cardDurationMs).toBe(7000);
    expect(form_to_settings({ ...f, iconSize: 500 }, base).dock.iconSize).toBe(96);
  });

  describe("gameMode and valorant mapping", () => {
    it("maps gameMode fields both ways", () => {
      const custom: FormState = {
        ...f,
        gameModeEnabled: false,
        gameModeInclude: ["C:\\Games\\Doom.exe"],
        gameModeExclude: ["C:\\Apps\\Discord.exe"],
        gameModePowerPlan: false,
        gameModeMonitorPill: false,
        gameModePingTarget: "8.8.8.8",
      };
      const mapped = form_to_settings(custom, base);
      expect(mapped.gameMode.enabled).toBe(false);
      expect(mapped.gameMode.include).toEqual(["c:\\games\\doom.exe"]);
      expect(mapped.gameMode.exclude).toEqual(["c:\\apps\\discord.exe"]);
      expect(mapped.gameMode.powerPlan).toBe(false);
      expect(mapped.gameMode.monitorPill).toBe(false);
      expect(mapped.gameMode.pingTarget).toBe("8.8.8.8");

      const back = settings_to_form(mapped);
      expect(back.gameModeEnabled).toBe(false);
      expect(back.gameModeInclude).toEqual(["c:\\games\\doom.exe"]);
      expect(back.gameModeExclude).toEqual(["c:\\apps\\discord.exe"]);
      expect(back.gameModePowerPlan).toBe(false);
      expect(back.gameModeMonitorPill).toBe(false);
      expect(back.gameModePingTarget).toBe("8.8.8.8");
    });

    it("falls back to default ping target 1.1.1.1 when empty", () => {
      const mapped = form_to_settings({ ...f, gameModePingTarget: "   " }, base);
      expect(mapped.gameMode.pingTarget).toBe("1.1.1.1");
    });

    it("maps valorant fields both ways and clamps region", () => {
      const custom: FormState = {
        ...f,
        valorantRiotId: "  Sentinel#EUW  ",
        valorantRegion: "na",
        valorantApiKey: "  RGAPI-secret  ",
        valorantPostMatch: false,
      };
      const mapped = form_to_settings(custom, base);
      expect(mapped.valorant.riotId).toBe("Sentinel#EUW");
      expect(mapped.valorant.region).toBe("na");
      expect(mapped.valorant.apiKey).toBe("RGAPI-secret");
      expect(mapped.valorant.postMatch).toBe(false);

      const back = settings_to_form(mapped);
      expect(back.valorantRiotId).toBe("Sentinel#EUW");
      expect(back.valorantRegion).toBe("na");
      expect(back.valorantApiKey).toBe("RGAPI-secret");
      expect(back.valorantPostMatch).toBe(false);

      const invalidRegion = form_to_settings({ ...f, valorantRegion: "invalid" }, base);
      expect(invalidRegion.valorant.region).toBe("eu");
    });
  });
});

describe("isValidRiotId", () => {
  it("accepts valid Pseudo#TAG formats", () => {
    expect(isValidRiotId("Sentinel#EUW")).toBe(true);
    expect(isValidRiotId("Player # EUW")).toBe(true);
    expect(isValidRiotId("Père Noël#ÉTÉ")).toBe(true);
  });

  it("rejects invalid formats", () => {
    expect(isValidRiotId("NoTag")).toBe(false);
    expect(isValidRiotId("NoName#")).toBe(false);
    expect(isValidRiotId("#NoName")).toBe(false);
    expect(isValidRiotId("   ")).toBe(false);
  });
});

describe("include / exclude management", () => {
  it("addInclude adds lowercased trimmed path and removes from exclude", () => {
    const initial: FormState = {
      ...f,
      gameModeInclude: ["c:\\games\\game1.exe"],
      gameModeExclude: ["c:\\games\\doom.exe"],
    };
    const next = addInclude(initial, "  C:\\Games\\Doom.exe  ");
    expect(next.gameModeInclude).toEqual(["c:\\games\\game1.exe", "c:\\games\\doom.exe"]);
    expect(next.gameModeExclude).toEqual([]);
  });

  it("addInclude deduplicates existing include", () => {
    const initial: FormState = {
      ...f,
      gameModeInclude: ["c:\\games\\doom.exe"],
    };
    const next = addInclude(initial, "c:\\games\\doom.exe");
    expect(next.gameModeInclude).toEqual(["c:\\games\\doom.exe"]);
  });

  it("removeInclude removes the specified path", () => {
    const initial: FormState = {
      ...f,
      gameModeInclude: ["c:\\games\\game1.exe", "c:\\games\\doom.exe"],
    };
    const next = removeInclude(initial, "c:\\games\\doom.exe");
    expect(next.gameModeInclude).toEqual(["c:\\games\\game1.exe"]);
  });

  it("addExclude adds lowercased trimmed path and removes from include", () => {
    const initial: FormState = {
      ...f,
      gameModeInclude: ["c:\\apps\\discord.exe"],
      gameModeExclude: [],
    };
    const next = addExclude(initial, "  C:\\Apps\\Discord.exe  ");
    expect(next.gameModeExclude).toEqual(["c:\\apps\\discord.exe"]);
    expect(next.gameModeInclude).toEqual([]);
  });

  it("removeExclude removes the specified path", () => {
    const initial: FormState = {
      ...f,
      gameModeExclude: ["c:\\apps\\discord.exe"],
    };
    const next = removeExclude(initial, "c:\\apps\\discord.exe");
    expect(next.gameModeExclude).toEqual([]);
  });
});

describe("createSaver", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());
  it("debounces saves to one call per 150ms", () => {
    const save = vi.fn();
    let cur = base;
    const s = createSaver(() => cur, save);
    cur = form_to_settings({ ...f, iconSize: 50 }, base);
    s.touch();
    vi.advanceTimersByTime(100);
    cur = form_to_settings({ ...f, iconSize: 60 }, base);
    s.touch();
    vi.advanceTimersByTime(100);
    expect(save).not.toHaveBeenCalled();
    expect(s.pending).toBe(true);
    vi.advanceTimersByTime(50);
    expect(save).toHaveBeenCalledTimes(1);
    expect(save.mock.calls[0]![0].dock.iconSize).toBe(60);
    expect(s.pending).toBe(false);
  });
  it("flush sends the pending value immediately and cancels the timer", () => {
    const save = vi.fn();
    const s = createSaver(() => form_to_settings({ ...f, iconSize: 70 }, base), save);
    s.flush();
    expect(save).not.toHaveBeenCalled();
    s.touch();
    s.flush();
    expect(save).toHaveBeenCalledTimes(1);
    expect(s.pending).toBe(false);
    vi.advanceTimersByTime(500);
    expect(save).toHaveBeenCalledTimes(1);
  });
  it("builds the payload at fire time from the latest base", () => {
    const save = vi.fn();
    let latest = base;
    const form = { ...f, glass: "dark" as const };
    const s = createSaver(() => form_to_settings(form, latest), save);
    s.touch();
    const pinned = [{ app: { kind: "exe" as const, value: "c:\\new.exe" }, name: "New", isGame: false }];
    latest = { ...base, dock: { ...base.dock, pinned } };
    vi.advanceTimersByTime(150);
    const out = save.mock.calls[0]![0] as Settings;
    expect(out.dock.pinned).toEqual(pinned);
    expect(out.dock.glass).toBe("dark");
  });
});

describe("settings UI and tabs", () => {
  beforeEach(async () => {
    vi.useRealTimers();
    vi.resetModules();
    document.body.innerHTML = '<div id="app"></div>';
  });

  it("registers tabs including Mode Jeu and Valorant", async () => {
    const { TABS } = await import("./main");
    expect(TABS).toContain("game");
    expect(TABS).toContain("valorant");
  });

  it("renders Mode Jeu pane with detected games, include/exclude, and DND button", async () => {
    await import("./main");
    await vi.waitFor(() => expect(document.querySelectorAll(".tab").length).toBe(7));
    const tabs = Array.from(document.querySelectorAll<HTMLButtonElement>(".tab"));
    const gameTab = tabs.find((t) => t.textContent === "Mode Jeu")!;
    expect(gameTab).toBeDefined();
    gameTab.click();

    await vi.waitFor(() => expect(document.querySelector("#game-enabled")).not.toBeNull());
    expect(document.querySelector("#detected-games-list")).not.toBeNull();
    expect(document.querySelector("#include-list")).not.toBeNull();
    expect(document.querySelector("#exclude-list")).not.toBeNull();
    expect(document.querySelector("#game-power")).not.toBeNull();
    expect(document.querySelector("#game-pill")).not.toBeNull();
    expect(document.querySelector("#game-ping")).not.toBeNull();

    // Clicking DND button calls open_external with ms-settings:notifications
    const dndBtn = document.querySelector<HTMLButtonElement>("#dnd-settings-btn")!;
    expect(dndBtn).not.toBeNull();
    dndBtn.click();
    expect(invoke).toHaveBeenCalledWith("open_external", { url: "ms-settings:notifications" });
  });

  it("renders Valorant pane, validates Riot ID, links free key, and runs valo_test", async () => {
    await import("./main");
    await vi.waitFor(() => expect(document.querySelectorAll(".tab").length).toBe(7));
    const tabs = Array.from(document.querySelectorAll<HTMLButtonElement>(".tab"));
    const valoTab = tabs.find((t) => t.textContent === "Valorant")!;
    expect(valoTab).toBeDefined();
    valoTab.click();

    await vi.waitFor(() => expect(document.querySelector("#valo-riot-id")).not.toBeNull());
    const riotInput = document.querySelector<HTMLInputElement>("#valo-riot-id")!;
    const hint = document.querySelector<HTMLElement>(".hint-error")!;
    expect(hint.style.display).toBe("none");

    // Invalid Riot ID shows hint
    riotInput.value = "InvalidFormat";
    riotInput.dispatchEvent(new Event("input"));
    expect(hint.style.display).toBe("block");

    // Valid Riot ID hides hint
    riotInput.value = "Sentinel#EUW";
    riotInput.dispatchEvent(new Event("input"));
    expect(hint.style.display).toBe("none");

    // Link opens HenrikDev docs
    const link = document.querySelector<HTMLAnchorElement>("a.link")!;
    expect(link).not.toBeNull();
    link.click();
    expect(invoke).toHaveBeenCalledWith("open_external", {
      url: "https://docs.henrikdev.xyz/authentication-and-authorization",
    });

    // Test button calls valo_test
    const testBtn = document.querySelector<HTMLButtonElement>("#valo-test-btn")!;
    expect(testBtn).not.toBeNull();
    testBtn.click();
    await vi.waitFor(() => {
      const status = document.querySelector("#valo-test-status")!;
      expect(status.textContent).toBe("Platine 2 - 45 RR");
    });
  });

  it("Général: version, automatic checks toggle, check then install", async () => {
    await import("./main");
    await vi.waitFor(() => expect(document.querySelectorAll(".tab").length).toBe(7));
    Array.from(document.querySelectorAll<HTMLButtonElement>(".tab")).find((t) => t.textContent === "Général")!.click();
    await vi.waitFor(() => expect(document.querySelector("#update-btn")).not.toBeNull());
    expect(document.querySelector<HTMLInputElement>("#check-updates")!.checked).toBe(true);
    expect(document.body.textContent).toContain("Version 0.3.0");
    document.querySelector<HTMLButtonElement>("#update-btn")!.click();
    expect(invoke).toHaveBeenCalledWith("update_check", undefined);
    await vi.waitFor(() => expect(document.querySelector("#update-status")!.textContent).toContain("Lucent 0.4.0 est disponible"));
    document.querySelector<HTMLButtonElement>("#update-btn")!.click();
    expect(invoke).toHaveBeenCalledWith("update_install", undefined);
  });
});

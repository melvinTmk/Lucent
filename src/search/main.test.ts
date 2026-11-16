// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SearchGroup } from "../shared/types";

type Pending = { q: string; resolve: (g: SearchGroup[]) => void };
let pending: Pending[] = [];
/** Answers `search_query` right away when set; otherwise the test resolves `pending`. */
let answer: ((q: string) => SearchGroup[]) | null = null;

const listen = vi.fn(async (..._args: unknown[]) => () => {});
const invoke = vi.fn(async (cmd: string, args?: unknown): Promise<unknown> => {
  if (cmd === "get_settings") return null;
  if (cmd === "quicklook_open") return { kind: "text", text: "hello" };
  if (cmd === "search_query") {
    const q = (args as { q: string }).q;
    if (answer) return answer(q);
    return new Promise<SearchGroup[]>((resolve) => pending.push({ q, resolve }));
  }
  return undefined;
});

vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({ getCurrentWebviewWindow: () => ({ label: "search" }) }));

function group(key: SearchGroup["key"], ...titles: string[]): SearchGroup {
  return {
    key,
    items: titles.map((t) => ({
      entry: { id: `${key}:${t}`, kind: key === "recent" ? "app" : key, title: t, subtitle: "", icon: null, keywords: [] },
      score: 1,
      indices: [],
    })),
  };
}

async function boot(): Promise<HTMLInputElement> {
  vi.resetModules();
  document.body.innerHTML = '<div id="app"></div>';
  await import("./main");
  const input = document.querySelector<HTMLInputElement>("#query")!;
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("search_query", { q: "" }));
  return input;
}

function key(input: HTMLInputElement, k: string, mods: KeyboardEventInit = {}): void {
  input.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...mods }));
}

function type(input: HTMLInputElement, value: string): void {
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

const selectedTitle = () => document.querySelector('[aria-selected="true"] .title')?.textContent;

beforeEach(() => {
  invoke.mockClear();
  pending = [];
  answer = () => [];
});

describe("search page", () => {
  it("escape_requests_hide", async () => {
    const input = await boot();
    expect(input.value).toBe("");
    key(input, "Escape");
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("search_hide", undefined));
  });

  it("escape_clears_then_hides", async () => {
    const input = await boot();
    type(input, "fire");
    invoke.mockClear();
    key(input, "Escape");
    expect(input.value).toBe("");
    expect(invoke).not.toHaveBeenCalledWith("search_hide", undefined);
    expect(invoke).toHaveBeenCalledWith("search_query", { q: "" });
    key(input, "Escape");
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("search_hide", undefined));
  });

  it("arrow_keys_wrap_and_enter_opens_selected", async () => {
    answer = (q) => (q ? [group("app", "Firefox", "Figma"), group("file", "fichier.txt")] : []);
    const input = await boot();
    type(input, "fi");
    await vi.waitFor(() => expect(selectedTitle()).toBe("Firefox"));
    key(input, "ArrowDown");
    expect(selectedTitle()).toBe("Figma");
    key(input, "ArrowDown");
    key(input, "ArrowDown");
    expect(selectedTitle()).toBe("Firefox");
    key(input, "ArrowUp");
    expect(selectedTitle()).toBe("fichier.txt");
    key(input, "Enter", { shiftKey: true });
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("search_hide", undefined));
    const open = invoke.mock.calls.findIndex((c) => c[0] === "search_open");
    expect(invoke.mock.calls[open][1]).toEqual({ id: "file:fichier.txt", q: "fi", shift: true, ctrl: false });
    const hide = invoke.mock.calls.findIndex((c) => c[0] === "search_hide");
    expect(hide).toBeGreaterThan(open);
  });

  it("stale_responses_are_ignored", async () => {
    answer = null;
    const input = await boot();
    pending.shift()!.resolve([]);
    type(input, "a");
    type(input, "ab");
    const [a, ab] = pending;
    expect([a.q, ab.q]).toEqual(["a", "ab"]);
    ab.resolve([group("app", "AB")]);
    await vi.waitFor(() => expect(selectedTitle()).toBe("AB"));
    a.resolve([group("app", "A")]);
    await new Promise((r) => setTimeout(r, 0));
    expect([...document.querySelectorAll(".row .title")].map((e) => e.textContent)).toEqual(["AB"]);
  });

  const opened = () => invoke.mock.calls.filter((c) => c[0] === "search_open").map((c) => (c[1] as { id: string }).id);
  const flush = () => new Promise((r) => setTimeout(r, 0));

  it("enter_while_a_query_is_pending_opens_the_matching_response", async () => {
    answer = null;
    const input = await boot();
    pending.shift()!.resolve([]);
    type(input, "klo");
    pending.shift()!.resolve([group("app", "Klondike")]);
    await vi.waitFor(() => expect(selectedTitle()).toBe("Klondike"));
    type(input, "myapp");
    key(input, "Enter", { ctrlKey: true });
    await flush();
    expect(opened()).toEqual([]);
    pending.shift()!.resolve([group("app", "Myapp")]);
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("search_hide", undefined));
    expect(opened()).toEqual(["app:Myapp"]);
    expect(invoke).toHaveBeenCalledWith("search_open", { id: "app:Myapp", q: "myapp", shift: false, ctrl: true });
  });

  it("open_sends_the_query_of_the_shown_rows", async () => {
    answer = null;
    const input = await boot();
    pending.shift()!.resolve([]);
    type(input, "2+2");
    pending.shift()!.resolve([group("command", "= 4")]);
    await vi.waitFor(() => expect(selectedTitle()).toBe("= 4"));
    // A newer query is still pending: a click opens the row shown, with ITS query.
    type(input, "2+23");
    document.querySelector<HTMLElement>("[data-index='0']")!.click();
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("search_hide", undefined));
    expect(invoke).toHaveBeenCalledWith("search_open", { id: "command:= 4", q: "2+2", shift: false, ctrl: false });
  });

  it("escape_then_enter_opens_nothing_stale", async () => {
    answer = null;
    const input = await boot();
    pending.shift()!.resolve([]);
    type(input, "fire");
    pending.shift()!.resolve([group("app", "Firefox")]);
    await vi.waitFor(() => expect(selectedTitle()).toBe("Firefox"));
    key(input, "Escape");
    key(input, "Enter");
    await flush();
    expect(opened()).toEqual([]);
    // The cleared input's answer has nothing to open: still nothing, and no hide.
    pending.shift()!.resolve([]);
    await flush();
    expect(opened()).toEqual([]);
    expect(invoke).not.toHaveBeenCalledWith("search_hide", undefined);
  });

  it("a_newer_keystroke_cancels_a_pending_enter", async () => {
    answer = null;
    const input = await boot();
    pending.shift()!.resolve([]);
    type(input, "a");
    key(input, "Enter");
    type(input, "ab");
    const [a, ab] = pending;
    a.resolve([group("app", "A")]);
    ab.resolve([group("app", "AB")]);
    await vi.waitFor(() => expect(selectedTitle()).toBe("AB"));
    await flush();
    expect(opened()).toEqual([]);
  });

  it("repeated_or_double_enter_opens_once", async () => {
    answer = (q) => (q ? [group("app", "Firefox")] : []);
    const input = await boot();
    type(input, "fi");
    await vi.waitFor(() => expect(selectedTitle()).toBe("Firefox"));
    key(input, "Enter", { repeat: true });
    await flush();
    expect(opened()).toEqual([]);
    key(input, "Enter");
    key(input, "Enter");
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("search_hide", undefined));
    await flush();
    expect(opened()).toEqual(["app:Firefox"]);
  });

  it("refresh_event_reruns_the_current_query", async () => {
    let n = 0;
    answer = (q) => (q === "valo" ? [group("valorant", n++ === 0 ? "Chargement…" : "Platine 2 · 47 RR")] : []);
    const input = await boot();
    type(input, "valo");
    await vi.waitFor(() => expect(selectedTitle()).toBe("Chargement…"));
    const refresh = [...listen.mock.calls].reverse().find((c) => c[0] === "search://refresh")?.[1] as
      | ((e: { payload: null }) => void)
      | undefined;
    expect(refresh).toBeDefined();
    refresh?.({ payload: null });
    await vi.waitFor(() => expect(selectedTitle()).toBe("Platine 2 · 47 RR"));
  });

  it("shift_space_toggles_quick_look_and_escape_closes_it_before_hiding", async () => {
    answer = (q) => (q ? [group("file", "a.txt")] : []);
    const input = await boot();
    type(input, "a");
    await vi.waitFor(() => expect(selectedTitle()).toBe("a.txt"));
    // Space alone is not intercepted.
    const plain = new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true });
    input.dispatchEvent(plain);
    expect(plain.defaultPrevented).toBe(false);
    key(input, " ", { shiftKey: true });
    await vi.waitFor(() => expect(document.querySelector("#panel.quicklook")).not.toBeNull());
    expect(invoke).toHaveBeenCalledWith("quicklook_open", { id: "file:a.txt" });
    expect(document.querySelector("#preview")?.textContent).toContain("hello");
    key(input, "Escape");
    expect(document.querySelector("#panel.quicklook")).toBeNull();
    expect(invoke).toHaveBeenCalledWith("quicklook_close", {});
    expect(invoke).not.toHaveBeenCalledWith("search_hide", undefined);
    key(input, " ", { shiftKey: true });
    await vi.waitFor(() => expect(document.querySelector("#panel.quicklook")).not.toBeNull());
    key(input, " ", { shiftKey: true });
    expect(document.querySelector("#panel.quicklook")).toBeNull();
  });

  describe("power commands", () => {
    const powerGroup = (id: string, title: string): SearchGroup => ({
      key: "command",
      items: [{ entry: { id, kind: "command", title, subtitle: "Entrée : demander confirmation", icon: null, keywords: [] }, score: 1, indices: [] }],
    });
    const opened = () => invoke.mock.calls.filter((c) => c[0] === "search_open");
    const sub = () => document.querySelector('[aria-selected="true"] .sub')?.textContent;

    it("destructive row: first Enter only asks, second Enter acts", async () => {
      answer = () => [powerGroup("cmd:power:shutdown", "Éteindre")];
      const input = await boot();
      type(input, "éteindre");
      await vi.waitFor(() => expect(selectedTitle()).toBe("Éteindre"));
      key(input, "Enter");
      await vi.waitFor(() => expect(document.body.textContent).toContain("Appuie encore sur Entrée pour confirmer"));
      expect(opened()).toHaveLength(0);
      key(input, "Enter");
      await vi.waitFor(() => expect(opened()).toHaveLength(1));
      expect(opened()[0]![1]).toMatchObject({ id: "cmd:power:shutdown" });
    });

    it("an expired confirmation asks again instead of acting", async () => {
      answer = () => [powerGroup("cmd:power:restart", "Redémarrer")];
      const input = await boot();
      type(input, "redémarrer");
      await vi.waitFor(() => expect(selectedTitle()).toBe("Redémarrer"));
      const now = vi.spyOn(performance, "now");
      now.mockReturnValue(1000);
      key(input, "Enter");
      await vi.waitFor(() => expect(document.body.textContent).toContain("Appuie encore"));
      now.mockReturnValue(1000 + 4001);
      key(input, "Enter");
      await new Promise((r) => setTimeout(r, 20));
      expect(opened()).toHaveLength(0);
      now.mockRestore();
      expect(sub() ?? "").toBeDefined();
    });

    it("lock needs no confirmation", async () => {
      answer = () => [powerGroup("cmd:power:lock", "Verrouiller")];
      const input = await boot();
      type(input, "verrouiller");
      await vi.waitFor(() => expect(selectedTitle()).toBe("Verrouiller"));
      key(input, "Enter");
      await vi.waitFor(() => expect(opened()).toHaveLength(1));
    });
  });
});

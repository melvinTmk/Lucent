// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { QuickLookPreview, SearchEntry } from "../shared/types";

let reply: QuickLookPreview = { kind: "none" };
const invoke = vi.fn(async (cmd: string, _args?: unknown): Promise<unknown> => (cmd === "quicklook_open" ? reply : undefined));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({ getCurrentWebviewWindow: () => ({ label: "search" }) }));

const { canPreview, createQuickLook, formatSize, renderPreview } = await import("./quicklook");

const entry = (kind: SearchEntry["kind"], id = `${kind}:x`): SearchEntry => ({
  id,
  kind,
  title: "x.png",
  subtitle: "~/pics",
  icon: null,
  keywords: [],
});

beforeEach(() => {
  invoke.mockClear();
  reply = { kind: "none" };
});

describe("quicklook", () => {
  it("previews only file, project and app entries", () => {
    expect(["file", "project", "app"].every((k) => canPreview(entry(k as SearchEntry["kind"])))).toBe(true);
    expect(["command", "clipboard", "emoji", "valorant"].some((k) => canPreview(entry(k as SearchEntry["kind"])))).toBe(false);
    expect(canPreview(undefined)).toBe(false);
  });

  it("formats sizes", () => {
    expect(formatSize(12)).toBe("12 o");
    expect(formatSize(1536)).toBe("1,5 Ko");
    expect(formatSize(5 * 1024 * 1024)).toBe("5,0 Mo");
    expect(formatSize(300 * 1024 * 1024)).toBe("300 Mo");
  });

  it("renders each kind", () => {
    const e = entry("file");
    expect(renderPreview(e, { kind: "image", url: "http://qlook.localhost/t" }).tagName).toBe("IMG");
    expect(renderPreview(e, { kind: "video", url: "http://qlook.localhost/t" }).tagName).toBe("VIDEO");
    expect(renderPreview(e, { kind: "pdf", url: "http://qlook.localhost/t" }).tagName).toBe("IFRAME");
    const text = renderPreview(e, { kind: "text", text: "a\nb\nc" });
    expect(text.querySelector(".ql-gutter")?.textContent).toBe("1\n2\n3");
    expect(text.querySelector(".ql-code")?.textContent).toBe("a\nb\nc");
    const none = renderPreview(e, { kind: "none", name: "n.bin", size: 2048, modified: 0 });
    expect(none.querySelector(".ql-name")?.textContent).toBe("n.bin");
    expect(none.textContent).toContain("2,0 Ko");
  });

  it("toggles: opens with the entry id only, closes on the second call and tells Rust", async () => {
    const host = document.createElement("div");
    host.hidden = true;
    const changes: boolean[] = [];
    const ql = createQuickLook(host, (o) => changes.push(o));
    reply = { kind: "image", url: "http://qlook.localhost/t" };
    await ql.toggle(entry("file", "file:C:\\a.png"));
    expect(ql.isOpen()).toBe(true);
    expect(host.hidden).toBe(false);
    expect(invoke).toHaveBeenCalledWith("quicklook_open", { id: "file:C:\\a.png" });
    expect(host.querySelector("img")).not.toBeNull();
    await ql.toggle(entry("file"));
    expect(ql.isOpen()).toBe(false);
    expect(host.hidden).toBe(true);
    expect(host.childElementCount).toBe(0);
    expect(invoke).toHaveBeenCalledWith("quicklook_close", {});
    expect(changes).toEqual([true, false]);
  });

  it("Esc path: close() is idempotent; a command row never opens", async () => {
    const host = document.createElement("div");
    const ql = createQuickLook(host, () => {});
    await ql.toggle(entry("command"));
    expect(ql.isOpen()).toBe(false);
    expect(invoke).not.toHaveBeenCalled();
    await ql.toggle(entry("app"));
    ql.close();
    ql.close();
    expect(invoke.mock.calls.filter((c) => c[0] === "quicklook_close")).toHaveLength(1);
  });

  it("a response arriving after close is dropped", async () => {
    const host = document.createElement("div");
    const ql = createQuickLook(host, () => {});
    let release: (p: QuickLookPreview) => void = () => {};
    invoke.mockImplementationOnce(() => new Promise((r) => (release = r as typeof release)));
    const p = ql.toggle(entry("file"));
    ql.close();
    release({ kind: "image", url: "u" });
    await p;
    expect(ql.isOpen()).toBe(false);
    expect(host.childElementCount).toBe(0);
  });
});

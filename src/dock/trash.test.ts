// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderTrash } from "./trash";
import type { TrashStatus } from "../shared/types";

const { invoke } = vi.hoisted(() => ({
  invoke: vi.fn(async () => undefined),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

describe("renderTrash", () => {
  let parent: HTMLElement;

  beforeEach(() => {
    document.body.innerHTML = '<div id="parent"></div>';
    parent = document.getElementById("parent")!;
    invoke.mockClear();
  });

  it("renders empty trash when count is 0", () => {
    const status: TrashStatus = { count: 0, bytes: 0 };
    const el = renderTrash(parent, status);
    expect(el.classList.contains("trash")).toBe(true);
    expect(el.classList.contains("full")).toBe(false);
    expect(el.getAttribute("aria-label")).toContain("vide");
  });

  it("renders full trash when count > 0", () => {
    const status: TrashStatus = { count: 5, bytes: 1048576 };
    const el = renderTrash(parent, status);
    expect(el.classList.contains("full")).toBe(true);
    expect(el.getAttribute("aria-label")).toContain("5");
  });

  it("left click opens shell:RecycleBinFolder", () => {
    const status: TrashStatus = { count: 2, bytes: 500 };
    const el = renderTrash(parent, status);
    el.click();
    expect(invoke).toHaveBeenCalledWith("open_external", { url: "shell:RecycleBinFolder" });
  });

  it("contextmenu shows empty trash action and clicking it calls empty_trash", async () => {
    const onEmpty = vi.fn();
    const status: TrashStatus = { count: 3, bytes: 1000 };
    const el = renderTrash(parent, status, onEmpty);
    el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));

    const menu = document.querySelector(".trash-menu");
    expect(menu).not.toBeNull();
    const emptyBtn = menu!.querySelector<HTMLButtonElement>(".trash-menu-item")!;
    expect(emptyBtn).not.toBeNull();
    emptyBtn.click();

    expect(invoke).toHaveBeenCalledWith("empty_trash", undefined);
  });
});

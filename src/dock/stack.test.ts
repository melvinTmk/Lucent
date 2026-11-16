// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { StackFile } from "../shared/types";
import { closeDownloadsStack, renderDownloadsStack } from "./stack";

const sampleFiles: StackFile[] = [
  {
    name: "screenshot.png",
    path: "C:\\Users\\test\\Downloads\\screenshot.png",
    size: 204800,
    modified: Date.now() - 5 * 60 * 1000, // 5 min ago
    isDir: false,
  },
  {
    name: "Documents",
    path: "C:\\Users\\test\\Downloads\\Documents",
    size: 0,
    modified: Date.now() - 2 * 3600 * 1000, // 2 h ago
    isDir: true,
  },
];

const { invoke } = vi.hoisted(() => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === "list_stack_files") return sampleFiles;
    return undefined;
  }),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

describe("Downloads stack popover", () => {
  let container: HTMLElement;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    invoke.mockClear();
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "list_stack_files") return sampleFiles;
      return undefined;
    });
  });

  afterEach(() => {
    closeDownloadsStack();
    container.remove();
    document.body.innerHTML = "";
    vi.restoreAllMocks();
  });

  it("renders the stack button with icon and aria-label", () => {
    const btn = renderDownloadsStack(container);
    expect(btn.classList.contains("stack")).toBe(true);
    expect(btn.getAttribute("aria-label")).toBe("Téléchargements");
    expect(btn.querySelector("svg")).not.toBeNull();
  });

  it("toggles the popover open on click and loads files", async () => {
    const btn = renderDownloadsStack(container);
    btn.click();

    await vi.waitFor(() => {
      expect(document.querySelector(".stack-popover")).not.toBeNull();
      expect(document.querySelectorAll(".stack-item").length).toBe(2);
    });

    expect(invoke).toHaveBeenCalledWith("list_stack_files", {
      folder: "downloads",
      limit: 8,
    });

    const items = document.querySelectorAll(".stack-item");
    expect(items[0].textContent).toContain("screenshot.png");
    expect(items[1].textContent).toContain("Documents");
  });

  it("clicking a file item opens it via open_external and closes popover", async () => {
    const btn = renderDownloadsStack(container);
    btn.click();

    await vi.waitFor(() => {
      expect(document.querySelectorAll(".stack-item").length).toBe(2);
    });

    const firstItem = document.querySelectorAll<HTMLElement>(".stack-item")[0];
    firstItem.click();

    expect(invoke).toHaveBeenCalledWith("open_external", {
      url: "C:\\Users\\test\\Downloads\\screenshot.png",
    });
    expect(document.querySelector(".stack-popover")).toBeNull();
  });

  it("clicking 'Ouvrir dans l'Explorateur' opens Downloads folder and closes popover", async () => {
    const btn = renderDownloadsStack(container);
    btn.click();

    await vi.waitFor(() => {
      expect(document.querySelector(".stack-footer-btn")).not.toBeNull();
    });

    const footer = document.querySelector<HTMLElement>(".stack-footer-btn");
    expect(footer!.textContent).toContain("Ouvrir dans l'Explorateur");

    footer!.click();
    expect(invoke).toHaveBeenCalledWith("open_external", {
      url: "shell:Downloads",
    });
    expect(document.querySelector(".stack-popover")).toBeNull();
  });

  it("clicking outside closes the popover", async () => {
    const btn = renderDownloadsStack(container);
    btn.click();

    await vi.waitFor(() => {
      expect(document.querySelector(".stack-popover")).not.toBeNull();
    });

    document.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.querySelector(".stack-popover")).toBeNull();
  });

  it("pressing Escape closes the popover", async () => {
    const btn = renderDownloadsStack(container);
    btn.click();

    await vi.waitFor(() => {
      expect(document.querySelector(".stack-popover")).not.toBeNull();
    });

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(document.querySelector(".stack-popover")).toBeNull();
  });
});

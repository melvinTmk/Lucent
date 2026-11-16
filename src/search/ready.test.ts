// @vitest-environment jsdom
// Rust reveals a fresh search window on its first `search_resize`, then emits
// `search://shown`: that first report must wait until the page listens.
import { expect, it, vi } from "vitest";

let shownRegistered = false;
let releaseShown: () => void = () => {};
const listen = vi.fn((event: string) => {
  if (event !== "search://shown") return Promise.resolve(() => {});
  return new Promise<() => void>((resolve) => {
    releaseShown = () => {
      shownRegistered = true;
      resolve(() => {});
    };
  });
});
const resizeCalls: boolean[] = [];
const invoke = vi.fn(async (cmd: string) => {
  if (cmd === "search_resize") resizeCalls.push(shownRegistered);
  return cmd === "get_settings" ? null : undefined;
});

vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({ getCurrentWebviewWindow: () => ({ label: "search" }) }));

it("first_resize_waits_for_shown_listener", async () => {
  // An observer that reports right away, like a real first layout.
  globalThis.ResizeObserver = class {
    constructor(private cb: ResizeObserverCallback) {}
    observe(): void {
      this.cb([], this as unknown as ResizeObserver);
    }
    unobserve(): void {}
    disconnect(): void {}
  } as unknown as typeof ResizeObserver;
  document.body.innerHTML = '<div id="app"></div>';
  await import("./main");
  await new Promise((r) => setTimeout(r, 0));
  expect(resizeCalls).toEqual([]);
  releaseShown();
  await vi.waitFor(() => expect(resizeCalls.length).toBeGreaterThan(0));
  expect(resizeCalls.every((registered) => registered)).toBe(true);
});

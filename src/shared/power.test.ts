import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({ getCurrentWebviewWindow: () => ({ label: "x" }) }));

const { createConfirmGate, isDestructive, isPowerId, confirmText } = await import("./power");

describe("power helpers", () => {
  it("flags only signout, restart and shutdown as destructive", () => {
    expect(["lock", "sleep", "signout", "restart", "shutdown"].filter(isDestructive)).toEqual(["signout", "restart", "shutdown"]);
    expect(isPowerId("hibernate")).toBe(false);
    expect(confirmText("restart")).toBe("Redémarrer maintenant ? Les applications ouvertes vont se fermer.");
    expect(confirmText("lock")).toBeNull();
  });

  it("confirm gate: second press within the timeout confirms", () => {
    let t = 0;
    const g = createConfirmGate(4000, () => t);
    expect(g.press("a")).toBe(false);
    expect(g.armed()).toBe("a");
    t = 3999;
    expect(g.press("a")).toBe(true);
    expect(g.armed()).toBeNull();
    expect(g.press("a")).toBe(false); // consumed: arms again
  });

  it("confirm gate: expiry, other row and reset re-arm instead of confirming", () => {
    let t = 0;
    const g = createConfirmGate(4000, () => t);
    g.press("a");
    t = 4001;
    expect(g.armed()).toBeNull();
    expect(g.press("a")).toBe(false);
    expect(g.press("b")).toBe(false);
    expect(g.press("a")).toBe(false);
    g.reset();
    expect(g.press("a")).toBe(false);
  });
});

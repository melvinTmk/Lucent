// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { attachReorder, beforeIdAfterDrop, clampSlot, dropSlot, moveItem, shiftFor, type Reorder } from "./reorder";

describe("reorder model", () => {
  it("dropSlot counts the other icons left of the pointer", () => {
    expect(dropSlot([10, 50, 90], 0, 60)).toBe(1);
    expect(dropSlot([10, 50, 90], 0, 200)).toBe(2);
    expect(dropSlot([10, 50, 90], 2, -5)).toBe(0);
    expect(dropSlot([10, 50, 90], 1, 50)).toBe(1);
  });

  it("beforeIdAfterDrop names the next item in the final order", () => {
    expect(beforeIdAfterDrop(["a", "b", "c"], 2, 0)).toBe("a");
    expect(beforeIdAfterDrop(["a", "b", "c"], 0, 2)).toBeNull();
    expect(beforeIdAfterDrop(["a", "b", "c"], 0, 1)).toBe("c");
  });

  it("clampSlot keeps a pinned icon within the pinned slots", () => {
    // 2 other pinned icons: slots 0..2 allowed.
    expect(clampSlot(1, 2)).toBe(1);
    expect(clampSlot(4, 2)).toBe(2);
    expect(clampSlot(2, 2)).toBe(2);
  });

  it("clampSlot clamps an unpinned icon to just after the last pinned one too", () => {
    // Unpinned icon at index 4, 3 pinned icons before it: slots 0..3.
    expect(clampSlot(5, 3)).toBe(3);
    expect(clampSlot(4, 3)).toBe(3);
    expect(clampSlot(3, 3)).toBe(3);
    expect(clampSlot(0, 3)).toBe(0);
  });

  it("shiftFor moves the icons between from and to by one slot", () => {
    expect(shiftFor(1, 0, 2, 48)).toBe(-48);
    expect(shiftFor(0, 2, 0, 48)).toBe(48);
    expect(shiftFor(2, 0, 1, 48)).toBe(0);
  });
});

describe("moveItem", () => {
  const list = [
    { id: "a", isGame: false },
    { id: "g1", isGame: true },
    { id: "b", isGame: false },
    { id: "g2", isGame: true },
    { id: "c", isGame: false },
  ];
  const order = (l: { id: string }[]) => l.map((i) => i.id);

  it("moves the item before beforeId", () => {
    expect(order(moveItem(list, "c", "a"))).toEqual(["c", "a", "g1", "b", "g2"]);
    expect(order(moveItem(list, "g2", "g1"))).toEqual(["a", "g2", "g1", "b", "c"]);
  });

  it("null puts it after the last item of its own section", () => {
    expect(order(moveItem(list, "a", null))).toEqual(["g1", "b", "g2", "c", "a"]);
    expect(order(moveItem(list, "g1", null))).toEqual(["a", "b", "g2", "g1", "c"]);
  });

  it("mirrors Rust: the moved item becomes pinned and lands among the pinned ones", () => {
    const items = [
      { id: "p1", isGame: false, pinned: true },
      { id: "p2", isGame: false, pinned: true },
      { id: "u1", isGame: false, pinned: false },
      { id: "u2", isGame: false, pinned: false },
      { id: "g1", isGame: true, pinned: true },
    ];
    // Unpinned u2 dropped before u1 (just after the pinned): pinned, after p2.
    const a = moveItem(items, "u2", "u1");
    expect(order(a)).toEqual(["p1", "p2", "u2", "u1", "g1"]);
    expect(a.find((i) => i.id === "u2")!.pinned).toBe(true);
    // The first unpinned one dropped in place (beforeId = next unpinned): last pin.
    const first = moveItem(items, "u1", "u2");
    expect(order(first)).toEqual(["p1", "p2", "u1", "u2", "g1"]);
    expect(first.find((i) => i.id === "u1")!.pinned).toBe(true);
    // null = end of the section: after the last pinned item, before the unpinned ones.
    expect(order(moveItem(items, "p1", null))).toEqual(["p2", "p1", "u1", "u2", "g1"]);
    expect(order(moveItem(items, "u2", null))).toEqual(["p1", "p2", "u2", "u1", "g1"]);
    // Never mutates the input objects.
    expect(items.find((i) => i.id === "u2")!.pinned).toBe(false);
  });

  it("returns the list unchanged (same contents) for unknown ids, never mutates", () => {
    const copy = order(list);
    expect(order(moveItem(list, "zz", "a"))).toEqual(copy);
    expect(order(moveItem(list, "a", "zz"))).toEqual(copy);
    expect(order(list)).toEqual(copy);
  });
});

// ---- DOM controller ------------------------------------------------------------------

/** Rest main-axis centres (jsdom has no layout). */
const CENTERS: Record<string, number> = { a: 10, b: 50, c: 90, g1: 150, g2: 190 };

function pointer(type: string, x: number, init: { button?: number; buttons?: number; y?: number } = {}): Event {
  const opts = {
    bubbles: true,
    cancelable: true,
    clientX: x,
    clientY: init.y ?? 0,
    button: init.button ?? 0,
    buttons: init.buttons ?? (type === "pointerup" ? 0 : 1),
  };
  const Ctor = (globalThis as { PointerEvent?: typeof MouseEvent }).PointerEvent ?? MouseEvent;
  const e = new Ctor(type, { ...opts, pointerId: 1 } as MouseEventInit);
  if (!("pointerId" in e)) Object.defineProperty(e, "pointerId", { value: 1 });
  return e;
}

const btn = (id: string) => document.querySelector<HTMLElement>(`.app[data-id="${id}"]`)!;
const ids = () => [...document.querySelectorAll<HTMLElement>(".island.apps > *")].map((el) => el.dataset.id ?? "|");

describe("attachReorder", () => {
  let dock: HTMLElement;
  let save: ReturnType<typeof vi.fn<(itemId: string, beforeId: string | null) => void>>;
  let onDragStart: ReturnType<typeof vi.fn<() => void>>;
  let reorder: Reorder;
  let clicked: string[];
  let onClick: (e: Event) => void;

  beforeEach(() => {
    document.body.innerHTML = `<div id="dock"><div class="island apps">
      <button class="app" data-id="a"></button><button class="app" data-id="b"></button>
      <button class="app" data-id="c"></button><div class="sep"></div>
      <button class="app game" data-id="g1"></button><button class="app game" data-id="g2"></button>
    </div><div class="island tray"><button class="tray-icon"></button></div></div>`;
    dock = document.getElementById("dock")!;
    save = vi.fn<(itemId: string, beforeId: string | null) => void>();
    onDragStart = vi.fn<() => void>();
    reorder = attachReorder(dock, {
      vertical: () => false,
      iconSize: () => 36,
      restCenter: (el) => CENTERS[el.dataset.id ?? ""] ?? 0,
      onDragStart,
      save,
    });
    clicked = [];
    // Registered after the controller, like main.ts's dock_click handler.
    onClick = (e) => clicked.push((e.target as HTMLElement).closest<HTMLElement>(".app")?.dataset.id ?? "");
    dock.addEventListener("click", onClick);
  });

  afterEach(() => {
    reorder.detach();
    vi.useRealTimers();
  });

  it("a 2 px wobble is still a click", () => {
    btn("b").dispatchEvent(pointer("pointerdown", 50));
    btn("b").dispatchEvent(pointer("pointermove", 52, { y: 1 }));
    btn("b").dispatchEvent(pointer("pointerup", 52));
    btn("b").click();
    expect(clicked).toEqual(["b"]);
    expect(save).not.toHaveBeenCalled();
    expect(onDragStart).not.toHaveBeenCalled();
    expect(reorder.dragging()).toBe(false);
  });

  it("a completed drag saves the move, reorders optimistically and swallows the click", () => {
    btn("a").dispatchEvent(pointer("pointerdown", 10));
    btn("a").dispatchEvent(pointer("pointermove", 20));
    expect(reorder.dragging()).toBe(true);
    expect(onDragStart).toHaveBeenCalledTimes(1);
    expect(dock.classList.contains("reordering")).toBe(true);
    expect(btn("a").classList.contains("dragging")).toBe(true);
    btn("a").dispatchEvent(pointer("pointermove", 60));
    // a's centre is now at 60: b slid back one slot (40 px between rest centres), c stays.
    expect(btn("a").style.transform).toBe("translate(50px, 0px)");
    expect(btn("b").style.transform).toBe("translate(-40px, 0px)");
    expect(btn("c").style.transform).toBe("");
    btn("a").dispatchEvent(pointer("pointerup", 60));

    expect(save).toHaveBeenCalledWith("a", "c");
    expect(ids()).toEqual(["b", "a", "c", "|", "g1", "g2"]);
    expect(btn("b").style.transform).toBe("");
    expect(dock.classList.contains("reordering")).toBe(false);
    expect(btn("a").classList.contains("dragging")).toBe(false);
    expect(reorder.dragging()).toBe(false);

    btn("a").click();
    expect(clicked).toEqual([]);
    // One-shot: the next click goes through.
    btn("a").click();
    expect(clicked).toEqual(["a"]);
  });

  it("dropping last sends beforeId null and dropping first names the old first", () => {
    btn("a").dispatchEvent(pointer("pointerdown", 10));
    btn("a").dispatchEvent(pointer("pointermove", 400)); // clamped to the section's end
    btn("a").dispatchEvent(pointer("pointerup", 400));
    expect(save).toHaveBeenLastCalledWith("a", null);
    expect(ids()).toEqual(["b", "c", "a", "|", "g1", "g2"]);

    btn("g2").dispatchEvent(pointer("pointerdown", 190));
    btn("g2").dispatchEvent(pointer("pointermove", -300)); // cannot leave the games
    btn("g2").dispatchEvent(pointer("pointerup", -300));
    expect(save).toHaveBeenLastCalledWith("g2", "g1");
    expect(ids()).toEqual(["b", "c", "a", "|", "g2", "g1"]);
  });

  it("the swallow expires after 300 ms", () => {
    vi.useFakeTimers();
    btn("a").dispatchEvent(pointer("pointerdown", 10));
    btn("a").dispatchEvent(pointer("pointermove", 60));
    btn("a").dispatchEvent(pointer("pointerup", 60));
    vi.advanceTimersByTime(301);
    btn("a").click();
    expect(clicked).toEqual(["a"]);
  });

  it("dropping back in place saves nothing but still swallows the click", () => {
    btn("b").dispatchEvent(pointer("pointerdown", 50));
    btn("b").dispatchEvent(pointer("pointermove", 58));
    btn("b").dispatchEvent(pointer("pointerup", 58));
    expect(save).not.toHaveBeenCalled();
    btn("b").click();
    expect(clicked).toEqual([]);
  });

  it.each([
    ["pointercancel", () => btn("a").dispatchEvent(pointer("pointercancel", 60))],
    ["lostpointercapture", () => btn("a").dispatchEvent(pointer("lostpointercapture", 60))],
    ["Escape", () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))],
    ["cancel()", () => reorder.cancel()],
  ])("%s cancels the drag without saving", (_name, cancel) => {
    btn("a").dispatchEvent(pointer("pointerdown", 10));
    btn("a").dispatchEvent(pointer("pointermove", 60));
    cancel();
    expect(reorder.dragging()).toBe(false);
    for (const id of ["a", "b", "c"]) expect(btn(id).style.transform).toBe("");
    expect(dock.classList.contains("reordering")).toBe(false);
    expect(btn("a").classList.contains("dragging")).toBe(false);
    // The user saw the gesture cancelled: releasing the button must not click the app.
    btn("a").dispatchEvent(pointer("pointerup", 60));
    btn("a").click();
    expect(clicked).toEqual([]);
    expect(save).not.toHaveBeenCalled();
    expect(ids()).toEqual(["a", "b", "c", "|", "g1", "g2"]);
    // Only that one click: the next press-and-click works.
    btn("a").dispatchEvent(pointer("pointerdown", 10));
    btn("a").dispatchEvent(pointer("pointerup", 10));
    btn("a").click();
    expect(clicked).toEqual(["a"]);
  });

  it("a cancel's pending swallow does not leak into the next gesture", () => {
    btn("a").dispatchEvent(pointer("pointerdown", 10));
    btn("a").dispatchEvent(pointer("pointermove", 60));
    reorder.cancel();
    // The button was released outside the window (no pointerup seen); a fresh press:
    btn("b").dispatchEvent(pointer("pointerdown", 50));
    btn("b").dispatchEvent(pointer("pointerup", 50));
    btn("b").click();
    expect(clicked).toEqual(["b"]);
  });

  it("never starts on the separator or the tray, nor with another button", () => {
    const sep = document.querySelector<HTMLElement>(".sep")!;
    sep.dispatchEvent(pointer("pointerdown", 120));
    sep.dispatchEvent(pointer("pointermove", 20));
    const tray = document.querySelector<HTMLElement>(".tray-icon")!;
    tray.dispatchEvent(pointer("pointerdown", 300));
    tray.dispatchEvent(pointer("pointermove", 200));
    btn("a").dispatchEvent(pointer("pointerdown", 10, { button: 2, buttons: 2 }));
    btn("a").dispatchEvent(pointer("pointermove", 60, { buttons: 2 }));
    expect(reorder.dragging()).toBe(false);
    expect(onDragStart).not.toHaveBeenCalled();
  });

  it("a pinned icon dropped past running unpinned apps lands just after the last pinned", () => {
    btn("c").dataset.pinned = "false"; // a, b pinned; c running unpinned
    btn("a").dispatchEvent(pointer("pointerdown", 10));
    btn("a").dispatchEvent(pointer("pointermove", 400));
    // The icon keeps following the pointer (section end), only the slot is clamped: c stays.
    expect(btn("a").style.transform).toBe("translate(80px, 0px)");
    expect(btn("c").style.transform).toBe("");
    btn("a").dispatchEvent(pointer("pointerup", 400));
    expect(save).toHaveBeenLastCalledWith("a", "c");
    expect(ids()).toEqual(["b", "a", "c", "|", "g1", "g2"]);
  });

  it("an unpinned app dropped past the pinned ones lands just after the last pinned (saved = pinned)", () => {
    btn("b").dataset.pinned = "false";
    btn("c").dataset.pinned = "false"; // a pinned; b, c unpinned
    btn("c").dispatchEvent(pointer("pointerdown", 90));
    btn("c").dispatchEvent(pointer("pointermove", 400));
    expect(btn("c").style.transform).toBe("");
    // Follows the pointer to the section end; dropped past b: lands just after a.
    btn("c").dispatchEvent(pointer("pointermove", 140));
    btn("c").dispatchEvent(pointer("pointerup", 140));
    expect(save).toHaveBeenLastCalledWith("c", "b");
    expect(ids()).toEqual(["a", "c", "b", "|", "g1", "g2"]);
  });

  it("the first unpinned app dropped in its own slot still commits (becomes the last pin)", () => {
    btn("b").dataset.pinned = "false";
    btn("c").dataset.pinned = "false"; // a pinned; b is the first unpinned
    btn("b").dispatchEvent(pointer("pointerdown", 50));
    btn("b").dispatchEvent(pointer("pointermove", 400));
    expect(btn("b").style.transform).toBe("translate(40px, 0px)");
    expect(btn("c").style.transform).toBe("");
    btn("b").dispatchEvent(pointer("pointerup", 400));
    expect(save).toHaveBeenLastCalledWith("b", "c");
    expect(ids()).toEqual(["a", "b", "c", "|", "g1", "g2"]);

    // The only unpinned app, already last: beforeId null.
    btn("b").dataset.pinned = "true";
    btn("c").dispatchEvent(pointer("pointerdown", 90));
    btn("c").dispatchEvent(pointer("pointermove", 400));
    btn("c").dispatchEvent(pointer("pointerup", 400));
    expect(save).toHaveBeenLastCalledWith("c", null);
    expect(ids()).toEqual(["a", "b", "c", "|", "g1", "g2"]);
  });

  it("a move with the button already released does not start a drag", () => {
    btn("a").dispatchEvent(pointer("pointerdown", 10));
    btn("a").dispatchEvent(pointer("pointermove", 60, { buttons: 0 }));
    expect(reorder.dragging()).toBe(false);
  });
});

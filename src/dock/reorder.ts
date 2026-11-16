// Drag-to-reorder for the dock's app icons. Pure slot maths + a DOM controller that
// main.ts attaches with its geometry helpers injected (so jsdom tests can drive it).
// The drag only moves icons with transforms: layout (offset*, the reported window
// region) stays at rest geometry until the drop reorders the DOM.

/** Pointer travel (CSS px, either axis) before a press on an icon becomes a drag. */
export const DRAG_THRESHOLD_PX = 4;
/** The click that follows a drop is swallowed for at most this long. */
const SWALLOW_MS = 300;
/** `.reorder-settle` transition (dock.css) + a little slack. */
const SETTLE_MS = 220;

/** Index the dragged icon would occupy: the number of *other* icons centred before `pos`. */
export function dropSlot(centers: number[], from: number, pos: number): number {
  let n = 0;
  centers.forEach((c, i) => {
    if (i !== from && c < pos) n++;
  });
  return n;
}

/**
 * Limits a slot to the pinned part of the section. `maxSlot`: number of *other* pinned
 * icons before the running unpinned ones (Rust re-emits a moved item right after the last
 * pinned icon, so a slot past them would jump back after the drop). Applies to unpinned
 * icons too: dropped anywhere among the apps, they become pinned at that place.
 */
export function clampSlot(slot: number, maxSlot: number): number {
  return Math.min(slot, maxSlot);
}

/** The id the moved item lands before in the final order, `null` when it ends last. */
export function beforeIdAfterDrop(ids: string[], from: number, to: number): string | null {
  const rest = ids.filter((_, i) => i !== from);
  return rest[to] ?? null;
}

/** Main-axis translation of non-dragged icon `i` while `from` hovers over slot `to`. */
export function shiftFor(i: number, from: number, to: number, slot: number): number {
  if (from < i && i <= to) return -slot;
  if (to <= i && i < from) return slot;
  return 0;
}

/**
 * The model-side twin of a drop (mirrors Rust's `dock_reorder` + `build_items`): `itemId`
 * moved before `beforeId`, or after the last item of its own section when `beforeId` is
 * null. With a `pinned` field, the moved item becomes pinned and the list is re-ordered
 * like Rust's: pinned non-games, running unpinned apps, games (stable).
 * Returns a new list (never mutates); unknown ids leave the order as is.
 */
export function moveItem<T extends { id: string; isGame: boolean; pinned?: boolean }>(
  list: T[],
  itemId: string,
  beforeId: string | null,
): T[] {
  const item = list.find((i) => i.id === itemId);
  if (!item) return [...list];
  const rest = list.filter((i) => i !== item);
  let at: number;
  if (beforeId === null) {
    at = 0;
    rest.forEach((i, idx) => {
      if (i.isGame === item.isGame) at = idx + 1;
    });
  } else {
    at = rest.findIndex((i) => i.id === beforeId);
    if (at < 0) return [...list];
  }
  if (!("pinned" in item)) {
    rest.splice(at, 0, item);
    return rest;
  }
  rest.splice(at, 0, { ...item, pinned: true });
  const rank = (i: T) => (i.isGame ? 2 : i.pinned ? 0 : 1);
  return rest.map((i, idx) => [i, idx] as const).sort((a, b) => rank(a[0]) - rank(b[0]) || a[1] - b[1]).map(([i]) => i);
}

export interface ReorderDeps {
  vertical(): boolean;
  iconSize(): number;
  /** Rest main-axis centre of an icon, in the same coordinates as clientX/clientY. */
  restCenter(el: HTMLElement): number;
  /** A drag starts (pause magnification). */
  onDragStart(): void;
  /** A drag ended; `moved`: the DOM was reordered. */
  onDragEnd?(moved: boolean): void;
  save(itemId: string, beforeId: string | null): void;
}

export interface Reorder {
  dragging(): boolean;
  /** Ends a drag without saving (no-op when idle). */
  cancel(): void;
  detach(): void;
}

interface Press {
  btn: HTMLElement;
  pointerId: number;
  x: number;
  y: number;
}

interface Drag {
  btn: HTMLElement;
  pointerId: number;
  section: HTMLElement[];
  centers: number[];
  from: number;
  to: number;
  /** Highest slot allowed (see `clampSlot`). */
  maxSlot: number;
  /** A running unpinned app: any drop pins it, even in its own slot. */
  unpinned: boolean;
  slot: number;
  /** Pointer position minus the dragged icon's rest centre at grab time. */
  grab: number;
  /** Current (clamped) main-axis centre of the dragged icon. */
  at: number;
}

const translate = (v: boolean, d: number): string => (d === 0 ? "" : v ? `translate(0px, ${d}px)` : `translate(${d}px, 0px)`);

export function attachReorder(dock: HTMLElement, deps: ReorderDeps): Reorder {
  const doc = dock.ownerDocument;
  let press: Press | null = null;
  let drag: Drag | null = null;
  let settleTimer = 0;
  let swallowTimer = 0;
  /** A drag was cancelled while the button may still be down: its release must not click. */
  let swallowNextUp = false;

  const appButtons = () => [...dock.querySelectorAll<HTMLElement>(".island.apps > .app")];
  const mainOf = (e: MouseEvent) => (deps.vertical() ? e.clientY : e.clientX);
  const idOf = (e: Event) => (e as PointerEvent).pointerId ?? 0;

  function swallow(e: Event): void {
    e.preventDefault();
    e.stopImmediatePropagation();
    stopSwallow();
  }
  function stopSwallow(): void {
    clearTimeout(swallowTimer);
    doc.removeEventListener("click", swallow, true);
  }
  function startSwallow(): void {
    stopSwallow();
    doc.addEventListener("click", swallow, true);
    swallowTimer = window.setTimeout(stopSwallow, SWALLOW_MS);
  }

  /** Lets the icons glide to their final place (transform transition), then drops it. */
  function settle(): void {
    dock.classList.add("reorder-settle");
    clearTimeout(settleTimer);
    settleTimer = window.setTimeout(() => dock.classList.remove("reorder-settle"), SETTLE_MS);
  }

  function begin(p: Press): void {
    const kindGame = p.btn.classList.contains("game");
    const section = appButtons().filter((b) => b.classList.contains("game") === kindGame);
    const from = section.indexOf(p.btn);
    if (from < 0) return;
    const centers = section.map((b) => deps.restCenter(b));
    // Games are all pinned; buttons without `data-pinned` count as pinned.
    const pinnedOthers = section.filter((b) => b !== p.btn && b.dataset.pinned !== "false").length;
    const maxSlot = kindGame ? section.length - 1 : pinnedOthers;
    const slot = centers.length >= 2 ? centers[1] - centers[0] : deps.iconSize() + 12;
    const half = deps.iconSize() / 2;
    // Pressed on a magnified icon, the pointer may be far from its rest centre: keep the
    // grab point, but never let the icon slip out from under the cursor.
    const start = deps.vertical() ? p.y : p.x;
    const grab = Math.max(-half, Math.min(half, start - centers[from]));
    const unpinned = !kindGame && p.btn.dataset.pinned === "false";
    drag = { btn: p.btn, pointerId: p.pointerId, section, centers, from, to: from, maxSlot, unpinned, slot, grab, at: centers[from] };
    p.btn.setPointerCapture?.(p.pointerId);
    clearTimeout(settleTimer);
    dock.classList.remove("reorder-settle");
    deps.onDragStart();
    dock.classList.add("reordering");
    p.btn.classList.add("dragging");
  }

  function update(pos: number): void {
    const d = drag!;
    const v = deps.vertical();
    const first = d.centers[0];
    // The icon follows the pointer along the whole section; only the slot is clamped.
    const last = d.centers[d.centers.length - 1];
    const centre = pos - d.grab;
    d.at = Math.max(first, Math.min(last, centre));
    // Slot from the unclamped position: reaching the last centre already means "last".
    d.to = clampSlot(dropSlot(d.centers, d.from, centre), d.maxSlot);
    d.btn.style.transform = translate(v, d.at - d.centers[d.from]);
    d.section.forEach((b, i) => {
      if (i !== d.from) b.style.transform = translate(v, shiftFor(i, d.from, d.to, d.slot));
    });
  }

  /** Ends the drag; `commit`: save and reorder when the icon changed slot. */
  function end(commit: boolean): void {
    const d = drag;
    press = null;
    if (!d) return;
    drag = null;
    if (!commit) swallowNextUp = true;
    const v = deps.vertical();
    // The first unpinned app dropped in its own slot still commits: it becomes the last pin.
    const moved = commit && (d.to !== d.from || d.unpinned);
    if (d.btn.hasPointerCapture?.(d.pointerId)) d.btn.releasePointerCapture?.(d.pointerId);

    if (!moved) {
      // Everything glides back to rest.
      settle();
      dock.classList.remove("reordering");
      d.btn.classList.remove("dragging");
      for (const b of d.section) b.style.transform = "";
    } else {
      const ids = d.section.map((b) => b.dataset.id ?? "");
      const beforeId = beforeIdAfterDrop(ids, d.from, d.to);
      // Others are already drawn at their final slots: drop their shift and move the
      // node in the same frame, without transition, so nothing flashes.
      dock.classList.remove("reordering");
      for (const b of d.section) if (b !== d.btn) b.style.transform = "";
      const before = beforeId ? d.section.find((b) => b.dataset.id === beforeId) : undefined;
      if (before) before.before(d.btn);
      else d.section[d.section.length - 1].after(d.btn);
      // The dragged icon sits where it was released: glide it into its new slot.
      d.btn.style.transform = translate(v, d.at - d.centers[d.to]);
      void d.btn.offsetWidth;
      settle();
      d.btn.classList.remove("dragging");
      d.btn.style.transform = "";
      deps.save(d.btn.dataset.id ?? "", beforeId);
    }
    deps.onDragEnd?.(moved);
  }

  function onDown(e: PointerEvent): void {
    if (drag) return;
    press = null;
    swallowNextUp = false;
    if (e.button !== 0) return;
    const btn = (e.target as Element).closest<HTMLElement>(".island.apps > .app");
    if (!btn || !dock.contains(btn)) return;
    press = { btn, pointerId: idOf(e), x: e.clientX, y: e.clientY };
  }

  function onMove(e: PointerEvent): void {
    if (drag) {
      if (idOf(e) === drag.pointerId) update(mainOf(e));
      return;
    }
    if (!press || idOf(e) !== press.pointerId) return;
    if ((e.buttons & 1) === 0) {
      press = null;
      return;
    }
    if (Math.abs(e.clientX - press.x) <= DRAG_THRESHOLD_PX && Math.abs(e.clientY - press.y) <= DRAG_THRESHOLD_PX) return;
    begin(press);
    if (drag) update(mainOf(e));
  }

  function onUp(e: PointerEvent): void {
    if (drag && idOf(e) === drag.pointerId) {
      end(true);
      startSwallow();
    } else {
      press = null;
      if (swallowNextUp) {
        swallowNextUp = false;
        startSwallow();
      }
    }
  }

  function onCancel(e: Event): void {
    if (!drag || idOf(e) === drag.pointerId) end(false);
  }

  function onKey(e: KeyboardEvent): void {
    if (e.key === "Escape" && (drag || press)) end(false);
  }

  const onUpL = onUp as EventListener;
  const onCancelL = onCancel as EventListener;
  dock.addEventListener("pointerdown", onDown as EventListener);
  dock.addEventListener("pointermove", onMove as EventListener);
  dock.addEventListener("pointerup", onUpL);
  dock.addEventListener("pointercancel", onCancelL);
  // Fires after pointerup on a normal drop (drag already ended): only cancels otherwise.
  dock.addEventListener("lostpointercapture", onCancelL);
  doc.addEventListener("keydown", onKey);

  return {
    dragging: () => drag !== null,
    cancel: () => end(false),
    detach: () => {
      end(false);
      stopSwallow();
      clearTimeout(settleTimer);
      dock.removeEventListener("pointerdown", onDown as EventListener);
      dock.removeEventListener("pointermove", onMove as EventListener);
      dock.removeEventListener("pointerup", onUpL);
      dock.removeEventListener("pointercancel", onCancelL);
      dock.removeEventListener("lostpointercapture", onCancelL);
      doc.removeEventListener("keydown", onKey);
    },
  };
}

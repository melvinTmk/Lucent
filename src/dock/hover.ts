// Hover previews (macOS-like): the pointer resting on a running app's icon opens its
// thumbnails popup; moving to another running icon switches it at once; leaving both the
// icon and the popup closes it after a short grace. Pure state machine over timers: the
// page feeds pointer facts, `open` / `close` reach Rust (`dock_hover`, `thumbs_close`).

/** Rest on a running icon this long before the popup opens. */
export const HOVER_DWELL_MS = 450;
/** Out of both the icon and the popup this long before it closes. */
export const HOVER_GRACE_MS = 250;

export interface HoverDeps {
  open(itemId: string): void;
  close(): void;
}

export interface HoverPreview {
  /** Pointer now over running icon `id`, or over nothing that previews (null). */
  icon(id: string | null): void;
  /** Pointer entered / left the popup (relayed by Rust from the thumbs page). */
  popup(inside: boolean): void;
  /** Click on icon `id`: `opensPopup` when Rust opens the popup for it (several windows);
   *  otherwise Rust focuses / launches and closes it: no preview until the pointer leaves. */
  click(id: string, opensPopup: boolean): void;
  /** Rust closed the popup (Escape, outside click, card picked, dock hidden). */
  closed(): void;
  /** Drag, Game Mode, hidden dock: no preview, an open one closes. */
  setBlocked(blocked: boolean): void;
  /** Item whose popup is open (as far as this machine knows), or null. */
  openId(): string | null;
}

export function createHoverPreview(deps: HoverDeps): HoverPreview {
  let hovered: string | null = null;
  let onPopup = false;
  let openId: string | null = null;
  /** Icon that must be left before it previews again (after a click or a close over it). */
  let muted: string | null = null;
  let blocked = false;
  let dwell: ReturnType<typeof setTimeout> | null = null;
  let grace: ReturnType<typeof setTimeout> | null = null;

  const stopDwell = (): void => {
    if (dwell !== null) clearTimeout(dwell);
    dwell = null;
  };
  const stopGrace = (): void => {
    if (grace !== null) clearTimeout(grace);
    grace = null;
  };
  const doOpen = (id: string): void => {
    stopDwell();
    stopGrace();
    openId = id;
    deps.open(id);
  };
  const doClose = (): void => {
    stopDwell();
    stopGrace();
    if (openId === null) return;
    openId = null;
    deps.close();
  };
  const startGrace = (): void => {
    if (grace !== null) return;
    grace = setTimeout(() => {
      grace = null;
      doClose();
    }, HOVER_GRACE_MS);
  };
  const startDwell = (id: string): void => {
    stopDwell();
    dwell = setTimeout(() => {
      dwell = null;
      if (!blocked && openId === null && hovered === id) doOpen(id);
    }, HOVER_DWELL_MS);
  };
  /** Re-evaluates what the current pointer facts mean. */
  const update = (): void => {
    if (blocked) return;
    if (openId !== null) {
      if (hovered !== null && hovered !== openId && hovered !== muted) doOpen(hovered);
      else if (hovered === openId || onPopup) stopGrace();
      else startGrace();
    } else if (hovered !== null && hovered !== muted) {
      if (dwell === null) startDwell(hovered);
    } else stopDwell();
  };

  return {
    icon(id) {
      if (id === hovered) return;
      hovered = id;
      if (muted !== null && id !== muted) muted = null;
      stopDwell();
      update();
    },
    popup(inside) {
      onPopup = inside;
      update();
    },
    click(id, opensPopup) {
      stopDwell();
      stopGrace();
      hovered = id;
      if (opensPopup) {
        muted = null;
        openId = id;
      } else {
        // Rust closes the popup itself (`dock_click`).
        openId = null;
        muted = id;
      }
    },
    closed() {
      stopGrace();
      stopDwell();
      if (openId === null) return;
      openId = null;
      onPopup = false;
      muted = hovered;
    },
    setBlocked(b) {
      if (b === blocked) return;
      blocked = b;
      if (b) {
        doClose();
        // Re-entered from scratch once unblocked.
        hovered = null;
        muted = null;
      } else update();
    },
    openId: () => openId,
  };
}

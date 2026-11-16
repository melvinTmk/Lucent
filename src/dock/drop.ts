// Pure logic for dropping OS files on dock icons: position -> item, spring-loading timer.

export interface ItemRect {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Id of the icon under (x, y) (same coordinate space as the rects), or null. */
export function hitItem(rects: ItemRect[], x: number, y: number): string | null {
  for (const r of rects) {
    if (x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h) return r.id;
  }
  return null;
}

/** Hovering this long on a running app brings its window forward. */
export const SPRING_DELAY_MS = 700;

export interface SpringState {
  target: string | null;
  since: number;
  fired: boolean;
}

export const SPRING_IDLE: SpringState = { target: null, since: 0, fired: false };

/**
 * One tick of the spring-loading timer. `hover` is the icon under the drag (null: none),
 * `running` tells whether that app has windows. Returns the new state and the item to
 * bring forward (at most once per continuous hover).
 */
export function stepSpring(
  s: SpringState,
  hover: string | null,
  running: boolean,
  now: number,
  delay = SPRING_DELAY_MS,
): { state: SpringState; fire: string | null } {
  if (hover === null) return { state: SPRING_IDLE, fire: null };
  if (hover !== s.target) return { state: { target: hover, since: now, fired: false }, fire: null };
  if (!running || s.fired || now - s.since < delay) return { state: s, fire: null };
  return { state: { ...s, fired: true }, fire: hover };
}

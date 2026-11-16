// macOS-style magnification curve (pure).

/**
 * Scale of an icon whose rest centre is `distancePx` away from the cursor (main axis).
 * Influence range = 3 × iconSize; inside it `1 + (maxScale-1)·cos²(π/2 · d/range)`.
 */
export function iconScale(distancePx: number, iconSize: number, maxScale: number): number {
  const range = 3 * iconSize;
  const d = Math.abs(distancePx);
  if (maxScale <= 1 || range <= 0 || d >= range) return 1;
  const c = Math.cos((Math.PI / 2) * (d / range));
  return 1 + (maxScale - 1) * c * c;
}

/** Extra main-axis length of a row of icons (rest centres) for a cursor at `cursor`. */
export function rowGrowth(centers: number[], cursor: number, iconSize: number, maxScale: number): number {
  let g = 0;
  for (const c of centers) g += (iconScale(cursor - c, iconSize, maxScale) - 1) * iconSize;
  return g;
}

/** Largest `rowGrowth` over every cursor position along the row (sampled every 2 px). */
export function maxRowGrowth(centers: number[], iconSize: number, maxScale: number): number {
  if (centers.length === 0 || maxScale <= 1) return 0;
  const lo = Math.min(...centers);
  const hi = Math.max(...centers);
  let best = 0;
  for (let x = lo; x <= hi; x += 2) best = Math.max(best, rowGrowth(centers, x, iconSize, maxScale));
  return Math.max(best, rowGrowth(centers, hi, iconSize, maxScale));
}

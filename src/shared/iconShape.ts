// Icon silhouette detection shared by the dock and the search results: full-bleed square
// icons get Apple's rounded-square mask, shaped icons (circles, folders) keep theirs.

/** True when the four corners of an RGBA `size`×`size` bitmap are opaque: a full-bleed
 *  square icon (vs a circle, a folder or any shape with transparent corners). */
export function isFullBleed(rgba: Uint8ClampedArray, size: number): boolean {
  const inset = Math.max(1, Math.round(size * 0.06));
  const far = size - 1 - inset;
  const alpha = (x: number, y: number) => rgba[(y * size + x) * 4 + 3];
  return [alpha(inset, inset), alpha(far, inset), alpha(inset, far), alpha(far, far)].every((a) => a > 200);
}

const SHAPE_PROBE = 32;

/** Toggles `full-bleed` on `target` from the loaded `img` (needs a CORS-readable image). */
export function classifyShape(img: HTMLImageElement, target: HTMLElement): void {
  try {
    const c = document.createElement("canvas");
    c.width = c.height = SHAPE_PROBE;
    const ctx = c.getContext("2d", { willReadFrequently: true });
    if (!ctx) return;
    ctx.drawImage(img, 0, 0, SHAPE_PROBE, SHAPE_PROBE);
    target.classList.toggle("full-bleed", isFullBleed(ctx.getImageData(0, 0, SHAPE_PROBE, SHAPE_PROBE).data, SHAPE_PROBE));
  } catch {
    // Tainted canvas (no CORS header): keep the icon as it is.
  }
}

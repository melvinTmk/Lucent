// Custom results text colour: maps "#rrggbb" onto the ink tokens ("" = Auto, tone-driven).
const HEX = /^#[0-9a-f]{6}$/i;

export const TEXT_COLOR_VARS = ["--ink", "--ink-soft", "--tile-ink"] as const;

export function textColorVars(color: string): Record<string, string> | null {
  if (!HEX.test(color)) return null;
  const n = parseInt(color.slice(1), 16);
  const soft = `rgb(${(n >> 16) & 255} ${(n >> 8) & 255} ${n & 255} / 0.65)`;
  return { "--ink": color.toLowerCase(), "--ink-soft": soft, "--tile-ink": soft };
}

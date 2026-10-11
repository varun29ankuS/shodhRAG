/**
 * Colours for model-written plots, sketches and simulations. Models name a
 * design token, never a raw colour, so every visual follows the light and
 * dark themes. Pure module.
 */

export const VISUAL_COLORS = ['accent', 'info', 'success', 'warning', 'violet', 'error'] as const;
const EXTRA_COLORS = ['text', 'muted'] as const;
export type VisualColor = typeof VISUAL_COLORS[number] | typeof EXTRA_COLORS[number];

const CSS_TOKEN: Record<VisualColor, string> = {
  accent: 'accent-text',
  info: 'info',
  success: 'success',
  warning: 'warning',
  violet: 'violet',
  error: 'error',
  text: 'text',
  muted: 'text-muted',
};

/** The token a model named, or the `index`-th series colour when unnamed or unknown. */
export function pickColor(value: unknown, index: number): VisualColor {
  if (typeof value === 'string') {
    const v = value.trim().toLowerCase();
    if ((VISUAL_COLORS as readonly string[]).includes(v) || (EXTRA_COLORS as readonly string[]).includes(v)) return v as VisualColor;
  }
  return VISUAL_COLORS[((index % VISUAL_COLORS.length) + VISUAL_COLORS.length) % VISUAL_COLORS.length];
}

/** CSS value of a token, e.g. `var(--c-info)`. */
export function colorVar(color: VisualColor): string {
  return `var(--c-${CSS_TOKEN[color]})`;
}

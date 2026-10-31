/**
 * Settings → Appearance → Text size: how large the whole interface is drawn
 * (the windows' zoom, applied by the backend on top of the operating system's
 * display scaling; `TextSize` in app_settings.rs has the same table). Ctrl + /
 * Ctrl − step through the sizes and Ctrl 0 goes back to Default.
 *
 * Pure (no imports) so it is unit-tested directly with Node (`app/tests/textSize.test.ts`).
 */

export const TEXT_SIZES = ['small', 'default', 'large', 'larger'] as const;

export type TextSize = typeof TEXT_SIZES[number];

export const DEFAULT_TEXT_SIZE: TextSize = 'default';

export const TEXT_SIZE_LABELS: Record<TextSize, string> = {
  small: 'Small',
  default: 'Default',
  large: 'Large',
  larger: 'Larger',
};

/** The windows' zoom for each size. */
export const TEXT_SIZE_ZOOM: Record<TextSize, number> = {
  small: 0.9,
  default: 1,
  large: 1.15,
  larger: 1.3,
};

export function isTextSize(value: unknown): value is TextSize {
  return typeof value === 'string' && (TEXT_SIZES as readonly string[]).includes(value);
}

/** A saved size; anything else (settings from before the setting existed) is Default. */
export function parseTextSize(value: unknown): TextSize {
  return isTextSize(value) ? value : DEFAULT_TEXT_SIZE;
}

/**
 * The size after a shortcut: one step larger or smaller, stopping at Small and Larger
 * (a press past either end changes nothing), or back to Default.
 */
export function stepTextSize(current: TextSize, step: 'larger' | 'smaller' | 'reset'): TextSize {
  if (step === 'reset') return DEFAULT_TEXT_SIZE;
  const i = TEXT_SIZES.indexOf(current);
  const next = step === 'larger' ? Math.min(TEXT_SIZES.length - 1, i + 1) : Math.max(0, i - 1);
  return TEXT_SIZES[next];
}

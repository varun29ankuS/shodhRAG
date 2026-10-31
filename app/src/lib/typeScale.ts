/**
 * The type scale: every font size the app uses below heading sizes, as Tailwind
 * classes (`text-caption`, `text-small`, `text-body`, `text-body-lg`, set in
 * tailwind.config.js). Nothing is smaller than 12px; secondary text is told apart
 * by colour and weight, not by a smaller size. The whole interface scales with
 * Settings → Appearance → Text size (`textSize.ts`).
 *
 * No imports: unit-tested under Node (`app/tests/typeScale.test.ts`).
 */
export const TYPE_SCALE = {
  /** Captions, badges, counters, key hints, status lines. */
  caption: '12px',
  /** Secondary text, menus, buttons. */
  small: '13px',
  /** Body text. */
  body: '14px',
  /** Lead text, prominent labels. */
  'body-lg': '15px',
} as const;

export type TypeScaleName = keyof typeof TYPE_SCALE;

export const TYPE_SCALE_NAMES = Object.keys(TYPE_SCALE) as TypeScaleName[];

/** The smallest font size anywhere in the app, in CSS pixels. */
export const MIN_FONT_PX = 12;

/**
 * The type scale: every font size the app's classes use, as Tailwind classes
 * (`text-caption` … `text-display`, set in tailwind.config.js). Eight sizes, each
 * with a name and a job; no class sets a size outside them (`app/tests/typeScale.test.ts`).
 * Nothing is smaller than 12px; secondary text is told apart by colour and weight,
 * not by a smaller size. The whole interface scales with Settings → Appearance →
 * Text size (`textSize.ts`).
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
  /** An answer's prose: the one long text the app is read in. */
  reading: '16px',
  /** A section's heading inside a page; a dialog's title. */
  subhead: '17px',
  /** A page's title (`PageFrame`). */
  title: '20px',
  /** The one line a nearly empty screen opens with (a new chat). */
  display: '28px',
} as const;

export type TypeScaleName = keyof typeof TYPE_SCALE;

export const TYPE_SCALE_NAMES = Object.keys(TYPE_SCALE) as TypeScaleName[];

/** The smallest font size anywhere in the app, in CSS pixels. */
export const MIN_FONT_PX = 12;

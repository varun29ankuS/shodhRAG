/**
 * Spacing: Tailwind's own scale (steps of 4px, with 2px and 6px half-steps and `px`), and
 * the two measures below, set in `tailwind.config.js`. No class writes a padding, margin
 * or gap in pixels of its own (`app/tests/uiStandard.test.ts`).
 *
 * No imports: the Node tests load it.
 */
export const EXTRA_SPACING = {
  /** The composer's inset: its text, its chips and the sidebar's brand row share it. */
  '4.5': '18px',
  /**
   * The end of a page: the 28px every page ends with, and the 56px kept clear for the
   * floating chat's pill (`components/shell/frameMeasures.ts`).
   */
  'page-end': '84px',
} as const;

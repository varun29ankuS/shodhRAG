/**
 * Motion tokens for framer-motion, mirroring the CSS custom properties in
 * src/index.css (`--dur-*`, `--ease-*`). `app/tests/motion.test.ts` fails if
 * the two drift. Reduced motion is handled once, at the root, by
 * `<MotionConfig reducedMotion="user">` (src/main.tsx); CSS durations
 * collapse to 0ms under prefers-reduced-motion.
 */

export type CubicBezier = readonly [number, number, number, number];

/** Durations in seconds. */
export const DURATION = {
  micro: 0.12,
  panel: 0.2,
  screen: 0.28,
  exit: 0.14,
} as const;

export const EASE = {
  standard: [0.2, 0.7, 0.2, 1] as CubicBezier,
  enter: [0, 0, 0.2, 1] as CubicBezier,
  exit: [0.4, 0, 1, 1] as CubicBezier,
} as const;

/** framer-motion transition for content appearing. */
export const ENTER_TRANSITION = { duration: DURATION.panel, ease: EASE.enter } as const;

/** framer-motion transition for content leaving. */
export const EXIT_TRANSITION = { duration: DURATION.exit, ease: EASE.exit } as const;

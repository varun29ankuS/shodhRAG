/**
 * The app's motion: springs (physics, interruptible, carrying the finger's
 * velocity), and only `transform` and `opacity`, which the phone's GPU
 * composites without laying the page out again. Every animated value of the
 * app is one of these presets or a flat `x`/`y`/`scale`/`opacity` value
 * (tests/motion.test.ts checks both). With the phone's "Remove animations"
 * on, MotionConfig's `reducedMotion="user"` drops the movement and keeps a
 * short cross-fade; nothing waits for an animation to show information.
 */
import type { Variants } from 'framer-motion';

/** Quick and firm: presses, chips, the tab indicator. */
export const snappy = { type: 'spring', stiffness: 520, damping: 34, mass: 0.8 } as const;

/** Screens and sheets moving in. */
export const glide = { type: 'spring', stiffness: 340, damping: 32, mass: 1 } as const;

/** A soft settle with a little overshoot: sources fanning in, the splash's elephant. */
export const bouncy = { type: 'spring', stiffness: 260, damping: 18, mass: 0.9 } as const;

/** A screen of a section appearing. */
export const viewIn: Variants = {
  hidden: { opacity: 0, y: 12 },
  shown: { opacity: 1, y: 0, transition: glide },
  gone: { opacity: 0, transition: { duration: 0.12 } },
};

/** A screen pushed on top of another (a conversation, a folder). */
export const pushIn: Variants = {
  hidden: { opacity: 0, x: 48 },
  shown: { opacity: 1, x: 0, transition: glide },
  gone: { opacity: 0, x: 24, transition: { duration: 0.14 } },
};

/** A citation chip arriving with its source. */
export const popIn: Variants = {
  hidden: { opacity: 0, scale: 0.3 },
  shown: { opacity: 1, scale: 1, transition: bouncy },
};

/** Source pages fanning in, one after another. */
export const fanIn: Variants = {
  hidden: { opacity: 0, y: 44, rotate: 7, scale: 0.88 },
  shown: (i: number) => ({ opacity: 1, y: 0, rotate: 0, scale: 1, transition: { ...bouncy, delay: i * 0.11 } }),
};

/** The line drawn under a finished answer. */
export const sweep: Variants = {
  hidden: { opacity: 1, scaleX: 0 },
  shown: { opacity: 0.4, scaleX: 1, transition: { type: 'spring', stiffness: 120, damping: 26 } },
};

/** A note sliding down from the top. */
export const noteIn: Variants = {
  hidden: { opacity: 0, y: -30, scale: 0.9 },
  shown: { opacity: 1, y: 0, scale: 1, transition: bouncy },
  gone: { opacity: 0, y: -16, transition: { duration: 0.16 } },
};

/** The dimmed page behind the menu and sheets. */
export const fade: Variants = {
  hidden: { opacity: 0 },
  shown: { opacity: 1, transition: { duration: 0.2 } },
  gone: { opacity: 0, transition: { duration: 0.16 } },
};

/** Pressed controls shrink a little. */
export const press = { scale: 0.96 } as const;

/** Every preset, for the motion test. */
export const PRESETS: Record<string, Variants> = { viewIn, pushIn, popIn, fanIn, sweep, noteIn, fade };

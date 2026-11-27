/**
 * Focus, where a ring does not fit: a text field drawn without a box of its own (a search
 * field inside a framed bar, a title edited in place) shows keyboard focus as a 2px line
 * under its text, in the focus ring's colour (`--c-accent-text`, whose contrast on every
 * surface `app/tests/contrast.test.ts` checks). Every element that removes the browser's
 * outline shows focus some way (`app/tests/uiStandard.test.ts`).
 *
 * No imports: the Node tests load it.
 */
export const FOCUS_UNDERLINE = 'focus-visible:shadow-[inset_0_-2px_0_var(--c-accent-text)]';

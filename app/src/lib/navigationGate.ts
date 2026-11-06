/**
 * The assistant never pulls the user away while they type.
 *
 * Every view change the assistant asks for (its `navigated` events, and a
 * Creator build opening the Artifacts tab) is offered here first. When the user
 * has typed into a text field in the last moments, the navigation is held
 * instead of followed: the shell shows "The assistant opened X · Go there", and
 * nothing moves until the user clicks it. A newer navigation replaces the held
 * one (the chip always names where the assistant went last).
 *
 * "Typing" is keyed on recent input, not on focus alone: after sending a
 * message the composer keeps the focus, and a focus-only rule would hold every
 * navigation the assistant ever makes.
 *
 * No imports: unit-tested under Node (`app/tests/navigationGate.test.ts`).
 */

/** How recently the user must have typed for a navigation to be held. */
export const TYPING_QUIET_MS = 2000;

/** The parts of a DOM element the typing check reads (structural, so tests need no DOM). */
export interface EditableLike {
  tagName?: string;
  type?: string;
  readOnly?: boolean;
  disabled?: boolean;
  isContentEditable?: boolean;
}

/** Input types that take typed text. */
const TEXT_INPUT_TYPES: readonly string[] = ['', 'text', 'search', 'email', 'url', 'tel', 'password', 'number'];

/** Whether `element` is a field the user types text into. */
export function isEditable(element: EditableLike | null | undefined): boolean {
  if (!element) return false;
  if (element.isContentEditable) return true;
  if (element.readOnly || element.disabled) return false;
  const tag = (element.tagName ?? '').toUpperCase();
  if (tag === 'TEXTAREA') return true;
  if (tag === 'INPUT') return TEXT_INPUT_TYPES.includes((element.type ?? '').toLowerCase());
  return false;
}

/** When the user last typed into a text field. */
export interface TypingTracker {
  /** An `input` event on `target` at `now`. */
  note(target: EditableLike | null | undefined, now: number): void;
  /** Whether the user is typing now: a text field has the focus and was typed into recently. */
  isTyping(focused: EditableLike | null | undefined, now: number): boolean;
}

export function createTypingTracker(quietMs: number = TYPING_QUIET_MS): TypingTracker {
  let lastInputAt: number | null = null;
  return {
    note(target, now) {
      if (isEditable(target)) lastInputAt = now;
    },
    isTyping(focused, now) {
      return isEditable(focused) && lastInputAt !== null && now - lastInputAt < quietMs;
    },
  };
}

/** A navigation the gate holds or lets through. */
export interface GateState<N> {
  /** The navigation waiting for the user's "Go there", if any. */
  held: N | null;
}

export interface GateResult<N> {
  state: GateState<N>;
  /** The navigation to follow now, if any. */
  follow: N | null;
}

export function initialGate<N>(): GateState<N> {
  return { held: null };
}

/**
 * The assistant asked for `nav`: follow it, or hold it while the user types.
 * Following one drops any older held navigation: the view the chip named is no
 * longer where the assistant is.
 */
export function offerNavigation<N>(_state: GateState<N>, nav: N, typing: boolean): GateResult<N> {
  if (typing) return { state: { held: nav }, follow: null };
  return { state: { held: null }, follow: nav };
}

/** The user clicked "Go there": follow the held navigation. */
export function goThere<N>(state: GateState<N>): GateResult<N> {
  return { state: { held: null }, follow: state.held };
}

/** The user dismissed the chip: nothing is followed. */
export function dismissHeld<N>(_state: GateState<N>): GateState<N> {
  return { held: null };
}

/** What the chip says (the button reads "Go there"). */
export function heldNavigationText(label: string): string {
  return `The assistant opened ${label}`;
}

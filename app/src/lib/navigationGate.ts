/**
 * How a move of the assistant reaches the screen, without pulling the user
 * away from what they are doing.
 *
 * 1. The coalescer: a burst of moves ("plan my week": eight `create_task`
 *    calls in twenty seconds) becomes one. The first move of a quiet spell goes
 *    at once; later ones within `COALESCE_MS` wait, the last one wins, and the
 *    view switches at most once per `SWITCH_GAP_MS`. What was folded in is
 *    counted, so the shell can say "Shodh has 8 things to show you".
 * 2. The gate: while the user is typing, a move is held instead of followed,
 *    and the shell shows "Shodh has … to show you in Tasks · Go there". A held
 *    move publishes nothing: no view, item or conversation changes until the
 *    user clicks it. A newer move replaces the held one (counts add up). It is
 *    dropped on a conversation switch, a new message, or when the user opens
 *    that view themselves; it stays after the run ends, when it is most useful.
 *
 * "Typing" means the focused text field holds a draft, or dictation is
 * listening, or the user typed in the last `TYPING_QUIET_MS`. Sending resets
 * it: the composer keeps the focus after a message is sent, and an empty
 * composer must not hold every move.
 *
 * No imports: unit-tested under Node (`app/tests/navigationGate.test.ts`).
 */

/** How recently the user must have typed for a move to be held (besides a draft or dictation). */
export const TYPING_QUIET_MS = 2000;
/** How long a move waits for a later one of the same burst. */
export const COALESCE_MS = 1500;
/** Shortest time between two view switches the assistant makes. */
export const SWITCH_GAP_MS = 4000;

/** The parts of a DOM element the typing check reads (structural, so tests need no DOM). */
export interface EditableLike {
  tagName?: string;
  type?: string;
  readOnly?: boolean;
  disabled?: boolean;
  isContentEditable?: boolean;
  /** An input's or textarea's text. */
  value?: string;
  /** A contenteditable's text. */
  textContent?: string | null;
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

/** Whether `element` is a text field holding a draft (any text but spaces). */
export function holdsDraft(element: EditableLike | null | undefined): boolean {
  if (!element || !isEditable(element)) return false;
  const text = element.isContentEditable ? element.textContent ?? '' : element.value ?? '';
  return text.trim().length > 0;
}

/** What the user is doing with text, for the gate. */
export interface TypingTracker {
  /** An `input` event on `target` at `now`. */
  note(target: EditableLike | null | undefined, now: number): void;
  /** Dictation into the composer started (`true`) or stopped. */
  setDictating(on: boolean): void;
  /** A message was sent: the recent typing is spent. */
  reset(): void;
  /** Whether the user is typing now, with `focused` the focused element. */
  isTyping(focused: EditableLike | null | undefined, now: number): boolean;
}

export function createTypingTracker(quietMs: number = TYPING_QUIET_MS): TypingTracker {
  let lastInputAt: number | null = null;
  let dictating = false;
  return {
    note(target, now) {
      if (isEditable(target)) lastInputAt = now;
    },
    setDictating(on) {
      dictating = on;
    },
    reset() {
      lastInputAt = null;
    },
    isTyping(focused, now) {
      if (dictating) return true;
      if (holdsDraft(focused)) return true;
      return isEditable(focused) && lastInputAt !== null && now - lastInputAt < quietMs;
    },
  };
}

/**
 * The app's one tracker: the session notes input and sends, the composer's
 * dictation reports itself, and the gate reads it.
 */
export const typingActivity: TypingTracker = createTypingTracker();

/** A move with how many moves of a burst it stands for (itself included). */
export interface Counted<N> {
  nav: N;
  count: number;
}

// ── 1. The coalescer ──────────────────────────────────────────────────────

export interface CoalescerState<N> {
  /** When the last move went out (null: none yet). */
  lastAt: number | null;
  /** When the last move that switched the view went out. */
  lastSwitchAt: number | null;
  /** The view the last move went to. */
  lastView: string | null;
  /** A move waiting for later ones (the last wins), with its view. */
  pending: (Counted<N> & { view: string | null }) | null;
  /** When the waiting move goes out. */
  dueAt: number | null;
}

export function initialCoalescer<N>(): CoalescerState<N> {
  return { lastAt: null, lastSwitchAt: null, lastView: null, pending: null, dueAt: null };
}

export interface CoalescerResult<N> {
  state: CoalescerState<N>;
  /** The move to hand to the gate now, if any. */
  emit: Counted<N> | null;
}

function sent<N>(state: CoalescerState<N>, view: string | null, now: number): CoalescerState<N> {
  const switched = view !== null && view !== state.lastView;
  return {
    lastAt: now,
    lastSwitchAt: switched ? now : state.lastSwitchAt,
    lastView: view ?? state.lastView,
    pending: null,
    dueAt: null,
  };
}

/**
 * A move to `view` (null: it stays on the current view, as the System status panel
 * does) arrived at `now`. The first of a quiet spell goes at once; one within
 * `COALESCE_MS` of the last, or one that would switch views sooner than
 * `SWITCH_GAP_MS` after the last switch, waits (the last wins) until `dueAt`.
 */
export function arrive<N>(state: CoalescerState<N>, nav: N, view: string | null, now: number): CoalescerResult<N> {
  const quiet = state.pending === null && (state.lastAt === null || now - state.lastAt >= COALESCE_MS);
  const switches = view !== null && view !== state.lastView;
  const switchAllowed = !switches || state.lastSwitchAt === null || now - state.lastSwitchAt >= SWITCH_GAP_MS;
  if (quiet && switchAllowed) {
    return { state: sent(state, view, now), emit: { nav, count: 1 } };
  }
  const count = (state.pending?.count ?? 0) + 1;
  const debounce = now + COALESCE_MS;
  const gap = switches && state.lastSwitchAt !== null ? state.lastSwitchAt + SWITCH_GAP_MS : 0;
  return {
    state: { ...state, pending: { nav, count, view }, dueAt: Math.max(debounce, gap) },
    emit: null,
  };
}

/** The waiting move, once it is due at `now`. */
export function flush<N>(state: CoalescerState<N>, now: number): CoalescerResult<N> {
  if (!state.pending || state.dueAt === null || now < state.dueAt) return { state, emit: null };
  const { nav, count, view } = state.pending;
  return { state: sent(state, view, now), emit: { nav, count } };
}

// ── 2. The gate ───────────────────────────────────────────────────────────

export interface GateState<N> {
  /** The move waiting for the user's "Go there", with how many moves it stands for. */
  held: Counted<N> | null;
}

export interface GateResult<N> {
  state: GateState<N>;
  /** The move to follow now, if any. */
  follow: N | null;
  /** The chip just appeared (announce it once; later updates are silent). */
  appeared: boolean;
}

export function initialGate<N>(): GateState<N> {
  return { held: null };
}

/**
 * The coalescer handed over `move`: follow it, or hold it while the user types.
 * Following drops an older held move (the chip no longer says where the assistant is).
 */
export function offerNavigation<N>(state: GateState<N>, move: Counted<N>, typing: boolean): GateResult<N> {
  if (typing) {
    const count = (state.held?.count ?? 0) + move.count;
    return { state: { held: { nav: move.nav, count } }, follow: null, appeared: state.held === null };
  }
  return { state: { held: null }, follow: move.nav, appeared: false };
}

/** The user clicked "Go there": follow the held move, once. */
export function goThere<N>(state: GateState<N>): GateResult<N> {
  return { state: { held: null }, follow: state.held?.nav ?? null, appeared: false };
}

/** The chip goes without following: dismissed, a conversation switch, or a new message. */
export function dropHeld<N>(_state: GateState<N>): GateState<N> {
  return { held: null };
}

/** The user opened `view` themselves: a move held for that view is spent. */
export function viewOpened<N extends { view: string | null }>(state: GateState<N>, view: string): GateState<N> {
  return state.held && state.held.nav.view === view ? { held: null } : state;
}

/**
 * What the chip says before its "Go there" button: "Shodh has “Renew lease” to show
 * you in Tasks", "Shodh has something to show you in Library", or for a burst
 * "Shodh has 8 things to show you in Tasks". `where` is null for the System status
 * panel, which opens over the current view.
 */
export function heldNavigationText(where: string | null, label: string | null, count: number): string {
  const what = count > 1 ? `${count} things` : label ? `“${label}”` : 'something';
  return where ? `Shodh has ${what} to show you in ${where}` : `Shodh has ${what} to show you`;
}

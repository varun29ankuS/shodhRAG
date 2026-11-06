/**
 * "The assistant is here": the one highlight every view uses for an item the
 * assistant opened, changed or pointed at (a task, a Library card, a setting, a
 * memory, a snippet, a visual, an Activity entry).
 *
 * An item marks itself with `data-agent-item={agentItemKey(kind, id)}`; the view
 * that owns a navigation target calls `revealAgentItem` with that key. The item
 * is scrolled into view (instantly under reduced motion) and outlined for a few
 * seconds by the `data-assistant-here` attribute (`index.css`: a soft pulse, a
 * still outline under reduced motion). The attribute is one React never
 * renders, so a re-render of the item (its `className` changing as it becomes
 * the selected card) cannot wipe the highlight. The keyboard focus never moves.
 *
 * The item may render a moment after the navigation (its view mounts, its list
 * loads), so the reveal waits for it, for a bounded time. One reveal at a time:
 * a newer one cancels the older, and the user's own scrolling, clicking or
 * typing cancels a reveal still waiting, so nothing scrolls under their hands.
 * What happened is said through the shell's live region: the item found, or,
 * when it never rendered, that it is not shown.
 */
import { VIEW_CUE_MS } from './viewCue.ts';
import { announce } from './announcer.ts';
import { scrollBehavior } from '../features/ask/viewer/sourceAccess';

/** The attribute that draws the highlight (never rendered by React). */
export const ASSISTANT_HERE_ATTRIBUTE = 'data-assistant-here';

/** How long the highlight stays (as long as the "opened by Shodh" cue). */
export const ASSISTANT_HERE_MS = VIEW_CUE_MS;

/** How long a reveal waits for its item to render. */
export const REVEAL_WAIT_MS = 4000;

/** The `data-agent-item` value of item `id` of `kind` ("task", "memory", "setting", …). */
export function agentItemKey(kind: string, id: string): string {
  return `${kind}:${id}`;
}

/** The element marked `key`, if it is rendered. */
function findItem(key: string): HTMLElement | null {
  if (typeof document === 'undefined') return null;
  for (const element of document.querySelectorAll<HTMLElement>('[data-agent-item]')) {
    if (element.dataset.agentItem === key) return element;
  }
  return null;
}

const timers = new WeakMap<HTMLElement, number>();

/** Outline `element` as the assistant's, scrolled into view; the focus stays where it is. */
export function highlightElement(element: HTMLElement): void {
  element.scrollIntoView({ block: 'center', behavior: scrollBehavior() });
  // Restart the highlight when the assistant points at the same item again.
  element.removeAttribute(ASSISTANT_HERE_ATTRIBUTE);
  void element.offsetWidth;
  element.setAttribute(ASSISTANT_HERE_ATTRIBUTE, '');
  const previous = timers.get(element);
  if (previous !== undefined) window.clearTimeout(previous);
  timers.set(element, window.setTimeout(() => {
    element.removeAttribute(ASSISTANT_HERE_ATTRIBUTE);
    timers.delete(element);
  }, ASSISTANT_HERE_MS));
}

export interface RevealWords {
  /** Said when the item is highlighted, e.g. "Shodh highlighted Passages per search in Settings". */
  found?: string;
  /** Said when the item never rendered, e.g. "Shodh pointed at a memory that isn't shown". */
  missed?: string;
}

/** The reveal waiting now, if any. */
let inFlight: (() => void) | null = null;

const USER_TAKES_OVER = ['wheel', 'pointerdown', 'keydown'] as const;

/**
 * Scroll to and highlight the item marked `key` once it is rendered (waiting up to
 * `REVEAL_WAIT_MS`), saying what happened. Cancels a reveal still waiting. Returns
 * a function that stops waiting.
 */
export function revealAgentItem(key: string, words: RevealWords = {}): () => void {
  if (typeof window === 'undefined') return () => {};
  inFlight?.();
  const deadline = Date.now() + REVEAL_WAIT_MS;
  let frame = 0;
  let done = false;
  const stop = () => {
    if (done) return;
    done = true;
    window.cancelAnimationFrame(frame);
    for (const type of USER_TAKES_OVER) window.removeEventListener(type, stop, true);
    if (inFlight === stop) inFlight = null;
  };
  // The user takes over: a reveal still waiting never scrolls under their hands.
  for (const type of USER_TAKES_OVER) window.addEventListener(type, stop, true);
  inFlight = stop;
  const attempt = () => {
    if (done) return;
    const element = findItem(key);
    if (element) {
      stop();
      highlightElement(element);
      if (words.found) announce(words.found);
      return;
    }
    if (Date.now() < deadline) {
      frame = window.requestAnimationFrame(attempt);
    } else {
      stop();
      if (words.missed) announce(words.missed);
    }
  };
  attempt();
  return stop;
}

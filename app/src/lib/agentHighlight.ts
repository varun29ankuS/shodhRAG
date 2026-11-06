/**
 * "The assistant is here": the one highlight every view uses for an item the
 * assistant opened, changed or pointed at (a task, a Library card, a setting, a
 * memory, a snippet, an Activity entry).
 *
 * An item marks itself with `data-agent-item={agentItemKey(kind, id)}`; the view
 * that owns a navigation target calls `revealAgentItem` with that key. The item
 * is scrolled into view (instantly under reduced motion) and outlined for a few
 * seconds with the `assistant-here` class (a soft pulse; a still outline under
 * reduced motion, `index.css`). The keyboard focus never moves: the user keeps
 * typing or reading where they were.
 *
 * The item may render a moment after the navigation (its view mounts, its list
 * loads), so the reveal waits for it, for a bounded time.
 */
import { VIEW_CUE_MS } from './viewCue.ts';

export const ASSISTANT_HERE_CLASS = 'assistant-here';

/** How long the highlight stays (as long as the "opened by Shodh" cue). */
export const ASSISTANT_HERE_MS = VIEW_CUE_MS;

/** How long a reveal waits for its item to render. */
export const REVEAL_WAIT_MS = 4000;

/** The `data-agent-item` value of item `id` of `kind` ("task", "memory", "setting", …). */
export function agentItemKey(kind: string, id: string): string {
  return `${kind}:${id}`;
}

function prefersReducedMotion(): boolean {
  return typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

/** The element marked `key`, if it is rendered. */
function findItem(key: string): HTMLElement | null {
  if (typeof document === 'undefined') return null;
  for (const element of document.querySelectorAll<HTMLElement>('[data-agent-item]')) {
    if (element.dataset.agentItem === key) return element;
  }
  return null;
}

/** Outline `element` as the assistant's, scrolled into view; the focus stays where it is. */
export function highlightElement(element: HTMLElement): void {
  element.scrollIntoView({ block: 'center', behavior: prefersReducedMotion() ? 'auto' : 'smooth' });
  // Restart the highlight when the assistant points at the same item again.
  element.classList.remove(ASSISTANT_HERE_CLASS);
  void element.offsetWidth;
  element.classList.add(ASSISTANT_HERE_CLASS);
  const previous = Number(element.dataset.agentHereTimer ?? '');
  if (Number.isFinite(previous) && previous > 0) window.clearTimeout(previous);
  const timer = window.setTimeout(() => {
    element.classList.remove(ASSISTANT_HERE_CLASS);
    delete element.dataset.agentHereTimer;
  }, ASSISTANT_HERE_MS);
  element.dataset.agentHereTimer = String(timer);
}

/**
 * Scroll to and highlight the item marked `key` once it is rendered (waiting
 * up to `REVEAL_WAIT_MS`). Returns a function that stops waiting.
 */
export function revealAgentItem(key: string): () => void {
  if (typeof window === 'undefined') return () => {};
  const deadline = Date.now() + REVEAL_WAIT_MS;
  let frame = 0;
  const attempt = () => {
    const element = findItem(key);
    if (element) {
      highlightElement(element);
      return;
    }
    if (Date.now() < deadline) frame = window.requestAnimationFrame(attempt);
  };
  attempt();
  return () => window.cancelAnimationFrame(frame);
}

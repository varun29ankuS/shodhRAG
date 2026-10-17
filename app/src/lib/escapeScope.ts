/**
 * Esc that belongs to a control inside a panel rather than to the panel: a find field
 * (`role="search"`) or an element that cancels itself on Esc (`data-esc-local`: an
 * editor, the PDF viewer while it is in snippet mode). The panel's own Esc handler
 * leaves such a key to it. No imports: unit-tested under Node.
 */

export const ESC_LOCAL_SELECTOR = '[role="search"], [data-esc-local]';

interface ClosestTarget {
  closest(selector: string): unknown;
}

interface Container<T> {
  contains(node: T | null): boolean;
}

/** Whether Esc on `target` is handled by a control inside `container`. */
export function escapeStaysLocal<T extends ClosestTarget>(target: T | null, container: Container<T> | null): boolean {
  if (!target || !container || !container.contains(target)) return false;
  return target.closest(ESC_LOCAL_SELECTOR) !== null;
}

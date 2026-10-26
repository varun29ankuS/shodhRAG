/**
 * Whether a control may take focus after work the reader started finishes
 * (a redraw that took a minute): only when nothing has focus (the element
 * that had it is gone, so focus fell to the body) or focus is still inside
 * the part of the page that work belongs to. Focus the reader moved
 * elsewhere (the composer, another answer) is never taken. No imports:
 * unit-tested under Node.
 */

interface Container<T> {
  contains(node: T | null): boolean;
}

export function focusIsFree<T>(active: T | null, body: T | null, container: Container<T> | null): boolean {
  if (active === null || active === body) return true;
  return container !== null && container.contains(active);
}

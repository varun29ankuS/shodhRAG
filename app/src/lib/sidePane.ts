/**
 * The pane docked at the window's right (the design check and the code review,
 * `features/code/ReviewPane`): its width in CSS pixels while it is open, 0 when it is not.
 *
 * The pane is docked, not laid over the page: the shell reserves its width (the views
 * narrow beside it), and the embedded Artifacts preview places the floating chat left of
 * it. So a panel never covers the conversation or the work it belongs to.
 */
import { useLayoutEffect, useSyncExternalStore } from 'react';
import type { RefObject } from 'react';

let width = 0;
const listeners = new Set<() => void>();

export const sidePane = {
  get: (): number => width,
  set(next: number): void {
    const value = Number.isFinite(next) && next > 0 ? Math.round(next) : 0;
    if (value === width) return;
    width = value;
    listeners.forEach(listener => listener());
  },
  subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  },
};

/** The docked pane's width now (0: none open). */
export function useSidePaneWidth(): number {
  return useSyncExternalStore(sidePane.subscribe, sidePane.get, sidePane.get);
}

/**
 * Report the docked pane `ref` measures while it shows (`shown`: which of its forms, null when
 * closed): its width as it is laid out, and 0 once it closes or goes.
 */
export function useDockedWidth(ref: RefObject<HTMLElement | null> | null | undefined, shown: string | null): void {
  useLayoutEffect(() => {
    const element = shown !== null ? ref?.current ?? null : null;
    if (!element) {
      sidePane.set(0);
      return;
    }
    const measure = () => sidePane.set(element.getBoundingClientRect().width);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => {
      observer.disconnect();
      sidePane.set(0);
    };
  }, [ref, shown]);
}

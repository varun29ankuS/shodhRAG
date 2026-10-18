/**
 * The window's device pixel ratio, followed as the window moves between monitors
 * with different scale factors (PDF pages are drawn at it). No imports:
 * unit-tested under Node.
 */

/** The ratio to draw at: `devicePixelRatio`, or 1 when it is missing or not positive. */
export function currentPixelRatio(ratio: number | undefined): number {
  return typeof ratio === 'number' && Number.isFinite(ratio) && ratio > 0 ? ratio : 1;
}

/** A media query that stops matching once the ratio is no longer `ratio`. */
export function resolutionQuery(ratio: number): string {
  return `(resolution: ${ratio}dppx)`;
}

interface MediaQueryListLike {
  addEventListener(type: 'change', listener: () => void): void;
  removeEventListener(type: 'change', listener: () => void): void;
}

interface PixelRatioWindow {
  readonly devicePixelRatio: number;
  matchMedia(query: string): MediaQueryListLike;
}

/**
 * Calls `onChange` with the new ratio whenever it changes. The query names the
 * current ratio, so it is armed again for the new one after every change.
 * Returns the unsubscribe function.
 */
export function watchPixelRatio(win: PixelRatioWindow, onChange: (ratio: number) => void): () => void {
  let query: MediaQueryListLike | null = null;
  const arm = () => {
    query = win.matchMedia(resolutionQuery(currentPixelRatio(win.devicePixelRatio)));
    query.addEventListener('change', changed);
  };
  const changed = () => {
    query?.removeEventListener('change', changed);
    arm();
    onChange(currentPixelRatio(win.devicePixelRatio));
  };
  arm();
  return () => {
    query?.removeEventListener('change', changed);
    query = null;
  };
}

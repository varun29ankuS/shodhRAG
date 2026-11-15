/**
 * Where Android's status bar and gesture bar cover the app. The app draws
 * edge to edge, under both, and the WebView reports neither to the page
 * (env(safe-area-inset-*) is 0 in it), so the phone's own measure is written
 * to `<html>` as `--system-inset-top`, `-right`, `-bottom` and `-left`.
 * theme.css takes the larger of that and env() as `--inset-*`, which every
 * edge of the app keeps clear.
 */
import { useEffect } from 'react';

export interface Insets {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

export interface SystemInsets {
  /** The insets now. */
  current(): Promise<Insets>;
  /** Each change (a rotation, the navigation mode); resolves to a function that stops it. */
  onChanged(handler: (insets: Insets) => void): Promise<() => void>;
}

export const SIDES = ['top', 'right', 'bottom', 'left'] as const;

function isInsets(value: unknown): value is Insets {
  if (typeof value !== 'object' || value === null) return false;
  const record = value as Record<string, unknown>;
  return SIDES.every(side => typeof record[side] === 'number' && Number.isFinite(record[side]) && (record[side] as number) >= 0);
}

/** Write the insets to `root` as `--system-inset-*`; anything that is not four sizes is ignored. */
export function applyInsets(insets: unknown, root: HTMLElement = document.documentElement): void {
  if (!isInsets(insets)) return;
  for (const side of SIDES) root.style.setProperty(`--system-inset-${side}`, `${Math.round(insets[side] * 100) / 100}px`);
}

/** Keep `<html>`'s `--system-inset-*` at the phone's insets, from the start and on each change. */
export function useSystemInsets(source: SystemInsets): void {
  useEffect(() => {
    let stopped = false;
    let stop: (() => void) | null = null;
    // Listening first, then asking: a change between the two is not missed.
    source
      .onChanged(insets => {
        if (!stopped) applyInsets(insets);
      })
      .then(unlisten => {
        if (stopped) unlisten();
        else stop = unlisten;
      })
      .catch(() => undefined)
      .finally(() => {
        if (stopped) return;
        source
          .current()
          .then(insets => {
            if (!stopped) applyInsets(insets);
          })
          .catch(() => {
            // Not measured: env() and the CSS fallback of 0 stand.
          });
      });
    return () => {
      stopped = true;
      stop?.();
    };
  }, [source]);
}

import { useCallback, useEffect, useRef, useState } from 'react';
import { getAppSettings, onAppSettingsChanged, updatePreferences } from '../lib/appSettings';
import { notify } from '../lib/notify';
import { DEFAULT_TEXT_SIZE, LatestRequest, stepTextSize, textSizeStepFor } from '../lib/textSize';
import type { TextSize } from '../lib/textSize';

export interface TextSizeHandle {
  /** The size shown: the one being saved, else the saved one. */
  size: TextSize;
  /** The saved size has been read (the shortcuts wait for it). */
  loaded: boolean;
  setSize: (size: TextSize) => void;
  saving: boolean;
  error: string | null;
}

/**
 * The saved text size (Settings → Appearance). The backend's settings are the truth:
 * a size being saved is shown until its answer, and only the answer to the latest
 * save counts, so quick presses never leave the size behind what the windows show. A
 * save that fails goes back to the saved size and says why.
 */
export function useTextSize(): TextSizeHandle {
  const [saved, setSaved] = useState<TextSize | null>(null);
  const [pending, setPending] = useState<TextSize | null>(null);
  const [error, setError] = useState<string | null>(null);
  const requests = useRef(new LatestRequest());

  useEffect(() => {
    let cancelled = false;
    getAppSettings()
      .then(settings => {
        if (!cancelled && settings) setSaved(settings.preferences.textSize);
      })
      .catch(err => setError(`The text size could not be read: ${String(err)}`));
    const unsubscribe = onAppSettingsChanged(settings => setSaved(settings.preferences.textSize));
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  const setSize = useCallback((next: TextSize) => {
    const id = requests.current.next();
    setPending(next);
    setError(null);
    updatePreferences({ textSize: next })
      .then(result => {
        if (!requests.current.isLatest(id)) return;
        if (result) setSaved(result.preferences.textSize);
        setPending(null);
      })
      .catch(err => {
        if (!requests.current.isLatest(id)) return;
        // Back to what is saved (and drawn), with the reason.
        setPending(null);
        setError(`The text size was not saved: ${String(err)}`);
        notify.error('The text size was not changed', { description: String(err) });
      });
  }, []);

  return {
    size: pending ?? saved ?? DEFAULT_TEXT_SIZE,
    loaded: saved !== null,
    setSize,
    saving: pending !== null,
    error,
  };
}

/**
 * Ctrl + / Ctrl − / Ctrl 0 (⌘ on a Mac) from anywhere in the app: one size larger or
 * smaller, stopping at Small and Larger, or back to Default. Mounted once, by the app.
 * A press something else already handled (the PDF viewer's and the pop-out's own
 * zoom) is left to it, auto-repeat is ignored, and nothing happens until the saved
 * size is known.
 */
export function useTextSizeShortcuts(): void {
  const { size, loaded, setSize } = useTextSize();
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!loaded) return;
      const step = textSizeStepFor(e);
      if (!step) return;
      // The browser's own zoom never runs: the size is the app's setting.
      e.preventDefault();
      const next = stepTextSize(size, step);
      if (next !== size) setSize(next);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [size, loaded, setSize]);
}

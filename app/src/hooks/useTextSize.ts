import { useCallback, useEffect, useState } from 'react';
import { getAppSettings, onAppSettingsChanged, updatePreferences } from '../lib/appSettings';
import { matchesShortcut } from '../lib/shortcuts';
import { DEFAULT_TEXT_SIZE, stepTextSize } from '../lib/textSize';
import type { TextSize } from '../lib/textSize';

export interface TextSizeHandle {
  size: TextSize;
  setSize: (size: TextSize) => void;
  saving: boolean;
  error: string | null;
}

/**
 * The saved text size (Settings → Appearance). Saving it makes the backend draw
 * every window at that size; every view reads the change from the settings event.
 */
export function useTextSize(): TextSizeHandle {
  const [size, setSizeState] = useState<TextSize>(DEFAULT_TEXT_SIZE);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    getAppSettings()
      .then(settings => {
        if (!cancelled && settings) setSizeState(settings.preferences.textSize);
      })
      .catch(err => setError(`The text size could not be read: ${String(err)}`));
    const unsubscribe = onAppSettingsChanged(settings => setSizeState(settings.preferences.textSize));
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  const setSize = useCallback((next: TextSize) => {
    setSizeState(next);
    setSaving(true);
    setError(null);
    updatePreferences({ textSize: next })
      .then(saved => {
        if (saved) setSizeState(saved.preferences.textSize);
      })
      .catch(err => setError(`The text size was not saved: ${String(err)}`))
      .finally(() => setSaving(false));
  }, []);

  return { size, setSize, saving, error };
}

/**
 * Ctrl + / Ctrl − / Ctrl 0 (⌘ on a Mac) from anywhere in the app: one size larger or
 * smaller, stopping at Small and Larger, or back to Default. Mounted once, by the app.
 */
export function useTextSizeShortcuts(): void {
  const { size, setSize } = useTextSize();
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const step = matchesShortcut(e, 'textLarger')
        ? 'larger'
        : matchesShortcut(e, 'textSmaller')
          ? 'smaller'
          : matchesShortcut(e, 'textReset')
            ? 'reset'
            : null;
      if (!step) return;
      // The browser's own zoom never runs: the size is the app's setting.
      e.preventDefault();
      const next = stepTextSize(size, step);
      if (next !== size) setSize(next);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [size, setSize]);
}

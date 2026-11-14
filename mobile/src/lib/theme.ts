/**
 * Light and dark, as on the desktop. By default the app follows the phone's
 * own setting and switches when the phone does; the user can pin Light or
 * Dark instead (Settings, and Appearance in the menu). The choice is kept on
 * this phone only.
 *
 * The resolved theme is written to `<html data-theme>`, which selects the
 * desktop's token values (tokens.ts) and the phone's own (theme.css).
 */
import { useCallback, useEffect, useState } from 'react';

export type ThemeChoice = 'phone' | 'light' | 'dark';
export type Theme = 'light' | 'dark';

export const THEME_CHOICES: ReadonlyArray<{ value: ThemeChoice; label: string }> = [
  { value: 'phone', label: 'Phone' },
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
];

const KEY = 'shodh.theme';
const DARK_QUERY = '(prefers-color-scheme: dark)';

/** The saved choice; "phone" when none was saved or storage is unavailable. */
export function savedChoice(): ThemeChoice {
  try {
    const value = window.localStorage.getItem(KEY);
    return value === 'light' || value === 'dark' ? value : 'phone';
  } catch {
    return 'phone';
  }
}

function saveChoice(choice: ThemeChoice) {
  try {
    if (choice === 'phone') window.localStorage.removeItem(KEY);
    else window.localStorage.setItem(KEY, choice);
  } catch {
    // Storage unavailable: the choice lasts until the app closes.
  }
}

function darkQuery(): MediaQueryList | null {
  return typeof window.matchMedia === 'function' ? window.matchMedia(DARK_QUERY) : null;
}

/** The phone's own setting; dark when the WebView does not say. */
export function phoneTheme(): Theme {
  const query = darkQuery();
  if (!query) return 'dark';
  return query.matches ? 'dark' : 'light';
}

export function resolveTheme(choice: ThemeChoice, phone: Theme): Theme {
  return choice === 'phone' ? phone : choice;
}

export interface ThemeState {
  choice: ThemeChoice;
  theme: Theme;
  choose: (choice: ThemeChoice) => void;
}

/** The theme, kept on `<html data-theme>` and following the phone while the choice is "phone". */
export function useTheme(): ThemeState {
  const [choice, setChoice] = useState<ThemeChoice>(savedChoice);
  const [phone, setPhone] = useState<Theme>(phoneTheme);

  useEffect(() => {
    const query = darkQuery();
    if (!query) return;
    const changed = () => setPhone(query.matches ? 'dark' : 'light');
    changed();
    query.addEventListener('change', changed);
    return () => query.removeEventListener('change', changed);
  }, []);

  const theme = resolveTheme(choice, phone);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  const choose = useCallback((next: ThemeChoice) => {
    saveChoice(next);
    setChoice(next);
  }, []);

  return { choice, theme, choose };
}

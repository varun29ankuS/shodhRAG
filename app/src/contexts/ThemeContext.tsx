import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { clampSearchResults, getAppSettings, onAppSettingsChanged, updatePreferences } from '../lib/appSettings';

type Theme = 'light' | 'dark';

export type ThemeColors = {
  // Backgrounds
  bg: string;
  bgSecondary: string;
  bgTertiary: string;
  bgHover: string;
  bgActive: string;

  // Text
  text: string;
  textSecondary: string;
  textTertiary: string;
  textMuted: string;

  // Borders
  border: string;
  borderHover: string;
  borderActive: string;

  // Brand
  /** Accent fill (buttons, active indicators). Pair with `primaryText`. */
  primary: string;
  primaryHover: string;
  primaryText: string;
  /** Accent colour for text/icons on the page background (AA contrast). */
  accentText: string;

  secondary: string;
  accent: string;
  info: string;
  success: string;
  warning: string;
  error: string;

  // Component specific
  cardBg: string;
  cardBorder: string;
  inputBg: string;
  buttonBg: string;
  buttonText: string;
  buttonHover: string;
};

interface ThemeContextType {
  theme: Theme;
  toggleTheme: () => void;
  colors: ThemeColors;
}

/**
 * Legacy `colors` keys → design tokens declared in src/index.css (`--c-<token>`).
 * index.css is the single source of truth; this table only names which token
 * each legacy key reads.
 */
const TOKEN_FOR_KEY: Record<keyof ThemeColors, string> = {
  bg: 'ground',
  bgSecondary: 'surface',
  bgTertiary: 'raised',
  bgHover: 'raised-2',
  bgActive: 'pressed',

  text: 'text',
  textSecondary: 'text-secondary',
  textTertiary: 'text-muted',
  textMuted: 'text-faint',

  border: 'border',
  borderHover: 'border-strong',
  borderActive: 'border-active',

  primary: 'accent',
  primaryHover: 'accent-hover',
  primaryText: 'on-accent',
  accentText: 'accent-text',

  secondary: 'info',
  accent: 'violet',
  info: 'info',
  success: 'success',
  warning: 'warning',
  error: 'error',

  cardBg: 'surface-2',
  cardBorder: 'border',
  inputBg: 'surface',
  buttonBg: 'raised',
  buttonText: 'text',
  buttonHover: 'raised-2',
};

const HEX6 = /^#[0-9a-fA-F]{6}$/;

/**
 * Activate `theme` on the document root and read the resolved palette back
 * from the CSS custom properties. Values must be 6-digit hex because legacy
 * panels append alpha suffixes (`${colors.primary}14`).
 */
function applyThemeAndReadColors(theme: Theme): ThemeColors {
  const root = document.documentElement;
  root.setAttribute('data-theme', theme);
  root.classList.toggle('dark', theme === 'dark');

  const computed = getComputedStyle(root);
  const colors = {} as ThemeColors;
  for (const key of Object.keys(TOKEN_FOR_KEY) as (keyof ThemeColors)[]) {
    const prop = `--c-${TOKEN_FOR_KEY[key]}`;
    const value = computed.getPropertyValue(prop).trim();
    if (!HEX6.test(value)) {
      throw new Error(
        `Design token ${prop} resolved to "${value}" for theme "${theme}". ` +
        'Tokens are defined in src/index.css and must be 6-digit hex.'
      );
    }
    colors[key] = value;
  }
  return colors;
}

function readStoredTheme(): Theme {
  try {
    const stored = localStorage.getItem('theme');
    return stored === 'light' || stored === 'dark' ? stored : 'dark';
  } catch {
    return 'dark';
  }
}

/** Passages per search the user chose before settings moved to the backend. */
function readStoredSearchResults(): number {
  try {
    const saved = JSON.parse(localStorage.getItem('shodh_search_config') ?? 'null') as unknown;
    if (typeof saved === 'object' && saved !== null && typeof (saved as { maxResults?: unknown }).maxResults === 'number') {
      return (saved as { maxResults: number }).maxResults;
    }
  } catch {
    // Unreadable storage: fall through to the default.
  }
  return 8;
}

const ThemeContext = createContext<ThemeContextType | undefined>(undefined);

export const ThemeProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [theme, setTheme] = useState<Theme>(readStoredTheme);

  // Applying the attribute and reading the palette happen together so the
  // colours handed to children always match the active stylesheet.
  const colors = useMemo(() => applyThemeAndReadColors(theme), [theme]);

  useEffect(() => {
    try {
      localStorage.setItem('theme', theme);
    } catch {
      // Storage unavailable (private mode / quota); theme still applies for this session.
    }
  }, [theme]);

  // The backend settings store is the source of truth for the theme (the
  // agent can change it too); local storage only avoids a flash at startup.
  // On first run the store is seeded from what this browser storage held.
  useEffect(() => {
    let cancelled = false;
    getAppSettings()
      .then(async settings => {
        if (cancelled || !settings) return;
        if (settings.seeded) {
          setTheme(settings.preferences.theme);
          return;
        }
        await updatePreferences(
          { theme: readStoredTheme(), searchMaxResults: clampSearchResults(readStoredSearchResults()) },
          true,
        );
      })
      .catch(err => console.error('Loading app settings failed:', err));
    const unsubscribe = onAppSettingsChanged(settings => setTheme(settings.preferences.theme));
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  const themeRef = useRef(theme);
  themeRef.current = theme;
  const toggleTheme = useCallback(() => {
    const next: Theme = themeRef.current === 'light' ? 'dark' : 'light';
    setTheme(next);
    updatePreferences({ theme: next }).catch(err => console.error('Saving the theme failed:', err));
  }, []);

  const value = useMemo(() => ({ theme, toggleTheme, colors }), [theme, toggleTheme, colors]);

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
};

export const useTheme = () => {
  const context = useContext(ThemeContext);
  if (!context) {
    throw new Error('useTheme must be used within ThemeProvider');
  }
  return context;
};

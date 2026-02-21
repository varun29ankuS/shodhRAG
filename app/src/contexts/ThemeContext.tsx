import React, { createContext, useContext, useEffect, useState } from 'react';

type Theme = 'light' | 'dark';

interface ThemeContextType {
  theme: Theme;
  toggleTheme: () => void;
  colors: {
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

    // Brand colors (same in both themes)
    primary: string;
    primaryHover: string;
    primaryText: string;

    secondary: string;
    accent: string;
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
}

const lightColors = {
  bg: '#fafafa',
  bgSecondary: '#ffffff',
  bgTertiary: '#f0f0f2',
  bgHover: '#e8e8ec',
  bgActive: '#dcdce0',

  text: '#111113',
  textSecondary: '#1c1c1f',
  textTertiary: '#60606b',
  textMuted: '#8e8e99',

  border: '#e0e0e5',
  borderHover: '#c8c8d0',
  borderActive: '#a0a0ab',

  primary: '#c94d1f',
  primaryHover: '#b5441b',
  primaryText: '#ffffff',

  secondary: '#2563eb',
  accent: '#7c3aed',
  success: '#059669',
  warning: '#d97706',
  error: '#dc2626',

  cardBg: '#ffffff',
  cardBorder: '#e0e0e5',
  inputBg: '#ffffff',
  buttonBg: '#f0f0f2',
  buttonText: '#111113',
  buttonHover: '#e8e8ec',
};

const darkColors = {
  bg: '#0c0c0d',
  bgSecondary: '#141415',
  bgTertiary: '#1c1c1e',
  bgHover: '#232326',
  bgActive: '#2c2c30',

  text: '#f0f0f2',
  textSecondary: '#d4d4d8',
  textTertiary: '#8b8b95',
  textMuted: '#5c5c66',

  border: '#1e1e22',
  borderHover: '#2a2a2f',
  borderActive: '#3a3a40',

  primary: '#e8602a',
  primaryHover: '#f07040',
  primaryText: '#ffffff',

  secondary: '#3b82f6',
  accent: '#a78bfa',
  success: '#10b981',
  warning: '#f59e0b',
  error: '#ef4444',

  cardBg: '#141415',
  cardBorder: '#1e1e22',
  inputBg: '#141415',
  buttonBg: '#1c1c1e',
  buttonText: '#f0f0f2',
  buttonHover: '#232326',
};

const ThemeContext = createContext<ThemeContextType | undefined>(undefined);

export const ThemeProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [theme, setTheme] = useState<Theme>(() => {
    const stored = localStorage.getItem('theme') as Theme;
    return stored || 'dark';
  });

  useEffect(() => {
    localStorage.setItem('theme', theme);
    document.documentElement.setAttribute('data-theme', theme);

    // Add/remove 'dark' class for Tailwind dark mode
    if (theme === 'dark') {
      document.documentElement.classList.add('dark');
    } else {
      document.documentElement.classList.remove('dark');
    }

    // Apply theme colors to CSS variables
    const colors = theme === 'light' ? lightColors : darkColors;
    Object.entries(colors).forEach(([key, value]) => {
      document.documentElement.style.setProperty(`--color-${key}`, value);
    });
  }, [theme]);

  const toggleTheme = () => {
    setTheme(prev => prev === 'light' ? 'dark' : 'light');
  };

  const colors = theme === 'light' ? lightColors : darkColors;

  return (
    <ThemeContext.Provider value={{ theme, toggleTheme, colors }}>
      {children}
    </ThemeContext.Provider>
  );
};

export const useTheme = () => {
  const context = useContext(ThemeContext);
  if (!context) {
    throw new Error('useTheme must be used within ThemeProvider');
  }
  return context;
};

// Palette and type from the app (app/src/index.css tokens, Geist from
// app/tailwind.config.js). One accent colour.
import { Easing } from "remotion";

export const color = {
  dark: {
    ground: "#0b0b0d",
    surface: "#121215",
    raised: "#1b1b1f",
    border: "#222227",
    text: "#ececee",
    muted: "#9b9ba4",
    faint: "#8d8d96",
  },
  light: {
    ground: "#fafaf9",
    surface: "#ffffff",
    raised: "#f4f4f3",
    border: "#e6e6e3",
    borderStrong: "#d6d6d2",
    text: "#18181b",
    secondary: "#3f3f46",
    muted: "#52525b",
    faint: "#5f5f68",
  },
  accent: "#c94d1c",
  accentSoft: "#fbeee8",
} as const;

export const font = {
  sans: '"Geist Variable", system-ui, "Segoe UI", sans-serif',
  devanagari: '"Noto Sans Devanagari", "Nirmala UI", sans-serif',
} as const;

/** Calm ease used for every move: slow in, slower out. */
export const calm = Easing.bezier(0.33, 0, 0.2, 1);

export const FPS = 30;

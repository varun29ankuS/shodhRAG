// Palette and type from the app (app/src/index.css tokens, Geist from
// app/tailwind.config.js). The ground is Shodh orange; text on it is white
// (4.6:1 on the accent, used only at display sizes).
import { Easing } from "remotion";

export const color = {
  accent: "#c94d1c",
  /** The ground's lit centre and dark edge, around the accent. */
  groundHi: "#e8672e",
  groundLo: "#a83c12",
  onAccent: "#ffffff",
  ink: "#18181b",
  surface: "#ffffff",
} as const;

export const font = {
  sans: '"Geist Variable", system-ui, "Segoe UI", sans-serif',
} as const;

/** Camera moves: quick out of the start, settling hard. */
export const snappy = Easing.bezier(0.16, 1, 0.3, 1);

/** Whip transitions: accelerate away. */
export const whipOut = Easing.bezier(0.7, 0, 0.84, 0);

export const FPS = 30;

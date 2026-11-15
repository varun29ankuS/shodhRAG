/**
 * The system bars in tests. The app draws edge to edge, under Android's
 * status bar and gesture bar; every layer of the app that reaches an edge of
 * the window keeps that edge's inset (theme.css `--inset-*`) clear.
 *
 * jsdom lays nothing out and resolves neither var() nor calc(), so the check
 * is in two halves: every control on every screen is inside one of the
 * layers below (`checkLayers`, run over each screen by tests/screens.test.tsx),
 * and each layer's rule keeps its edges clear of simulated insets
 * (tests/insets.test.tsx).
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { expect } from 'vitest';

export type Edge = 'top' | 'right' | 'bottom' | 'left';

/** The insets the rules are checked against: a tall status bar, a gesture bar and side cutouts. */
export const SIMULATED: Record<Edge, number> = { top: 48, right: 16, bottom: 34, left: 16 };

/**
 * What keeps an edge of a layer clear: a property of a rule (a padding, or the
 * layer's offset from that edge), and the room it keeps besides the inset.
 */
export interface Reserve {
  rule: string;
  property: string;
  /** Room in px the edge keeps in addition to the inset (the tab bar under the composer). */
  beyond: number;
}

/**
 * Each layer that reaches an edge of the window, by its class, and what keeps
 * each edge it reaches clear. A layer that lies along an edge but only in a
 * part (the composer, the tab bar) keeps only that edge.
 */
export const LAYERS: Record<string, Partial<Record<Edge, Reserve>>> = {
  // A screen: its bar and large title start below the status bar.
  // Its end scrolls clear of the tab bar and the gesture bar under it.
  view: {
    top: { rule: '.view', property: 'padding-top', beyond: 0 },
    right: { rule: '.view', property: 'padding-right', beyond: 0 },
    bottom: { rule: '.scroll', property: 'padding-bottom', beyond: 50 },
    left: { rule: '.view', property: 'padding-left', beyond: 0 },
  },
  reader: {
    top: { rule: '.reader', property: 'padding-top', beyond: 0 },
    right: { rule: '.reader', property: 'padding-right', beyond: 0 },
    bottom: { rule: '.reader-body', property: 'padding-bottom', beyond: 0 },
    left: { rule: '.reader', property: 'padding-left', beyond: 0 },
  },
  drawer: {
    top: { rule: '.drawer', property: 'padding-top', beyond: 0 },
    bottom: { rule: '.drawer', property: 'padding-bottom', beyond: 0 },
    left: { rule: '.drawer', property: 'padding-left', beyond: 0 },
  },
  // A sheet rises from the bottom; a tall one stops under the status bar.
  overlay: {
    top: { rule: '.overlay', property: 'padding-top', beyond: 0 },
    bottom: { rule: '.sheet', property: 'padding-bottom', beyond: 0 },
  },
  lock: {
    top: { rule: '.lock', property: 'padding-top', beyond: 0 },
    right: { rule: '.lock', property: 'padding-right', beyond: 0 },
    bottom: { rule: '.lock', property: 'padding-bottom', beyond: 0 },
    left: { rule: '.lock', property: 'padding-left', beyond: 0 },
  },
  note: {
    top: { rule: '.note', property: 'top', beyond: 0 },
  },
  // The tabs sit above the gesture bar: the bar's padding is the inset.
  tabbar: {
    right: { rule: '.tabbar', property: 'padding-right', beyond: 0 },
    bottom: { rule: '.tabbar', property: 'padding-bottom', beyond: 0 },
    left: { rule: '.tabbar', property: 'padding-left', beyond: 0 },
  },
  // The follow-up field floats above the tab bar (50px) and the gesture bar.
  'askbox-floating': {
    right: { rule: '.askbox-floating', property: 'right', beyond: 0 },
    bottom: { rule: '.askbox-floating', property: 'bottom', beyond: 50 },
    left: { rule: '.askbox-floating', property: 'left', beyond: 0 },
  },
};

const SELECTOR = Object.keys(LAYERS)
  .map(name => `.${name}`)
  .join(', ');

/**
 * Every control on the screen is inside a layer that keeps the insets clear.
 * The scrim behind a sheet or the menu is not: it covers the whole window on
 * purpose, so a tap anywhere outside the sheet closes it.
 */
export function checkLayers(where: string) {
  const controls = document.querySelectorAll<HTMLElement>('button, a[href], input, textarea, select, [role="button"], [role="radio"]');
  expect(controls.length, where).toBeGreaterThan(0);
  for (const control of controls) {
    if (control.classList.contains('scrim')) continue;
    const name = `${where}: ${control.tagName.toLowerCase()}.${control.className || '(no class)'} "${control.textContent?.trim().slice(0, 30) ?? ''}"`;
    expect(control.closest(SELECTOR), `${name} is in no layer that keeps the system bars clear (tests/insets.ts)`).not.toBeNull();
  }
}

// ── The stylesheet, read as the phone's WebView would with the insets above ──

export const stylesheet = readFileSync(resolve(process.cwd(), 'src/theme.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

/** The declarations of the rule written exactly as `selector { … }` (the first one). */
export function ruleBody(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const body = new RegExp(`(?:^|\\n)${escaped}\\s*\\{([^}]*)\\}`).exec(stylesheet)?.[1];
  expect(body, `theme.css has a rule ${selector}`).toBeDefined();
  return body ?? '';
}

function declared(body: string, property: string): string | null {
  const match = new RegExp(`(?:^|[\\s;])${property}:\\s*([^;]+);`).exec(body);
  return match ? match[1].trim() : null;
}

/** The values of a shorthand, split at spaces outside parentheses. */
function components(value: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let current = '';
  for (const ch of value) {
    if (ch === '(') depth += 1;
    if (ch === ')') depth -= 1;
    if (/\s/.test(ch) && depth === 0) {
      if (current) out.push(current);
      current = '';
    } else current += ch;
  }
  if (current) out.push(current);
  return out;
}

const SIDE_INDEX: Record<Edge, number> = { top: 0, right: 1, bottom: 2, left: 3 };

/** One side of a 1 to 4 value shorthand (padding, margin). */
function side(values: string[], edge: Edge): string {
  const [a, b = a, c = a, d = b] = values;
  return [a, b, c, d][SIDE_INDEX[edge]];
}

/** A length that only adds px and the insets (`calc(28px + var(--inset-top))`), in px. */
export function pixels(value: string, insets: Record<Edge, number>): number {
  const substituted = value.replace(/var\(--inset-(top|right|bottom|left)\)/g, (_, edge: Edge) => `${insets[edge]}px`);
  expect(substituted, `${value} is read without the insets' variables`).not.toMatch(/env\(|var\(/);
  expect(substituted, `${value} only adds`).not.toMatch(/\s[-*/]\s/);
  return [...substituted.matchAll(/(\d+(?:\.\d+)?)px/g)].reduce((sum, m) => sum + Number(m[1]), 0);
}

/** A property of a rule as px, with the insets: a longhand, or its side of the padding shorthand. */
export function reserved(rule: string, property: string, insets: Record<Edge, number>): number {
  const body = ruleBody(rule);
  const longhand = declared(body, property);
  if (longhand !== null) return pixels(longhand, insets);
  const shorthand = /^padding-(top|right|bottom|left)$/.exec(property);
  const padding = shorthand ? declared(body, 'padding') : null;
  expect(padding, `${rule} declares ${property}`).not.toBeNull();
  return pixels(side(components(padding ?? ''), shorthand?.[1] as Edge), insets);
}

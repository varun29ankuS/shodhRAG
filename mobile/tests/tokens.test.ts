import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { contrastRatio, themeTokens } from '../../app/src/lib/contrast.ts';
import { PHONE_COLOURS, STATUS_TOKENS, tokensCss, type PhoneColours } from '../tokens';

const desktop = readFileSync(resolve(process.cwd(), '../app/src/index.css'), 'utf8');
const phoneCss = readFileSync(resolve(process.cwd(), 'src/theme.css'), 'utf8');
const desktopDark = themeTokens(desktop, '[data-theme="dark"]');
const desktopLight = themeTokens(desktop, '[data-theme="light"]');

/** A colour as it shows over an opaque one: "#rrggbb", or "rgba(r, g, b, a)" laid over `under`. */
function over(colour: string, under: string): string {
  if (/^#[0-9a-f]{6}$/i.test(colour)) return colour.toLowerCase();
  const rgba = /^rgba\(\s*(\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\s*\)$/.exec(colour);
  if (!rgba) throw new Error(`not a colour the test reads: ${colour}`);
  const alpha = Number(rgba[4]);
  const base = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(under);
  if (!base) throw new Error(`not an opaque colour: ${under}`);
  return `#${[1, 2, 3]
    .map(i => Math.round(Number(rgba[i]) * alpha + parseInt(base[i], 16) * (1 - alpha)))
    .map(v => v.toString(16).padStart(2, '0'))
    .join('')}`;
}

describe("the phone's colours", () => {
  it("are the phone's own greys and tint in both themes, with the desktop's status colours: the phone decides until the app says, and the app can pin either", () => {
    const css = tokensCss(desktop);
    const [darkPart, rest] = css.split('@media (prefers-color-scheme: light)');
    const [followed, pinned] = rest.split(':root[data-theme="light"]');
    for (const [name, value] of Object.entries(PHONE_COLOURS.dark)) expect(darkPart).toContain(`--${name}: ${value};`);
    for (const [name, value] of Object.entries(PHONE_COLOURS.light)) {
      expect(followed).toContain(`--${name}: ${value};`);
      expect(pinned).toContain(`--${name}: ${value};`);
    }
    for (const name of STATUS_TOKENS) {
      expect(darkPart).toContain(`--c-${name}: ${desktopDark[name]};`);
      expect(followed).toContain(`--c-${name}: ${desktopLight[name]};`);
      expect(pinned).toContain(`--c-${name}: ${desktopLight[name]};`);
    }
    // Only the status colours come from the desktop: its greys and accent are not the phone's.
    expect(css.match(/--c-[a-z-]+:/g)?.length).toBe(STATUS_TOKENS.length * 3);
    expect(darkPart).toContain('color-scheme: dark;');
    // The phone's setting applies only until the app writes its theme.
    expect(followed).toContain(':root:not([data-theme])');
    expect(pinned).toContain('color-scheme: light;');
  });

  it('are the approved system values: greys, cells, separators and the tint', () => {
    expect(PHONE_COLOURS.light).toMatchObject({ background: '#f2f2f7', cell: '#ffffff', label: '#000000', tint: '#c2410c', separator: 'rgba(60, 60, 67, 0.2)' });
    expect(PHONE_COLOURS.dark).toMatchObject({ background: '#000000', cell: '#1c1c1e', label: '#ffffff', tint: '#ff8a4c', separator: 'rgba(84, 84, 88, 0.6)' });
    expect(PHONE_COLOURS.light['label-3']).toBe('rgba(60, 60, 67, 0.3)');
    expect(PHONE_COLOURS.dark['label-2']).toBe('rgba(235, 235, 245, 0.6)');
  });

  it('read at 4.5:1 or more in both themes, on every surface text sits on', () => {
    const brandRed = /--brand-red:\s*(#[0-9a-f]{6})/i.exec(phoneCss)?.[1];
    expect(brandRed).toBeDefined();
    for (const [theme, t, d] of [
      ['dark', PHONE_COLOURS.dark, desktopDark],
      ['light', PHONE_COLOURS.light, desktopLight],
    ] as Array<[string, PhoneColours, Record<string, string>]>) {
      // The surfaces: the background, a cell, a field on each, and the translucent bars.
      const ground = t.background;
      const cell = t.cell;
      const fieldOnGround = over(t.fill, ground);
      const fieldOnCell = over(t.fill, cell);
      const bar = over(t.glass, ground);
      const pairs: Array<[string, string, string]> = [
        ['text on the background', t.label, ground],
        ['text in a cell', t.label, cell],
        ['text in a field', t.label, fieldOnGround],
        ['text on a bar', t.label, bar],
        ['secondary text on the background (section headers)', over(t['label-2'], ground), ground],
        ['secondary text in a cell', over(t['label-2'], cell), cell],
        ['a placeholder in a field on the background', over(t['label-2'], fieldOnGround), fieldOnGround],
        ['a placeholder in a field in a cell', over(t['label-2'], fieldOnCell), fieldOnCell],
        ['an inactive tab, on the bar', over(t['label-2'], bar), bar],
        ['a link on the background', t.tint, ground],
        ['a link in a cell', t.tint, cell],
        ['the active tab, on the bar', t.tint, bar],
        ['a word or glyph on the tint (Send, a primary button)', t['on-tint'], t.tint],
        ['the count of approvals waiting', '#ffffff', brandRed ?? '#ffffff'],
        ['the connected note', d['success'], d['success-soft']],
        ['a note of a problem', d['warning'], d['warning-soft']],
        ['warning words on their callout', t.label, d['warning-soft']],
        ['the asleep line', d['warning'], ground],
        // The conversation kinds' icons, and the Code layout.
        ['the Code icon in a cell', d['info'], cell],
        ['the Code icon on the background', d['info'], ground],
        ['the Creator icon in a cell', d['violet'], cell],
        ['the Creator icon on the background', d['violet'], ground],
        ['a command that passed', d['success'], cell],
        ['a command that failed', d['error'], cell],
        ["a command's output", over(t['label-2'], fieldOnCell), fieldOnCell],
      ];
      for (const [what, fg, bg] of pairs) {
        expect(fg, `${theme}: ${what}`).toMatch(/^#[0-9a-f]{6}$/i);
        expect(contrastRatio(fg, bg), `${theme}: ${what}`).toBeGreaterThanOrEqual(4.5);
      }
    }
    // Pages are paper in both themes.
    const paper = (name: string) => new RegExp(`--${name}:\\s*(#[0-9a-f]{6})`, 'i').exec(phoneCss)?.[1] ?? '#000000';
    expect(contrastRatio(paper('paper-ink'), paper('paper'))).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(paper('paper-text'), paper('paper'))).toBeGreaterThanOrEqual(4.5);
  });

  it('keep the tertiary grey off text: it is for chevrons and hairlines only', () => {
    const css = phoneCss.replace(/\/\*[\s\S]*?\*\//g, '');
    // Glyphs with no words in them: a row's chevron, the arrow between breadcrumbs.
    const glyphs = new Set(['.chevron', '.crumb-sep']);
    let seen = 0;
    for (const rule of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
      const selector = rule[1].trim();
      if (!/(^|[;\s])color:\s*var\(--label-3\)/.test(rule[2])) continue;
      seen += 1;
      expect(glyphs.has(selector), `${selector} sets text in the tertiary grey`).toBe(true);
    }
    expect(seen).toBeGreaterThan(0);
  });
});

import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { contrastRatio, themeTokens } from '../../app/src/lib/contrast.ts';
import { tokensCss } from '../tokens';

const desktop = readFileSync(resolve(process.cwd(), '../app/src/index.css'), 'utf8');
const phoneCss = readFileSync(resolve(process.cwd(), 'src/theme.css'), 'utf8');
const dark = themeTokens(desktop, '[data-theme="dark"]');
const light = themeTokens(desktop, '[data-theme="light"]');

describe("the phone's colours", () => {
  it('are the desktop tokens of both themes: the phone decides until the app says, and the app can pin either', () => {
    const css = tokensCss(desktop);
    const [darkPart, rest] = css.split('@media (prefers-color-scheme: light)');
    const [followed, pinned] = rest.split(':root[data-theme="light"]');
    for (const [name, value] of Object.entries(dark)) expect(darkPart).toContain(`--c-${name}: ${value};`);
    for (const [name, value] of Object.entries(light)) {
      expect(followed).toContain(`--c-${name}: ${value};`);
      expect(pinned).toContain(`--c-${name}: ${value};`);
    }
    expect(darkPart).toContain('color-scheme: dark;');
    // The phone's setting applies only until the app writes its theme.
    expect(followed).toContain(':root:not([data-theme])');
    expect(pinned).toContain('color-scheme: light;');
  });

  it('read at 4.5:1 or more in both themes, the gradient under white text included', () => {
    const brandRed = /--brand-red:\s*(#[0-9a-f]{6})/i.exec(phoneCss)?.[1];
    expect(brandRed).toBeDefined();
    for (const [theme, t] of [
      ['dark', dark],
      ['light', light],
    ] as const) {
      const pairs: Array<[string, string, string]> = [
        ['text on the ground', t['text'], t['ground']],
        ['text on a card', t['text'], t['surface']],
        ['secondary text on a card', t['text-secondary'], t['surface']],
        ['muted text on the ground', t['text-muted'], t['ground']],
        ['muted text on a field', t['text-muted'], t['raised']],
        ['faint text on the ground', t['text-faint'], t['ground']],
        ['faint text on a card', t['text-faint'], t['surface']],
        ['links on the ground', t['accent-text'], t['ground']],
        ['links on a card', t['accent-text'], t['surface']],
        ["white on the accent (the gradient's end)", t['on-accent'], t['accent']],
        ["white on the elephant red (the gradient's start)", t['on-accent'], brandRed ?? '#000000'],
        ['warning words on their callout', t['text'], t['warning-soft']],
        ['the connected note', t['success'], t['success-soft']],
        ['a note of a problem', t['warning'], t['warning-soft']],
        ['the asleep line', t['warning'], t['ground']],
        ['the count on the tab bar', brandRed ?? '#ffffff', t['on-accent']],
        ['muted text in the menu', t['text-muted'], t['sidebar']],
        ['links in the menu', t['accent-text'], t['sidebar']],
        // The conversation kinds' icons (on their light tint), and the Code layout.
        ['the Research icon', t['accent-text'], t['surface']],
        ['the Code icon', t['info'], t['surface']],
        ['the Code icon on the ground', t['info'], t['ground']],
        ['the Creator icon', t['violet'], t['surface']],
        ['the Creator icon on the ground', t['violet'], t['ground']],
        ['a command that passed', t['success'], t['surface']],
        ['a command that failed', t['error'], t['surface']],
        ["a command's output", t['text-secondary'], t['raised']],
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
});

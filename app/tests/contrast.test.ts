/**
 * Colour token contrast audit (WCAG 2.x AA), read straight from
 * src/index.css for both themes. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/contrast.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { contrastRatio, luminance, parseHex, themeTokens } from '../src/lib/contrast.ts';

const css = readFileSync(new URL('../src/index.css', import.meta.url), 'utf8');

const THEMES = {
  dark: themeTokens(css, '[data-theme="dark"]'),
  light: themeTokens(css, '[data-theme="light"]'),
};

/** Surfaces text is drawn on. */
const SURFACES = ['ground', 'sidebar', 'surface', 'surface-2', 'raised', 'raised-2'];
/** Tinted backgrounds used behind chips and banners. */
const TINTS = ['accent-soft', 'success-soft', 'warning-soft'];
/** Tokens used for body and secondary text. */
const TEXT = ['text', 'text-secondary', 'text-tertiary', 'text-muted', 'text-faint'];
/** Tokens used for coloured text (links, status labels). */
const COLOURED_TEXT = ['accent-text', 'success', 'warning', 'error', 'info', 'violet'];

const AA_TEXT = 4.5;
const AA_UI = 3;

test('helpers compute WCAG ratios', () => {
  assert.deepEqual(parseHex('#c94d1c'), [201, 77, 28]);
  assert.equal(Math.round(contrastRatio('#000000', '#ffffff') * 100) / 100, 21);
  assert.equal(contrastRatio('#777777', '#777777'), 1);
  assert.ok(luminance('#ffffff') > luminance('#fafaf9'));
  assert.throws(() => parseHex('#fff'));
});

for (const [theme, t] of Object.entries(THEMES)) {
  test(`${theme}: every token used here is defined`, () => {
    for (const name of [...SURFACES, ...TINTS, ...TEXT, ...COLOURED_TEXT, 'accent', 'on-accent', 'danger', 'on-danger']) {
      assert.ok(t[name], `--c-${name} missing in ${theme}`);
    }
  });

  test(`${theme}: text tokens reach AA on every surface and tint`, () => {
    const failures: string[] = [];
    for (const fg of [...TEXT, ...COLOURED_TEXT]) {
      for (const bg of [...SURFACES, ...TINTS]) {
        const ratio = contrastRatio(t[fg], t[bg]);
        if (ratio < AA_TEXT) failures.push(`${fg} on ${bg}: ${ratio.toFixed(2)}`);
      }
    }
    assert.deepEqual(failures, []);
  });

  test(`${theme}: filled buttons reach AA`, () => {
    assert.ok(contrastRatio(t['on-accent'], t.accent) >= AA_TEXT, 'on-accent on accent');
    assert.ok(contrastRatio(t['on-accent'], t['accent-hover']) >= AA_TEXT, 'on-accent on accent-hover');
    assert.ok(contrastRatio(t['on-danger'], t.danger) >= AA_TEXT, 'on-danger on danger');
  });

  test(`${theme}: the focus ring is visible on every surface`, () => {
    // Tailwind's `ring` colour is --c-accent-text.
    for (const bg of SURFACES) {
      assert.ok(contrastRatio(t['accent-text'], t[bg]) >= AA_UI, `focus ring on ${bg}`);
    }
  });
}

/**
 * Legible text: the type scale has nothing below 12px, Tailwind is configured
 * with the same scale, and no source file sets a smaller size in any form
 * (classes, CSS, SVG attributes, inline styles, px, em or rem). Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/typeScale.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { MIN_FONT_PX, TYPE_SCALE, TYPE_SCALE_NAMES } from '../src/lib/typeScale.ts';

const here = dirname(fileURLToPath(import.meta.url));
const appDir = join(here, '..');
const srcDir = join(appDir, 'src');

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) out.push(...sourceFiles(path));
    else if (/\.(tsx?|css)$/.test(name)) out.push(path);
  }
  return out;
}

/** A size in CSS pixels: px (or a bare SVG number) as written, em and rem against a 16px base. */
function px(value: string, unit: string | undefined): number {
  const n = parseFloat(value);
  return unit === 'em' || unit === 'rem' ? n * 16 : n;
}

/**
 * Every way a font size is written in the app: classes (`text-[11px]`, `text-[0.6rem]`),
 * CSS (`font-size: 11px`), SVG attributes (`font-size="10.5"`) and inline styles
 * (`fontSize: 11`, `fontSize: '11px'`, `fontSize={11}`). Group 1 is the number, group 2
 * its unit (none: pixels).
 */
const SIZE_PATTERNS: readonly RegExp[] = [
  /text-\[(\d+(?:\.\d+)?)(px|em|rem)\]/g,
  /font-size:\s*(\d+(?:\.\d+)?)(px|em|rem)/g,
  /font-size="(\d+(?:\.\d+)?)(px|em|rem)?"/g,
  /fontSize(?::|=\{)\s*['"]?(\d+(?:\.\d+)?)(px|em|rem)?/g,
];

/** The sizes below 12px written in `text`. */
function smallSizes(text: string): string[] {
  const found: string[] = [];
  for (const pattern of SIZE_PATTERNS) {
    for (const m of text.matchAll(pattern)) {
      if (px(m[1], m[2]) < MIN_FONT_PX) found.push(m[0]);
    }
  }
  // An SVG size from a template: `font-size="${mono ? 11 : 22}"`.
  for (const m of text.matchAll(/font-size="\$\{([^}]*)\}"/g)) {
    for (const n of m[1].matchAll(/(\d+(?:\.\d+)?)/g)) {
      if (parseFloat(n[1]) < MIN_FONT_PX) found.push(m[0]);
    }
  }
  return found;
}

test('the type scale starts at 12px and grows', () => {
  const sizes = TYPE_SCALE_NAMES.map(name => parseFloat(TYPE_SCALE[name]));
  assert.equal(Math.min(...sizes), MIN_FONT_PX);
  assert.ok(sizes.every((size, i) => i === 0 || size > sizes[i - 1]), `${sizes}`);
});

test('Tailwind is configured with the same scale', () => {
  // tailwind.config.js is CommonJS in an ES module package: read it as text.
  const config = readFileSync(join(appDir, 'tailwind.config.js'), 'utf8');
  const block = /fontSize:\s*\{([^}]*)\}/.exec(config);
  assert.ok(block, 'tailwind.config.js has a fontSize scale');
  const configured: Record<string, string> = {};
  for (const m of block[1].matchAll(/['"]?([\w-]+)['"]?\s*:\s*['"]([^'"]+)['"]/g)) configured[m[1]] = m[2];
  assert.deepEqual(configured, { ...TYPE_SCALE });
});

test('the check sees every way a size is written', () => {
  for (const sample of [
    'text-[11px]',
    'text-[0.6rem]',
    'text-[0.7em]',
    'font-size: 10px',
    'font-size: 0.65rem',
    'font-size="10.5"',
    'font-size="${mono ? 11 : 22}"',
    'fontSize: 11',
    "fontSize: '11px'",
    'fontSize={10}',
  ]) {
    assert.ok(smallSizes(sample).length > 0, sample);
  }
  for (const fine of ['text-[12px]', 'text-[0.875em]', 'font-size="12"', 'font-size="${mono ? 12 : 22}"', 'fontSize: 14']) {
    assert.deepEqual(smallSizes(fine), [], fine);
  }
});

test('no source file sets a font size below 12px', () => {
  const tooSmall: string[] = [];
  for (const file of sourceFiles(srcDir)) {
    const where = relative(appDir, file);
    for (const size of smallSizes(readFileSync(file, 'utf8'))) tooSmall.push(`${where}: ${size}`);
  }
  assert.deepEqual(tooSmall, []);
});

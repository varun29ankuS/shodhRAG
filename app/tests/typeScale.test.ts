/**
 * Legible text: the type scale has nothing below 12px, Tailwind is configured
 * with the same scale, and no source file sets a smaller pixel size. Run with Node 22.6+:
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

test('no source file sets a font size below 12px', () => {
  const tooSmall: string[] = [];
  for (const file of sourceFiles(srcDir)) {
    const text = readFileSync(file, 'utf8');
    const where = relative(appDir, file);
    for (const m of text.matchAll(/text-\[(\d+(?:\.\d+)?)px\]/g)) {
      if (parseFloat(m[1]) < MIN_FONT_PX) tooSmall.push(`${where}: ${m[0]}`);
    }
    for (const m of text.matchAll(/font-size:\s*(\d+(?:\.\d+)?)px/g)) {
      if (parseFloat(m[1]) < MIN_FONT_PX) tooSmall.push(`${where}: ${m[0]}`);
    }
    for (const m of text.matchAll(/fontSize(?::|=\{)\s*(\d+(?:\.\d+)?)\b/g)) {
      if (parseFloat(m[1]) < MIN_FONT_PX) tooSmall.push(`${where}: ${m[0]}`);
    }
  }
  assert.deepEqual(tooSmall, []);
});

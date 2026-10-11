/**
 * Motion tokens: src/lib/motion.ts (framer-motion) must match the CSS custom
 * properties in src/index.css. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/motion.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { DURATION, EASE } from '../src/lib/motion.ts';

const css = readFileSync(new URL('../src/index.css', import.meta.url), 'utf8');

function cssVar(name: string): string {
  const m = new RegExp(`--${name}:\\s*([^;]+);`).exec(css);
  assert.ok(m, `--${name} is defined`);
  return m[1].trim();
}

test('durations match the CSS tokens', () => {
  for (const [name, seconds] of Object.entries(DURATION)) {
    assert.equal(cssVar(`dur-${name}`), `${Math.round(seconds * 1000)}ms`, `--dur-${name}`);
  }
});

test('easings match the CSS tokens', () => {
  for (const [name, curve] of Object.entries(EASE)) {
    const values = /cubic-bezier\(([^)]+)\)/.exec(cssVar(`ease-${name}`))![1].split(',').map(v => Number(v.trim()));
    assert.deepEqual(values, [...curve], `--ease-${name}`);
  }
});

test('exits are faster than entrances', () => {
  assert.ok(DURATION.exit < DURATION.panel);
});

test('reduced motion collapses every duration token', () => {
  const block = /@media \(prefers-reduced-motion: reduce\)\s*\{\s*:root\s*\{([^}]*)\}/.exec(css);
  assert.ok(block, 'reduced-motion token block exists');
  for (const name of Object.keys(DURATION)) {
    assert.match(block[1], new RegExp(`--dur-${name}:\\s*0ms`), `--dur-${name} is 0ms under reduced motion`);
  }
});

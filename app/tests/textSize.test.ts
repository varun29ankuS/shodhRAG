/**
 * Settings → Appearance → Text size, and Ctrl + / Ctrl − / Ctrl 0. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/textSize.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  DEFAULT_TEXT_SIZE,
  TEXT_SIZES,
  TEXT_SIZE_ZOOM,
  parseTextSize,
  stepTextSize,
} from '../src/lib/textSize.ts';
import { parseAppSettings } from '../src/lib/appSettings.ts';

const base = {
  preferences: { theme: 'dark', searchMaxResults: 8 },
  policy: { localOnly: false, webAccess: true },
};

test('the saved size is read back, and settings from before it read as Default', () => {
  assert.equal(parseAppSettings(base)?.preferences.textSize, 'default');
  assert.equal(parseAppSettings({ ...base, preferences: { ...base.preferences, textSize: 'larger' } })?.preferences.textSize, 'larger');
  for (const value of [undefined, null, 'huge', 3, 'Large']) {
    assert.equal(parseTextSize(value), DEFAULT_TEXT_SIZE, String(value));
  }
});

test('the shortcuts step one size and stop at Small and Larger', () => {
  assert.equal(stepTextSize('default', 'larger'), 'large');
  assert.equal(stepTextSize('large', 'larger'), 'larger');
  assert.equal(stepTextSize('larger', 'larger'), 'larger');
  assert.equal(stepTextSize('default', 'smaller'), 'small');
  assert.equal(stepTextSize('small', 'smaller'), 'small');
  for (const size of TEXT_SIZES) assert.equal(stepTextSize(size, 'reset'), 'default');
});

test('each size draws larger than the one before, and Default is unscaled', () => {
  const zooms = TEXT_SIZES.map(size => TEXT_SIZE_ZOOM[size]);
  assert.ok(zooms.every((z, i) => i === 0 || z > zooms[i - 1]), `${zooms}`);
  assert.equal(TEXT_SIZE_ZOOM.default, 1);
});

test('the backend draws the same zoom for each size', () => {
  const rust = readFileSync(join(dirname(fileURLToPath(import.meta.url)), '../src-tauri/src/app_settings.rs'), 'utf8');
  for (const size of TEXT_SIZES) {
    const variant = size.charAt(0).toUpperCase() + size.slice(1);
    const zoom = TEXT_SIZE_ZOOM[size];
    const written = Number.isInteger(zoom) ? zoom.toFixed(1) : String(zoom);
    assert.ok(rust.includes(`TextSize::${variant} => ${written},`), `${variant} => ${written}`);
  }
});

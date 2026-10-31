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
  LatestRequest,
  textSizeStepFor,
  TEXT_SIZES,
  TEXT_SIZE_ZOOM,
  parseTextSize,
  stepTextSize,
} from '../src/lib/textSize.ts';
import { parseAppSettings } from '../src/lib/appSettings.ts';
import { viewerCommand } from '../src/features/ask/viewer/viewerKeys.ts';
import { focusCommand } from '../src/features/focus/focusKeys.ts';

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

const chord = (key: string, code: string, extra: Record<string, boolean> = {}) => ({
  key, code, ctrlKey: true, metaKey: false, shiftKey: false, altKey: false, defaultPrevented: false, repeat: false, ...extra,
});

test('the PDF viewer and the pop-out keep their own zoom chords: the app size does not change', () => {
  for (const [key, code, step] of [['=', 'Equal', 'larger'], ['-', 'Minus', 'smaller'], ['0', 'Digit0', 'reset']] as const) {
    const e = chord(key, code);
    // Unhandled, the chord changes the app's text size...
    assert.equal(textSizeStepFor(e, false), step);
    // ...but the viewer and the pop-out take it (and prevent it): then it is theirs.
    const base = { key, ctrlKey: true, metaKey: false, altKey: false, shiftKey: false, editable: false, mac: false };
    assert.notEqual(viewerCommand(base), null, `the viewer handles Ctrl ${key}`);
    assert.notEqual(focusCommand({ ...base, pannable: false }), null, `the pop-out handles Ctrl ${key}`);
    assert.equal(textSizeStepFor({ ...e, defaultPrevented: true }, false), null);
  }
});

test('auto-repeat never races through the sizes, and a Mac uses Cmd only', () => {
  assert.equal(textSizeStepFor(chord('=', 'Equal', { repeat: true }), false), null);
  assert.equal(textSizeStepFor(chord('=', 'Equal'), true), null, "a Mac's Ctrl+= is not the shortcut");
  assert.equal(textSizeStepFor(chord('=', 'Equal', { ctrlKey: false, metaKey: true }), true), 'larger');
  assert.equal(textSizeStepFor(chord('_', 'Minus', { shiftKey: true }), false), 'smaller', 'Ctrl+Shift+− works like Ctrl+Shift+=');
  assert.equal(textSizeStepFor(chord('a', 'KeyA'), false), null);
});

test('only the answer to the latest save sets the size', () => {
  const requests = new LatestRequest();
  const first = requests.next();
  const second = requests.next();
  assert.equal(requests.isLatest(first), false, 'an older answer arriving late is dropped');
  assert.equal(requests.isLatest(second), true);
});

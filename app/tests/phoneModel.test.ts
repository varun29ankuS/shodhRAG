/**
 * Settings → Phone model tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/phoneModel.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { errorText, PHONE_DEVICES_CHANGED, whenSeen } from '../src/features/phone/phoneModel.ts';

const here = dirname(fileURLToPath(import.meta.url));

test('a paired phone says when it was paired and last connected', () => {
  const phone = { nodeId: 'a', name: 'Pixel 8', pairedAt: '2026-11-13T10:00:00Z', lastSeen: '2026-11-14T09:00:00Z' };
  assert.equal(whenSeen(phone, 'en-GB'), 'Paired 13 Nov 2026 · last connected 14 Nov 2026');
  assert.equal(whenSeen({ ...phone, lastSeen: null }, 'en-GB'), 'Paired 13 Nov 2026');
  assert.equal(whenSeen({ ...phone, pairedAt: 'not a date', lastSeen: null }, 'en-GB'), 'Paired');
});

test('errors read as words', () => {
  assert.equal(errorText('Local-only mode is on.'), 'Local-only mode is on.');
  assert.equal(errorText(new Error('broken')), 'broken');
  assert.equal(errorText({ code: 'refused', message: 'Not from a phone.' }), 'Not from a phone.');
});

test('the devices event is the one the desktop sends', () => {
  const rust = readFileSync(join(here, '..', 'src-tauri', 'src', 'phone_link.rs'), 'utf8');
  assert.match(rust, new RegExp(`PHONE_DEVICES_EVENT: &str = "${PHONE_DEVICES_CHANGED}"`));
});

test('the Phone page writes no text under 12px and puts icons before labels', () => {
  const page = readFileSync(join(here, '..', 'src', 'features', 'phone', 'PhoneSettings.tsx'), 'utf8');
  assert.doesNotMatch(page, /text-\[(?:[0-9]|1[01])px\]/, 'no text below 12px');
  assert.doesNotMatch(page, /text-(?:xs|\[10px\]|\[11px\])\b/);
  // Every button's icon comes before its label.
  for (const button of page.matchAll(/<button[^>]*>([\s\S]*?)<\/button>/g)) {
    const body = button[1];
    const icon = body.search(/<(?:Loader2|RefreshCw|QrIcon|Trash2|Smartphone)\b/);
    const label = body.search(/\n\s*[A-Z][a-z]+(?: [a-z]+)*\s*\n/);
    if (icon >= 0 && label >= 0) assert.ok(icon < label, `icon after label in: ${body.trim().slice(0, 80)}`);
  }
});

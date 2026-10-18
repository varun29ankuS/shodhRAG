/**
 * Settings → General names the start-at-login switch for the system it runs on.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/startupText.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { desktopPlatform, startupText } from '../src/lib/startupText.ts';

test('navigator.platform values map to their system', () => {
  assert.equal(desktopPlatform('Win32'), 'windows');
  assert.equal(desktopPlatform('MacIntel'), 'macos');
  assert.equal(desktopPlatform('Linux x86_64'), 'linux');
  assert.equal(desktopPlatform(''), 'linux');
});

test('only Windows is told about Windows and Task Manager', () => {
  assert.match(startupText('windows').label, /Windows/);
  assert.match(startupText('windows').description, /Task Manager/);
  for (const platform of ['macos', 'linux'] as const) {
    const text = startupText(platform);
    assert.doesNotMatch(`${text.label} ${text.description}`, /Windows|Task Manager/);
    assert.match(text.label, /when you log in/);
  }
  assert.match(startupText('macos').description, /Login Items/);
});

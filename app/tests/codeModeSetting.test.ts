/**
 * Code mode is opt-in. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/codeModeSetting.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseAppSettings, parseCodePrefs } from '../src/lib/appSettings.ts';

const base = {
  preferences: { theme: 'dark', searchMaxResults: 8 },
  policy: { localOnly: false, webAccess: true },
};

test('Code mode reads as on only when the backend says so explicitly', () => {
  assert.deepEqual(parseCodePrefs({ enabled: true }), { enabled: true });
  for (const value of [undefined, null, {}, { enabled: false }, { enabled: 'true' }, { enabled: 1 }, []]) {
    assert.deepEqual(parseCodePrefs(value), { enabled: false }, JSON.stringify(value));
  }
  assert.equal(parseAppSettings(base)?.code.enabled, false);
  assert.equal(parseAppSettings({ ...base, code: { enabled: true } })?.code.enabled, true);
});

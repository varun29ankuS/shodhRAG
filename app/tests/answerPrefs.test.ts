/**
 * Answer preferences: "Think before answering" is off unless the user turns
 * it on, and survives the settings round trip. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/answerPrefs.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DEFAULT_ANSWER_PREFS, parseAnswerPrefs, parseAppSettings } from '../src/lib/appSettings.ts';

const base = {
  preferences: { theme: 'dark', searchMaxResults: 15 },
  policy: { localOnly: false, webAccess: true },
  seeded: true,
};

test('thinking before answering is off by default', () => {
  assert.equal(DEFAULT_ANSWER_PREFS.thinkBeforeAnswering, false);
  // Settings written before the answers section, or before the switch, existed.
  assert.deepEqual(parseAnswerPrefs(undefined), { autoRepair: true, thinkBeforeAnswering: false });
  assert.deepEqual(parseAnswerPrefs({ autoRepair: false }), { autoRepair: false, thinkBeforeAnswering: false });
  assert.equal(parseAppSettings(base)?.answers.thinkBeforeAnswering, false);
});

test('the switch is read back as saved and malformed values are refused', () => {
  const saved = parseAppSettings({ ...base, answers: { autoRepair: true, thinkBeforeAnswering: true } });
  assert.deepEqual(saved?.answers, { autoRepair: true, thinkBeforeAnswering: true });
  assert.equal(parseAnswerPrefs({ autoRepair: true, thinkBeforeAnswering: 'yes' }), null);
});

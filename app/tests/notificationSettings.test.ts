/**
 * Settings → Notifications: the switches' defaults and parsing, how the
 * panel shows them, and the test button's call. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/notificationSettings.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  DEFAULT_NOTIFICATION_PREFS,
  NOTIFICATION_KEYS,
  TEST_HELP,
  notificationRows,
  openSystemNotificationSettings,
  parseNotificationPrefs,
  saveNotificationPrefs,
  sendTestNotification,
  testMessage,
  withSwitch,
} from '../src/features/notifications/notificationPrefs.ts';
import type { Invoke } from '../src/features/notifications/notificationPrefs.ts';

/** Records every call; answers with `reply` (a rejection when it is an Error or a string). */
function recorder(reply?: unknown) {
  const calls: { command: string; args?: Record<string, unknown> }[] = [];
  const call: Invoke = async <T>(command: string, args?: Record<string, unknown>): Promise<T> => {
    calls.push(args === undefined ? { command } : { command, args });
    if (reply instanceof Error || typeof reply === 'string') throw reply;
    return reply as T;
  };
  return { calls, call };
}

test('everything is on by default except memory suggestions', () => {
  assert.deepEqual(DEFAULT_NOTIFICATION_PREFS, {
    reminders: true,
    needsYou: true,
    memorySuggestions: false,
    workFinished: true,
    problems: true,
  });
  // Settings written before notifications existed.
  assert.deepEqual(parseNotificationPrefs(undefined), DEFAULT_NOTIFICATION_PREFS);
});

test('saved switches are read back; missing ones get their default; malformed ones are refused', () => {
  assert.deepEqual(parseNotificationPrefs({ workFinished: false, memorySuggestions: true }), {
    ...DEFAULT_NOTIFICATION_PREFS,
    workFinished: false,
    memorySuggestions: true,
  });
  assert.equal(parseNotificationPrefs({ reminders: 'yes' }), null);
  assert.equal(parseNotificationPrefs(null), null);
  assert.equal(parseNotificationPrefs([true]), null);
  // The backend's camelCase names, all of them.
  assert.deepEqual([...NOTIFICATION_KEYS], ['reminders', 'needsYou', 'memorySuggestions', 'workFinished', 'problems']);
});

test('the panel shows one switch per category, memory suggestions under "Needs you"', () => {
  const rows = notificationRows(DEFAULT_NOTIFICATION_PREFS, false);
  assert.deepEqual(rows.map(r => r.key), ['reminders', 'needsYou', 'memorySuggestions', 'workFinished', 'problems']);
  assert.deepEqual(rows.map(r => r.checked), [true, true, false, true, true]);
  assert.ok(rows.every(r => !r.disabled));
  assert.deepEqual(rows.filter(r => r.nested).map(r => r.key), ['memorySuggestions']);
  assert.ok(rows.every(r => r.label.length > 0 && r.description.length > 0));
});

test('with "Needs you" off, memory suggestions are off and cannot be switched', () => {
  const prefs = withSwitch({ ...DEFAULT_NOTIFICATION_PREFS, memorySuggestions: true }, 'needsYou', false);
  const memory = notificationRows(prefs, false).find(r => r.key === 'memorySuggestions');
  assert.ok(memory);
  assert.equal(memory.checked, false);
  assert.equal(memory.disabled, true);
  assert.match(memory.description, /Turn on “Needs you”/);
  // The saved choice is kept for when "Needs you" is on again.
  assert.equal(prefs.memorySuggestions, true);
});

test('every switch is disabled while a change is saved', () => {
  assert.ok(notificationRows(DEFAULT_NOTIFICATION_PREFS, true).every(r => r.disabled));
});

test('a switch change saves all switches under `notifications`', async () => {
  const { calls, call } = recorder({});
  const next = withSwitch(DEFAULT_NOTIFICATION_PREFS, 'problems', false);
  assert.equal(DEFAULT_NOTIFICATION_PREFS.problems, true, 'the old value is not changed');
  await saveNotificationPrefs(call, next);
  assert.deepEqual(calls, [{ command: 'set_notification_preferences', args: { notifications: next } }]);
});

test('the test button calls send_test_notification and reports what happened', async () => {
  const ok = recorder(undefined);
  const sent = await sendTestNotification(ok.call);
  assert.deepEqual(ok.calls, [{ command: 'send_test_notification' }]);
  assert.deepEqual(sent, { sent: true });
  assert.match(testMessage(sent), /^Sent\./);

  const refused = recorder('The system did not accept the notification: no executable path');
  const failed = await sendTestNotification(refused.call);
  assert.deepEqual(failed, { sent: false, error: 'The system did not accept the notification: no executable path' });
  assert.equal(testMessage(failed), 'The test was not sent: The system did not accept the notification: no executable path');

  const thrown = await sendTestNotification(recorder(new Error('IPC closed')).call);
  assert.deepEqual(thrown, { sent: false, error: 'IPC closed' });
});

test('the help text says what a missing test notification means', () => {
  assert.equal(
    TEST_HELP,
    "If the test doesn't appear, your system is blocking Shodh's notifications. Open system settings and allow Shodh.",
  );
});

test('the system settings button asks the backend, which picks the page', async () => {
  const { calls, call } = recorder(undefined);
  await openSystemNotificationSettings(call);
  assert.deepEqual(calls, [{ command: 'open_notification_settings' }]);
});

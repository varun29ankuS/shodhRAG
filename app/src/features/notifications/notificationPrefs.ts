/**
 * Settings → Notifications: which desktop notifications Shodh shows
 * (`app_settings.rs::NotificationPrefs`, user-only; the assistant has no tool
 * to change them) and the panel's actions. No Tauri imports: the panel passes
 * the IPC function in, so this module runs under Node's test runner.
 */

/** One switch per category (`notifications.rs::Category`). */
export interface NotificationPrefs {
  /** Task reminders. */
  reminders: boolean;
  /** The assistant waits for an approval. */
  needsYou: boolean;
  /** New memory suggestions; part of "needs you". */
  memorySuggestions: boolean;
  /** An answer, a folder's indexing or sync, or a result extraction finished. */
  workFinished: boolean;
  /** Indexing or a sync failed, learning could not reach its model, a data file was restored. */
  problems: boolean;
}

export type NotificationKey = keyof NotificationPrefs;

export const NOTIFICATION_KEYS: readonly NotificationKey[] = [
  'reminders',
  'needsYou',
  'memorySuggestions',
  'workFinished',
  'problems',
];

/** The backend's defaults: everything on except memory suggestions. */
export const DEFAULT_NOTIFICATION_PREFS: NotificationPrefs = {
  reminders: true,
  needsYou: true,
  memorySuggestions: false,
  workFinished: true,
  problems: true,
};

/** Parse the notifications section; settings written before a switch existed get its default. */
export function parseNotificationPrefs(value: unknown): NotificationPrefs | null {
  if (value === undefined) return DEFAULT_NOTIFICATION_PREFS;
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return null;
  const raw = value as Record<string, unknown>;
  const prefs: NotificationPrefs = { ...DEFAULT_NOTIFICATION_PREFS };
  for (const key of NOTIFICATION_KEYS) {
    const switched = raw[key];
    if (switched === undefined) continue;
    if (typeof switched !== 'boolean') return null;
    prefs[key] = switched;
  }
  return prefs;
}

/** One switch as the panel shows it. */
export interface NotificationRow {
  key: NotificationKey;
  label: string;
  description: string;
  checked: boolean;
  disabled: boolean;
  /** Shown indented under the row it belongs to. */
  nested: boolean;
}

const ROWS: readonly { key: NotificationKey; label: string; description: string }[] = [
  {
    key: 'reminders',
    label: 'Task reminders',
    description: 'When a task’s reminder rings, and the reminders missed while Shodh was closed.',
  },
  {
    key: 'needsYou',
    label: 'Needs you',
    description: 'When the assistant waits for you to approve an action. The notification names the conversation.',
  },
  {
    key: 'memorySuggestions',
    label: 'Memory suggestions',
    description: 'When Shodh suggests something to remember about you. Only the number of suggestions is shown.',
  },
  {
    key: 'workFinished',
    label: 'Work finished',
    description:
      'When an answer is ready, a folder finished indexing or syncing, or results were extracted from a paper. At most one a minute; more are grouped.',
  },
  {
    key: 'problems',
    label: 'Problems',
    description:
      'When indexing or a sync failed, learning could not reach its model, or a data file had to be restored from its backup.',
  },
];

/** The switches for `prefs`; all are disabled while a change is being saved. */
export function notificationRows(prefs: NotificationPrefs, saving: boolean): NotificationRow[] {
  return ROWS.map(row => {
    const nested = row.key === 'memorySuggestions';
    const parentOff = nested && !prefs.needsYou;
    return {
      ...row,
      description: parentOff ? `${row.description} Turn on “Needs you” to use this.` : row.description,
      checked: prefs[row.key] && !parentOff,
      disabled: saving || parentOff,
      nested,
    };
  });
}

/** `prefs` with one switch changed. */
export function withSwitch(prefs: NotificationPrefs, key: NotificationKey, on: boolean): NotificationPrefs {
  return { ...prefs, [key]: on };
}

/** Shown under the test button, always. */
export const TEST_HELP =
  "If the test doesn't appear, your system is blocking Shodh's notifications. Open system settings and allow Shodh.";

/** The IPC call the panel passes in (Tauri's `invoke`). */
export type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

export type TestResult = { sent: true } | { sent: false; error: string };

function errorText(err: unknown): string {
  if (typeof err === 'string') return err;
  if (err instanceof Error) return err.message;
  return String(err);
}

/** Saves the switches; resolves to the backend's settings payload. */
export function saveNotificationPrefs(call: Invoke, prefs: NotificationPrefs): Promise<unknown> {
  return call<unknown>('set_notification_preferences', { notifications: prefs });
}

/**
 * Sends the test notification. It is shown even while Shodh is in front and
 * whatever the switches say; `sent` means the system took it, not that it
 * was displayed (the system can still block it, see `TEST_HELP`).
 */
export async function sendTestNotification(call: Invoke): Promise<TestResult> {
  try {
    await call<void>('send_test_notification');
    return { sent: true };
  } catch (err) {
    return { sent: false, error: errorText(err) };
  }
}

/** What the panel says after a test. */
export function testMessage(result: TestResult): string {
  return result.sent
    ? 'Sent. It should appear in the corner of your screen or in the notification centre.'
    : `The test was not sent: ${result.error}`;
}

/** Opens the system's notification settings (only on the user's click). */
export function openSystemNotificationSettings(call: Invoke): Promise<void> {
  return call<void>('open_notification_settings');
}

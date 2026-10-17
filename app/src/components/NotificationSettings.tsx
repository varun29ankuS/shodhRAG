import React, { useEffect, useId, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { cn } from '../lib/utils';
import { notify } from '../lib/notify';
import { getAppSettings, onAppSettingsChanged, setNotificationPreferences } from '../lib/appSettings';
import { SwitchRow } from './PrivacySettings';
import {
  TEST_HELP,
  TEST_PENDING,
  notificationRows,
  openSystemNotificationSettings,
  sendTestNotification,
  testMessage,
  withSwitch,
} from '../features/notifications/notificationPrefs';
import type { NotificationKey, NotificationPrefs } from '../features/notifications/notificationPrefs';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const BUTTON = cn(
  'h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border bg-shodh-surface text-[12.5px] text-shodh-text',
  'hover:bg-shodh-raised disabled:opacity-50 disabled:cursor-not-allowed aria-disabled:opacity-50 aria-disabled:cursor-not-allowed transition-colors duration-micro',
  FOCUS_RING,
);

/**
 * Settings → Notifications: which desktop notifications Shodh shows while its
 * window is not in front, a test, and the way to the system's own settings.
 * Only the user can change these (the assistant has no tool for them).
 */
export default function NotificationSettings() {
  const [prefs, setPrefs] = useState<NotificationPrefs | null>(null);
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const helpId = useId();

  useEffect(() => {
    let cancelled = false;
    getAppSettings()
      .then(settings => {
        if (!cancelled && settings) setPrefs(settings.notifications);
      })
      .catch(err => notify.error('Notification settings could not be loaded', { description: String(err) }));
    const unsubscribe = onAppSettingsChanged(settings => setPrefs(settings.notifications));
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  const change = async (key: NotificationKey, on: boolean) => {
    if (!prefs) return;
    setSaving(true);
    try {
      const saved = await setNotificationPreferences(withSwitch(prefs, key, on));
      if (saved) setPrefs(saved.notifications);
    } catch (err) {
      notify.error('The notification setting was not saved', { description: String(err) });
    } finally {
      setSaving(false);
    }
  };

  const test = async () => {
    // aria-disabled keeps the button focusable while the test waits; clicks then do nothing.
    if (testing) return;
    setTesting(true);
    setResult(TEST_PENDING);
    try {
      setResult(testMessage(await sendTestNotification(invoke)));
    } finally {
      setTesting(false);
    }
  };

  const openSystemSettings = () => {
    openSystemNotificationSettings(invoke).catch(err =>
      notify.error('The system notification settings could not be opened', { description: String(err) }),
    );
  };

  if (!prefs) {
    return <p className="m-0 text-[13px] text-shodh-text-muted">Loading…</p>;
  }

  return (
    <div className="flex flex-col gap-5">
      <p className="m-0 text-[12.5px] text-shodh-text-muted">
        Shodh shows a desktop notification only while its window is minimised, in the tray or behind another window;
        while you look at it, the Inbox and the messages in the window say the same. Notifications name a task, a folder,
        a paper or a conversation, never what your documents or answers say.
      </p>
      {notificationRows(prefs, saving).map(row => (
        <div key={row.key} className={cn(row.nested && 'pl-6')}>
          <SwitchRow
            label={row.label}
            description={row.description}
            checked={row.checked}
            disabled={row.disabled}
            onChange={on => void change(row.key, on)}
          />
        </div>
      ))}
      <div className="flex flex-col gap-3 pt-5 border-t border-shodh-border">
        <div className="flex flex-wrap items-center gap-3">
          <button
            type="button"
            className={BUTTON}
            onClick={() => void test()}
            aria-disabled={testing}
            aria-describedby={helpId}
          >
            Send a test notification
          </button>
          <button type="button" className={BUTTON} onClick={openSystemSettings} aria-describedby={helpId}>
            Open system notification settings
          </button>
        </div>
        <p id={helpId} className="m-0 text-[12.5px] text-shodh-text-muted">{TEST_HELP}</p>
        {/* Always present, so screen readers hear each new message. */}
        <p className="m-0 text-[12.5px] text-shodh-text-secondary" role="status" aria-live="polite">
          {result ?? ''}
        </p>
      </div>
    </div>
  );
}

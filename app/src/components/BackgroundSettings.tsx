import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { notify } from '../lib/notify';
import { onAppSettingsChanged } from '../lib/appSettings';
import { SwitchRow } from './PrivacySettings';

/** `background.rs::BackgroundStatus`. */
interface BackgroundStatus {
  closeToTray: boolean;
  startWithWindows: boolean;
  paused: boolean;
}

function isStatus(value: unknown): value is BackgroundStatus {
  if (typeof value !== 'object' || value === null) return false;
  const v = value as Record<string, unknown>;
  return typeof v.closeToTray === 'boolean' && typeof v.startWithWindows === 'boolean' && typeof v.paused === 'boolean';
}

/**
 * Settings → General: how Shodh runs when its window is closed. Only the
 * user can change these (the assistant has no tool for them).
 */
export default function BackgroundSettings() {
  const [status, setStatus] = useState<BackgroundStatus | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const load = () => {
      invoke<unknown>('get_background_status')
        .then(next => { if (!cancelled && isStatus(next)) setStatus(next); })
        .catch(err => notify.error('Background settings could not be loaded', { description: String(err) }));
    };
    load();
    // Closing the window the first time records that it was explained.
    const unsubscribe = onAppSettingsChanged(load);
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  const save = async (run: () => Promise<unknown>, failure: string) => {
    setSaving(true);
    try {
      const next = await run();
      if (isStatus(next)) setStatus(next);
    } catch (err) {
      notify.error(failure, { description: String(err) });
    } finally {
      setSaving(false);
    }
  };

  if (!status) {
    return <p className="m-0 text-[13px] text-shodh-text-muted">Loading…</p>;
  }

  return (
    <div className="flex flex-col gap-5">
      <SwitchRow
        label="Keep running in the tray when the window is closed"
        description="Task reminders ring only while Shodh runs. With this on, closing the window hides it to the tray; quit from the tray icon's menu."
        checked={status.closeToTray}
        disabled={saving}
        onChange={enabled => void save(() => invoke('set_close_to_tray', { enabled }), 'The setting was not saved')}
      />
      <SwitchRow
        label="Start Shodh when Windows starts"
        description="Starts hidden in the tray so reminders ring after a restart. Windows can also turn this off in Task Manager → Startup apps."
        checked={status.startWithWindows}
        disabled={saving}
        onChange={enabled => void save(() => invoke('set_start_with_windows', { enabled }), 'The startup setting was not changed')}
      />
      {status.paused && (
        <p className="m-0 text-[12.5px] text-shodh-warning" role="status">
          Background work is paused from the tray: indexing the assistant starts waits until you resume it there.
        </p>
      )}
    </div>
  );
}

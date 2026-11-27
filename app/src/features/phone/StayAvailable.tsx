import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { SwitchRow } from '../../components/PrivacySettings';
import { Callout, Skeleton } from '../connectors/ui';
import { errorText, PHONE_STAY_CHANGED, type StayStatus, stayWords } from './phoneModel';

/**
 * Settings → Phone: "Stay available for my phone". While it is on and this computer is
 * plugged in, Shodh keeps it from going to sleep, so the phone can reach it; on battery it
 * sleeps as usual. Only the user turns it on: the assistant and phones cannot.
 */
export default function StayAvailable() {
  const [status, setStatus] = useState<StayStatus | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    invoke<StayStatus>('phone_stay_available')
      .then(next => { setStatus(next); setError(null); })
      .catch(err => setError(errorText(err)));
  }, []);

  useEffect(() => { load(); }, [load]);

  // Plugging in, unplugging, pairing and Local-only mode change what it does.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<StayStatus>(PHONE_STAY_CHANGED, event => setStatus(event.payload))
      .then(fn => { if (disposed) fn(); else unlisten = fn; })
      .catch(err => setError(errorText(err)));
    return () => { disposed = true; unlisten?.(); };
  }, []);

  const change = (enabled: boolean) => {
    setSaving(true);
    setError(null);
    invoke<StayStatus>('phone_stay_available_set', { enabled })
      .then(setStatus)
      .catch(err => setError(errorText(err)))
      .finally(() => setSaving(false));
  };

  return (
    <div className="flex flex-col gap-2">
      {status === null && !error ? (
        <Skeleton className="h-12 w-full" />
      ) : (
        <SwitchRow
          label="Stay available for my phone"
          description={'Keeps this computer awake on power.'}
          checked={status?.enabled ?? false}
          disabled={saving || status === null || !status.supported}
          onChange={change}
        />
      )}
      {status && <p className="m-0 text-small text-shodh-text-secondary" aria-live="polite">{stayWords(status)}</p>}
      {error && (
        <Callout tone="error" role="alert">
          <p className="m-0">This setting could not be changed: {error}</p>
        </Callout>
      )}
    </div>
  );
}

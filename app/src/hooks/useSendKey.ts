import { useCallback, useEffect, useState } from 'react';
import { getAppSettings, onAppSettingsChanged, updatePreferences } from '../lib/appSettings';
import { notify } from '../lib/notify';
import { DEFAULT_SEND_KEY } from '../lib/sendKey';
import type { SendKey } from '../lib/sendKey';

export interface SendKeyHandle {
  /** The key being saved, else the saved one (Enter until read). */
  sendKey: SendKey;
  setSendKey: (key: SendKey) => void;
  saving: boolean;
}

/**
 * Which key sends from the composer (Settings → General), kept by the backend's settings
 * and current in every window. A save that fails goes back to the saved key and says why.
 */
export function useSendKey(): SendKeyHandle {
  const [saved, setSaved] = useState<SendKey | null>(null);
  const [pending, setPending] = useState<SendKey | null>(null);

  useEffect(() => {
    let cancelled = false;
    getAppSettings()
      .then(settings => {
        if (!cancelled && settings) setSaved(settings.preferences.sendWith);
      })
      .catch(err => console.error('Send key setting:', err));
    const unsubscribe = onAppSettingsChanged(settings => setSaved(settings.preferences.sendWith));
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  const setSendKey = useCallback((next: SendKey) => {
    setPending(next);
    updatePreferences({ sendWith: next })
      .then(result => {
        if (result) setSaved(result.preferences.sendWith);
      })
      .catch(err => notify.error('The send key was not changed', { description: String(err) }))
      .finally(() => setPending(null));
  }, []);

  return { sendKey: pending ?? saved ?? DEFAULT_SEND_KEY, setSendKey, saving: pending !== null };
}

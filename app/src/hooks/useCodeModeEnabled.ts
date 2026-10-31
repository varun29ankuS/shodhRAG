import { useEffect, useState } from 'react';
import { getAppSettings, onAppSettingsChanged } from '../lib/appSettings';

/**
 * Whether Code mode is turned on in Settings → Privacy (off on a new install).
 * Null until the settings are read: nothing is shown or blocked on a guess (the
 * backend refuses Code work while it is off either way).
 */
export function useCodeModeEnabled(): boolean | null {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  useEffect(() => {
    let cancelled = false;
    getAppSettings()
      .then(settings => {
        if (!cancelled && settings) setEnabled(settings.code.enabled);
      })
      .catch(err => console.error('Code mode setting:', err));
    const unsubscribe = onAppSettingsChanged(settings => setEnabled(settings.code.enabled));
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);
  return enabled;
}

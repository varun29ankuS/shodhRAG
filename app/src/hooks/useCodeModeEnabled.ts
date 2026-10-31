import { useEffect, useState } from 'react';
import { getAppSettings, onAppSettingsChanged } from '../lib/appSettings';

/**
 * Whether Code mode is turned on in Settings → Privacy (off on a new install).
 * False until the settings are read, so the Code switch never flashes up for a
 * user who has not turned it on.
 */
export function useCodeModeEnabled(): boolean {
  const [enabled, setEnabled] = useState(false);
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

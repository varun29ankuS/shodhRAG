import { useEffect, useState } from 'react';
import { getAppSettings, onAppSettingsChanged } from '../lib/appSettings';
import { DEFAULT_AGENT_MODE } from '../features/agent/agentModes';
import type { AgentMode } from '../features/agent/agentModes';

/**
 * The mode new conversations answer in (Settings → Privacy), kept current. Null until
 * the settings are read, so no mode is shown or sent on a guess; Ask when they cannot be
 * read (the mode that asks for every change).
 */
export function useDefaultAgentMode(): AgentMode | null {
  const [mode, setMode] = useState<AgentMode | null>(null);
  useEffect(() => {
    let cancelled = false;
    getAppSettings()
      .then(settings => {
        if (!cancelled) setMode(settings?.agent.defaultMode ?? DEFAULT_AGENT_MODE);
      })
      .catch(err => {
        console.error('Default mode setting:', err);
        if (!cancelled) setMode(DEFAULT_AGENT_MODE);
      });
    const unsubscribe = onAppSettingsChanged(settings => setMode(settings.agent.defaultMode));
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);
  return mode;
}

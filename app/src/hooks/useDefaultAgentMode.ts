import { useEffect, useState } from 'react';
import { getAppSettings, onAppSettingsChanged } from '../lib/appSettings';
import { DEFAULT_AGENT_MODE } from '../features/agent/agentModes';
import type { AgentMode } from '../features/agent/agentModes';

/**
 * The mode new conversations answer in (Settings → Privacy), kept current. Ask until
 * the settings are read, the mode that asks for every change; the backend falls back
 * to the same saved default for a conversation without a mode of its own.
 */
export function useDefaultAgentMode(): AgentMode {
  const [mode, setMode] = useState<AgentMode>(DEFAULT_AGENT_MODE);
  useEffect(() => {
    let cancelled = false;
    getAppSettings()
      .then(settings => {
        if (!cancelled && settings) setMode(settings.agent.defaultMode);
      })
      .catch(err => console.error('Default mode setting:', err));
    const unsubscribe = onAppSettingsChanged(settings => setMode(settings.agent.defaultMode));
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);
  return mode;
}

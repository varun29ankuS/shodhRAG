import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { agentApi, toAgentError, useRuntimeProgress } from '../agent/useAgentSession';
import type { RuntimeInstall, RuntimeProgress, RuntimeStatus } from '../agent/useAgentSession';

interface RuntimeSetupValue {
  /** Latest status, or null until the first check finishes. */
  status: RuntimeStatus | null;
  statusError: string | null;
  /**
   * Whether the answer engine is on this computer (null: not known yet). A start
   * that failed because it is missing sets it false (`setInstalled`).
   */
  installed: boolean | null;
  installing: boolean;
  progress: RuntimeProgress | null;
  installError: string | null;
  /** The last install could not connect. */
  installOffline: boolean;
  /** Result of an install that finished in this session. */
  justInstalled: RuntimeInstall | null;
  /** When that install finished (ms), so a card shown after it is not covered by it. */
  installedAt: number | null;
  dismissInstalled: () => void;
  /** Download and verify it; resolves whether it is installed afterwards. */
  install: () => Promise<boolean>;
  setInstalled: (installed: boolean) => void;
  refresh: () => Promise<void>;
}

const RuntimeSetupContext = createContext<RuntimeSetupValue | null>(null);

/**
 * One shared view of the answer engine (the pinned agent runtime): first-run
 * setup, Ask, the dock and the runtime card all show the same install, and only
 * one download runs (the backend also returns a copy already in place).
 */
export function RuntimeSetupProvider({ children }: { children: React.ReactNode }) {
  const [status, setStatus] = useState<RuntimeStatus | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [installed, setInstalledState] = useState<boolean | null>(null);
  const [installing, setInstalling] = useState(false);
  const [progress, setProgress] = useState<RuntimeProgress | null>(null);
  const [installError, setInstallError] = useState<string | null>(null);
  const [installOffline, setInstallOffline] = useState(false);
  const [justInstalled, setJustInstalled] = useState<RuntimeInstall | null>(null);
  const [installedAt, setInstalledAt] = useState<number | null>(null);
  const installingRef = useRef<Promise<boolean> | null>(null);

  const refresh = useCallback(async () => {
    try {
      const next = await agentApi.runtimeStatus();
      setStatus(next);
      setStatusError(null);
      setInstalledState(next.installed);
    } catch (error) {
      setStatusError(toAgentError(error).message);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useRuntimeProgress(next => {
    if (installingRef.current) setProgress(next);
  });

  const install = useCallback((): Promise<boolean> => {
    if (installingRef.current) return installingRef.current;
    setInstalling(true);
    setInstallError(null);
    setInstallOffline(false);
    setProgress(null);
    const run = (async () => {
      try {
        const result = await agentApi.installRuntime();
        setJustInstalled(result);
        setInstalledAt(Date.now());
        setInstalledState(true);
        setStatus(prev => (prev ? { ...prev, installed: true } : prev));
        return true;
      } catch (error) {
        const failure = toAgentError(error);
        setInstallError(failure.message);
        setInstallOffline(failure.code === 'offline');
        await refresh();
        return false;
      } finally {
        installingRef.current = null;
        setInstalling(false);
      }
    })();
    installingRef.current = run;
    return run;
  }, [refresh]);

  const setInstalled = useCallback((next: boolean) => {
    setInstalledState(next);
    // Gone again (removed, or quarantined by antivirus): the earlier install no longer counts.
    if (!next) {
      setJustInstalled(null);
      setInstalledAt(null);
    }
    setStatus(prev => (prev ? { ...prev, installed: next } : prev));
  }, []);

  const dismissInstalled = useCallback(() => {
    setJustInstalled(null);
    setInstalledAt(null);
  }, []);

  const value = useMemo<RuntimeSetupValue>(
    () => ({ status, statusError, installed, installing, progress, installError, installOffline, justInstalled, installedAt, dismissInstalled, install, setInstalled, refresh }),
    [status, statusError, installed, installing, progress, installError, installOffline, justInstalled, installedAt, dismissInstalled, install, setInstalled, refresh],
  );

  return <RuntimeSetupContext.Provider value={value}>{children}</RuntimeSetupContext.Provider>;
}

export function useRuntimeSetup(): RuntimeSetupValue {
  const context = useContext(RuntimeSetupContext);
  if (!context) throw new Error('useRuntimeSetup must be used within a RuntimeSetupProvider');
  return context;
}

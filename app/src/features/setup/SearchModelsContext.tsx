import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import {
  INSTALL_IN_PROGRESS_CODE,
  SEARCH_MODELS_PROGRESS_EVENT,
  SEARCH_MODELS_READY_EVENT,
  errorMessage,
  isInstallProgress,
  isSearchModelsStatus,
} from './searchModels';
import type { InstallProgress, SearchModelsStatus } from './searchModels';

interface SearchModelsContextValue {
  /** Latest status, or null until the first check finishes. */
  status: SearchModelsStatus | null;
  /** The status check itself failed. */
  statusError: string | null;
  installing: boolean;
  progress: InstallProgress | null;
  installError: string | null;
  /** Status returned by an install that finished in this session. */
  installed: SearchModelsStatus | null;
  install: () => Promise<void>;
  refresh: () => Promise<void>;
  dismissInstalled: () => void;
}

const SearchModelsContext = createContext<SearchModelsContextValue | null>(null);

/**
 * One shared view of the search model setup, so every setup card (Ask,
 * Library) shows the same install and only one install runs at a time.
 */
export function SearchModelsProvider({ children }: { children: React.ReactNode }) {
  const [status, setStatus] = useState<SearchModelsStatus | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);
  const [progress, setProgress] = useState<InstallProgress | null>(null);
  const [installError, setInstallError] = useState<string | null>(null);
  const [installed, setInstalled] = useState<SearchModelsStatus | null>(null);
  const installingRef = useRef(false);

  const refresh = useCallback(async () => {
    try {
      const next = await invoke<unknown>('search_models_status');
      if (!isSearchModelsStatus(next)) throw new Error('Unexpected search setup status');
      setStatus(next);
      setStatusError(null);
      // An install started earlier (e.g. before a reload) is still running.
      if (next.installing && !installingRef.current) setInstalling(true);
      if (!next.installing && !installingRef.current) setInstalling(false);
    } catch (error) {
      setStatusError(errorMessage(error));
    }
  }, []);

  // Subscribe to progress before any install can start.
  useEffect(() => {
    let disposed = false;
    const unlisteners: UnlistenFn[] = [];
    const subscribe = (event: string, handler: (payload: unknown) => void) => {
      listen<unknown>(event, e => handler(e.payload))
        .then(fn => {
          if (disposed) fn();
          else unlisteners.push(fn);
        })
        .catch(err => console.error(`Failed to listen for ${event}:`, err));
    };
    subscribe(SEARCH_MODELS_PROGRESS_EVENT, payload => {
      if (isInstallProgress(payload)) setProgress(payload);
    });
    subscribe(SEARCH_MODELS_READY_EVENT, () => {
      void refresh();
    });
    void refresh();
    return () => {
      disposed = true;
      unlisteners.forEach(fn => fn());
    };
  }, [refresh]);

  // An install this view did not start (another window, or before a reload)
  // reports success through the ready event; poll so a failure also ends it.
  useEffect(() => {
    if (!installing || installingRef.current) return;
    const timer = window.setInterval(() => void refresh(), 3000);
    return () => window.clearInterval(timer);
  }, [installing, refresh]);

  const install = useCallback(async () => {
    if (installingRef.current) return;
    installingRef.current = true;
    setInstalling(true);
    setInstallError(null);
    setProgress(null);
    try {
      const result = await invoke<unknown>('install_search_models');
      if (!isSearchModelsStatus(result)) throw new Error('Unexpected search setup result');
      setStatus(result);
      setInstalled(result);
    } catch (error) {
      const raw = error instanceof Error ? error.message : String(error);
      if (!raw.startsWith(INSTALL_IN_PROGRESS_CODE)) setInstallError(errorMessage(error));
      await refresh();
    } finally {
      installingRef.current = false;
      setInstalling(false);
    }
  }, [refresh]);

  const dismissInstalled = useCallback(() => setInstalled(null), []);

  const value = useMemo<SearchModelsContextValue>(
    () => ({ status, statusError, installing, progress, installError, installed, install, refresh, dismissInstalled }),
    [status, statusError, installing, progress, installError, installed, install, refresh, dismissInstalled],
  );

  return <SearchModelsContext.Provider value={value}>{children}</SearchModelsContext.Provider>;
}

export function useSearchModels(): SearchModelsContextValue {
  const context = useContext(SearchModelsContext);
  if (!context) throw new Error('useSearchModels must be used within a SearchModelsProvider');
  return context;
}

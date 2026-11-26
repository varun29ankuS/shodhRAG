import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { MODEL_CHANGED_EVENT } from '../modelPicker/modelApi';
import type { ProviderId } from '../modelPicker/modelTypes';
import type { ConnectionSnapshot } from './connectionStatus';
import { CONNECTION_STATUS_EVENT, readSnapshot } from './connectionStatus';

export interface ConnectionSnapshotHandle {
  snapshot: ConnectionSnapshot | null;
  /** Why the status could not be read, or null. */
  loadError: string | null;
  /** Check one provider now (all of them with null); never spends a chat request. */
  check: (provider: ProviderId | null) => Promise<void>;
}

/**
 * The model status as the backend knows it (`connection_health.rs`): read on mount, on every
 * change the backend sends, and when the model changes anywhere in the app. The sidebar's
 * model status and the composer's model control both read it here, so they show one state.
 */
export function useConnectionSnapshot(): ConnectionSnapshotHandle {
  const [snapshot, setSnapshot] = useState<ConnectionSnapshot | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const next = readSnapshot(await invoke<unknown>('connection_status'));
      if (next) setSnapshot(next);
      setLoadError(null);
    } catch (error) {
      setLoadError(error instanceof Error ? error.message : String(error));
    }
  }, []);

  // The backend sends every change; a model change elsewhere re-reads it.
  useEffect(() => {
    void load();
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<unknown>(CONNECTION_STATUS_EVENT, event => {
      const next = readSnapshot(event.payload);
      if (next) setSnapshot(next);
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(error => console.error('Connection status listener failed:', error));
    const onModelChanged = () => void load();
    window.addEventListener(MODEL_CHANGED_EVENT, onModelChanged);
    return () => {
      disposed = true;
      if (unlisten) unlisten();
      window.removeEventListener(MODEL_CHANGED_EVENT, onModelChanged);
    };
  }, [load]);

  const check = useCallback(async (provider: ProviderId | null) => {
    try {
      const next = readSnapshot(await invoke<unknown>('connection_check', { provider }));
      if (next) setSnapshot(next);
    } catch (error) {
      setLoadError(error instanceof Error ? error.message : String(error));
    }
  }, []);

  return { snapshot, loadError, check };
}

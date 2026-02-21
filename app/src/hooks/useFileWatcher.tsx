import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

interface FileChangeEvent {
  path: string;
  change_type: string;
  space_id?: string;
  timestamp: string;
}

interface UseFileWatcherOptions {
  path?: string;
  spaceId?: string;
  enabled?: boolean;
  onFileChange?: (event: FileChangeEvent) => void;
}

export function useFileWatcher({
  path,
  spaceId,
  enabled = true,
  onFileChange
}: UseFileWatcherOptions = {}) {
  const [isWatching, setIsWatching] = useState(false);
  const [lastChange, setLastChange] = useState<FileChangeEvent | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!path || !enabled) return;

    let unlisten: (() => void) | null = null;

    const startWatching = async () => {
      try {
        // Start file watcher
        await invoke('start_watching_folder', {
          path,
          spaceId: spaceId || null
        });

        setIsWatching(true);
        setError(null);

        // Listen for file change events
        unlisten = await listen<FileChangeEvent>('file-change', (event) => {
          const changeEvent = event.payload;
          setLastChange(changeEvent);

          // Show toast notification
          console.log(`File ${changeEvent.change_type}: ${changeEvent.path}`);

          // Call custom handler
          onFileChange?.(changeEvent);
        });

      } catch (err) {
        console.error('Failed to start file watcher:', err);
        setError(String(err));
        setIsWatching(false);
      }
    };

    startWatching();

    // Cleanup
    return () => {
      if (unlisten) {
        unlisten();
      }

      if (path) {
        invoke('stop_watching_folder', { path }).catch(console.error);
      }

      setIsWatching(false);
    };
  }, [path, spaceId, enabled]);

  const manualRefresh = async () => {
    if (!path) return;

    try {
      // Re-index the entire workspace
      await invoke('index_workspace', { path, spaceId });
    } catch (err) {
      console.error('Manual refresh failed:', err);
      setError(String(err));
    }
  };

  return {
    isWatching,
    lastChange,
    error,
    manualRefresh
  };
}

import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { notify } from '../lib/notify';
import { addBreadcrumb } from '../lib/errorReporting';
import {
  UPDATE_AVAILABLE,
  UPDATE_PROGRESS,
  installUpdate,
  progressPercent,
  type UpdateInfo,
  type UpdateProgress,
} from '../lib/updates';

/**
 * The update the backend announced (`update-available`, from its quiet check
 * or from Settings → Privacy → Check now) and the install the user starts.
 */
export function useUpdater() {
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [downloading, setDownloading] = useState(false);
  const [progress, setProgress] = useState<UpdateProgress | null>(null);

  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    const keep = (unlisten: () => void) => {
      if (disposed) unlisten();
      else unlisteners.push(unlisten);
    };
    listen<UpdateInfo>(UPDATE_AVAILABLE, event => {
      setUpdateInfo(event.payload);
      addBreadcrumb('Update available', 'update', {
        version: event.payload.version,
        currentVersion: event.payload.currentVersion,
      });
    }).then(keep, err => console.error('Listening for updates failed:', err));
    listen<UpdateProgress>(UPDATE_PROGRESS, event => setProgress(event.payload)).then(keep, err =>
      console.error('Listening for update progress failed:', err),
    );
    return () => {
      disposed = true;
      unlisteners.forEach(unlisten => unlisten());
    };
  }, []);

  async function downloadAndInstall() {
    if (!updateInfo || downloading) return;
    setDownloading(true);
    setProgress(null);
    addBreadcrumb('Installing update', 'update', { version: updateInfo.version });
    try {
      // Restarts into the new version when it succeeds.
      await installUpdate();
    } catch (error) {
      notify.error('The update was not installed', { description: String(error) });
      setDownloading(false);
      setProgress(null);
    }
  }

  function dismissUpdate() {
    if (!downloading) setUpdateInfo(null);
  }

  return {
    updateAvailable: updateInfo !== null,
    updateInfo,
    downloading,
    downloadProgress: progressPercent(progress),
    downloadAndInstall,
    dismissUpdate,
  };
}

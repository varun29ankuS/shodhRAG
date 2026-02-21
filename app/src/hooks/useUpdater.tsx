import { useEffect, useState } from 'react';
import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { toast } from 'sonner';
import { notify } from '../lib/notify';
import { captureError, addBreadcrumb } from '../lib/errorReporting';

export interface UpdateInfo {
  version: string;
  date?: string;
  body?: string;
  currentVersion: string;
}

export function useUpdater() {
  const [updateAvailable, setUpdateAvailable] = useState(false);
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [downloading, setDownloading] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState(0);
  const [checking, setChecking] = useState(false);

  useEffect(() => {
    checkForUpdates();

    const interval = setInterval(() => {
      checkForUpdates(true);
    }, 1000 * 60 * 60); // Check every hour

    return () => clearInterval(interval);
  }, []);

  async function checkForUpdates(silent: boolean = false) {
    if (checking) return;

    setChecking(true);
    addBreadcrumb('Checking for updates', 'update', { silent });

    try {
      const update = await check();

      if (update) {
        const info: UpdateInfo = {
          version: update.version,
          date: update.date,
          body: update.body,
          currentVersion: update.currentVersion,
        };

        setUpdateInfo(info);
        setUpdateAvailable(true);

        if (!silent) {
          toast.info(`Update ${update.version} is available!`, {
            duration: 10000,
            action: {
              label: 'Download',
              onClick: () => downloadAndInstall(),
            },
          });
        }

        addBreadcrumb('Update available', 'update', {
          version: update.version,
          currentVersion: update.currentVersion,
        });
      } else if (!silent) {
        notify.success('You are on the latest version');
      }
    } catch (error) {
      // Silently fail if updater is disabled/not configured
      if (!silent) {
        console.log('Update check unavailable (updater disabled or not configured)');
      }
    } finally {
      setChecking(false);
    }
  }

  async function downloadAndInstall() {
    if (!updateInfo) return;

    setDownloading(true);
    setDownloadProgress(0);

    addBreadcrumb('Downloading update', 'update', { version: updateInfo.version });

    try {
      const update = await check();

      if (!update) {
        notify.error('Update no longer available');
        return;
      }

      let downloaded = 0;
      let contentLength = 0;

      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case 'Started':
            contentLength = event.data.contentLength || 0;
            notify.info('Downloading update...');
            break;
          case 'Progress':
            downloaded += event.data.chunkLength;
            const progress = contentLength > 0 ? (downloaded / contentLength) * 100 : 0;
            setDownloadProgress(progress);
            break;
          case 'Finished':
            setDownloadProgress(100);
            notify.success('Update downloaded! Restarting...');
            addBreadcrumb('Update installed', 'update', { version: updateInfo.version });

            setTimeout(async () => {
              await relaunch();
            }, 2000);
            break;
        }
      });
    } catch (error) {
      console.error('Failed to download update:', error);
      captureError(error as Error, { context: 'update_download' });
      notify.error(`Failed to download update: ${(error as Error).message}`);
      setDownloading(false);
      setDownloadProgress(0);
    }
  }

  function dismissUpdate() {
    setUpdateAvailable(false);
    setUpdateInfo(null);
  }

  return {
    updateAvailable,
    updateInfo,
    downloading,
    downloadProgress,
    checking,
    checkForUpdates,
    downloadAndInstall,
    dismissUpdate,
  };
}

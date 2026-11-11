import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

/** A file (or hidden folder) of a linked folder that is there but not indexed (`folder_sync::SkippedFile`). */
export interface SkippedFile {
  file: string;
  kind: 'secret' | 'unsupported' | 'hidden' | 'temporary';
  reason: string;
}

/** Every skipped entry counted, the first ones listed (`folder_sync::SkippedFiles`). */
export interface SkippedFiles {
  total: number;
  files: SkippedFile[];
}

const NONE: SkippedFiles = { total: 0, files: [] };

/**
 * The files of the folder at `path` that are not indexed, and why: read when the
 * folder is ready and again after each change to it (`version`, e.g. when it was
 * last indexed or synced). Nothing while it is being indexed.
 */
export function useSkippedFiles(path: string, ready: boolean, version: string | undefined): SkippedFiles {
  const [skipped, setSkipped] = useState<SkippedFiles>(NONE);
  useEffect(() => {
    if (!ready) return;
    let current = true;
    invoke<SkippedFiles>('library_skipped_files', { folderPath: path })
      .then(found => {
        if (current) setSkipped(found);
      })
      .catch(error => {
        // The folder moved or cannot be read: its sync says so; nothing is listed here.
        console.warn('Skipped files not read:', error);
        if (current) setSkipped(NONE);
      });
    return () => {
      current = false;
    };
  }, [path, ready, version]);
  return ready ? skipped : NONE;
}

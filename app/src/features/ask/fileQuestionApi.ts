/**
 * The index check behind "Ask about this file" (`features/ask/fileQuestion`): whether the
 * file a question is about is still indexed, asked before the question is sent or retried.
 */
import { invoke } from '@tauri-apps/api/core';
import { notify } from '../../lib/notify';
import { missingFileNotice, missingFiles } from './fileQuestion';

/** Whether the index holds `path` (`indexed_file_exists`). */
export function isIndexedFile(path: string): Promise<boolean> {
  return invoke<boolean>('indexed_file_exists', { path });
}

/** Whether every one of `paths` is still indexed; the paths that are not. */
export function missingIndexedFiles(paths: readonly string[]): Promise<string[]> {
  return missingFiles(paths, isIndexedFile);
}

/**
 * Tell the user a file a question is about is gone (names only). `onAskWithout`: the
 * composer's question can still be asked without it (removes the chip).
 */
export function reportMissingFiles(paths: readonly string[], retry: boolean, onAskWithout?: () => void): void {
  const notice = missingFileNotice(paths, retry);
  notify.error(notice.title, {
    description: notice.description,
    ...(onAskWithout ? { action: { label: 'Ask without it', onClick: onAskWithout } } : {}),
  });
}

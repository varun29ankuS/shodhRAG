import { invoke } from '@tauri-apps/api/core';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { notify } from '../../lib/notify';

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  return typeof error === 'string' ? error : 'Unknown error';
}

/** Open a file with the system's default app for its type. */
export async function openInDefaultApp(path: string): Promise<void> {
  try {
    await invoke('jump_to_source', { filePath: path });
  } catch (error) {
    notify.error('Could not open the file', { description: describe(error) });
  }
}

/** Show a file or folder selected in Explorer / Finder. */
export async function showInFolder(path: string): Promise<void> {
  try {
    await revealItemInDir(path);
  } catch (error) {
    notify.error('Could not show it in the file manager', { description: describe(error) });
  }
}

/** Copy a path to the clipboard. */
export async function copyPath(path: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(path);
    notify.success('Path copied');
  } catch (error) {
    notify.error('Could not copy the path', { description: describe(error) });
  }
}

/** Join a folder path and child segments with the folder's own separator. */
export function joinPath(root: string, segments: readonly string[]): string {
  if (segments.length === 0) return root;
  const sep = root.includes('\\') ? '\\' : '/';
  return [root.replace(/[\\/]+$/, ''), ...segments].join(sep);
}

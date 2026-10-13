/**
 * In-app updates (`updater.rs`). The backend checks quietly a few minutes
 * after start and then daily, only while the user leaves the check on and
 * Local-only mode is off, and announces a newer version with
 * `update-available`. Nothing is downloaded until the user chooses Install;
 * the download is signature-checked before it is installed.
 */
import { invoke } from '@tauri-apps/api/core';

export const UPDATE_AVAILABLE = 'update-available';
export const UPDATE_PROGRESS = 'update-progress';

export interface UpdateInfo {
  version: string;
  currentVersion: string;
  /** Release date (RFC 3339), when the server gives one. */
  date: string | null;
  /** Release notes, when the server gives them. */
  notes: string | null;
}

export interface UpdateProgress {
  downloaded: number;
  total: number | null;
}

export interface UpdateStatus {
  currentVersion: string;
  checkAutomatically: boolean;
  /** Why this app may not check now (no update key in this build, Local-only mode). */
  blockedReason: string | null;
}

export function getUpdateStatus(): Promise<UpdateStatus> {
  return invoke<UpdateStatus>('get_update_status');
}

export function setUpdateChecks(enabled: boolean): Promise<UpdateStatus> {
  return invoke<UpdateStatus>('set_update_checks', { enabled });
}

/** Ask the update server now; `null` when this is the latest version. */
export function checkForUpdate(): Promise<UpdateInfo | null> {
  return invoke<UpdateInfo | null>('check_for_update');
}

/** Download, verify and install the update the last check found, then restart. */
export function installUpdate(): Promise<void> {
  return invoke<void>('install_update');
}

/** Whole-percent download progress, or `null` when the size is unknown. */
export function progressPercent(progress: UpdateProgress | null): number | null {
  if (!progress || !progress.total || progress.total <= 0) return null;
  return Math.min(100, Math.floor((progress.downloaded * 100) / progress.total));
}

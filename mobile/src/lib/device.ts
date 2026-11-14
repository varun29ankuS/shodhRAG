/**
 * What the app needs of the phone itself, besides the link: its sealed
 * outbox, the one-tap update, and (in later parts) files, the calendar,
 * notifications, the lock and voice. The app's Rust side and Kotlin plugin
 * provide them (`tauriDevice`); tests give the app their own.
 */
import { invoke } from '@tauri-apps/api/core';
import { tauriOutbox, type OutboxSource } from './outbox';

/** What looking for the phone's update found (`update_check`). */
export type UpdateCheck = { state: 'ready'; version: string; size: number } | { state: 'unavailable'; message: string };

export interface Updater {
  /** Look for the app matching the computer's version. */
  check(): Promise<UpdateCheck>;
  /**
   * Download it, verify it and hand it to Android's installer. False when
   * Android first needs the user to allow Shodh to install updates.
   */
  install(): Promise<boolean>;
}

export interface Device {
  outbox: OutboxSource;
  updater: Updater;
}

export const tauriDevice: Device = {
  outbox: tauriOutbox,
  updater: {
    check: () => invoke<UpdateCheck>('update_check'),
    install: () => invoke<boolean>('update_install'),
  },
};

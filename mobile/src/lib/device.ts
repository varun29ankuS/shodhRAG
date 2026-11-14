/**
 * What the app needs of the phone itself, besides the link: its sealed
 * outbox, the one-tap update, files (Add, the camera, the share sheet) and,
 * in later parts, the calendar, notifications, the lock and voice. The app's
 * Rust side and Kotlin plugin provide them (`tauriDevice`); tests give the app
 * their own.
 */
import { addPluginListener, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { tauriOutbox, type OutboxSource, type QueuedFile } from './outbox';
import type { PhoneEvent } from './calsync';

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

/** A file Android copied into the app's own storage, ready to send. */
export interface Picked {
  path: string;
  name: string;
}

/** What another app shared to Shodh. */
export type Incoming =
  | { kind: 'none' }
  | { kind: 'files'; files: Picked[] }
  | { kind: 'text'; text: string; subject: string | null }
  | { kind: 'error'; message: string }
  /** A brief's notification was tapped. */
  | { kind: 'brief'; eventId: string };

/** The computer's answer to a file sent (`phone_file_finish`). */
export interface FileAdded {
  name: string;
  path: string;
  added: boolean;
  /** Why it was not added (the computer's upload rules). */
  reason: string | null;
  notice: string | null;
}

/** A photo sent to ask about (`phone_file_finish` for a photo). */
export interface PhotoRead {
  photoId: string;
  name: string;
  /** Its text, read on the computer. */
  text: string;
  words: number;
}

export interface FileProgress {
  localPath: string;
  received: number;
  size: number;
}

export interface PhoneFiles {
  /** Add: Android's file picker (nothing when the user went back). */
  pick(): Promise<Picked[]>;
  /** The camera: the photo taken, or null when the user went back. */
  takePhoto(): Promise<Picked | null>;
  /** What was shared to Shodh, once. */
  takeIncoming(): Promise<Incoming>;
  /** Something was shared to Shodh (read it with `takeIncoming`). */
  onIncoming(handler: () => void): Promise<() => void>;
  /** Its size and SHA-256, for the outbox. */
  details(picked: Picked, workspaceId: string | null): Promise<QueuedFile>;
  /** The user chose not to send it. */
  discard(path: string): Promise<void>;
  /** Send it to the library; the computer's answer. */
  send(file: QueuedFile): Promise<FileAdded>;
  /** Send a photo to ask about; its text, read on the computer. */
  sendPhoto(file: QueuedFile): Promise<PhotoRead>;
  onProgress(handler: (progress: FileProgress) => void): Promise<() => void>;
}

/** The user's calendar choices (crates/shodh-link/src/phone_calendar.rs). */
export interface CalendarChoice {
  enabled: boolean;
  chosen: number[];
  shodhCalendar: number | null;
  /** "Let Shodh use my calendar". */
  useWithShodh: boolean;
}

/** A calendar on the phone. */
export interface PhoneCalendarInfo {
  id: number;
  name: string;
  account: string;
  color: string;
  /** Shodh's own calendar. */
  shodh: boolean;
}

export type CalendarWrite =
  | { kind: 'insert'; calendarId: number; fields: Record<string, unknown> }
  | { kind: 'update'; calendarId: number; eventId: number; fields: Record<string, unknown> }
  | { kind: 'delete'; calendarId: number; eventId: number };

export interface PhoneCalendar {
  /** Ask Android for the calendar permission; whether it was given. */
  access(): Promise<boolean>;
  list(): Promise<PhoneCalendarInfo[]>;
  choice(): Promise<CalendarChoice>;
  choose(choice: CalendarChoice): Promise<CalendarChoice>;
  /** Single events of the calendars Shodh may read. */
  events(fromMs: number, toMs: number): Promise<PhoneEvent[]>;
  write(write: CalendarWrite): Promise<{ eventId?: number }>;
  /** The phone's calendars changed (while the app runs). */
  onChanged(handler: () => void): Promise<() => void>;
}

/** The phone's sealed files the window keeps. */
export type SealedName = 'agenda' | 'calendar-sync' | 'settings';

export interface Sealed {
  read<T>(name: SealedName): Promise<T | null>;
  write(name: SealedName, value: unknown): Promise<void>;
}

export interface PhoneVoice {
  /** Listen once: what the user said. */
  listen(): Promise<string>;
  stopListening(): Promise<void>;
  /** Read aloud; resolves when it was read. */
  speak(text: string): Promise<void>;
  stop(): Promise<void>;
}

export interface Device {
  outbox: OutboxSource;
  updater: Updater;
  files: PhoneFiles;
  calendar: PhoneCalendar;
  sealed: Sealed;
  /** Android's status and navigation bars: light icons on the dark theme, dark icons on the light. */
  systemBars(theme: 'light' | 'dark'): Promise<void>;
  notifications: PhoneNotifications;
  lock: PhoneLock;
  voice: PhoneVoice;
}

/** How the phone tells the user while the app is closed: never (the default), Stay connected, or UnifiedPush. */
export type NotifyMode = 'off' | 'stay' | 'push';

export interface PhoneNotifications {
  mode(): Promise<NotifyMode>;
  /** Choose the mode (with the push app, for UnifiedPush). */
  set(mode: NotifyMode, distributor?: string): Promise<NotifyMode>;
  /** The push apps on this phone. */
  distributors(): Promise<string[]>;
  /** Give the computer the push address; whether it has it. */
  syncPush(): Promise<boolean>;
  /** Alarms for briefs: [event id, Unix ms]. */
  scheduleBriefs(briefs: Array<[string, number]>): Promise<void>;
}

export interface LockStatus {
  required: boolean;
  hasPin: boolean;
  biometric: boolean;
  locked: boolean;
}

export interface PhoneLock {
  status(): Promise<LockStatus>;
  setPin(pin: string): Promise<void>;
  unlock(pin: string): Promise<void>;
  unlockBiometric(): Promise<void>;
  configure(pin: string, change: { enabled?: boolean; biometric?: boolean }): Promise<void>;
  hidden(): Promise<void>;
  /** Back on screen: whether it is locked again. */
  shown(): Promise<boolean>;
}

const PLUGIN = 'shodh-android';

export const tauriFiles: PhoneFiles = {
  pick: () => invoke<Picked[]>('files_pick'),
  takePhoto: () => invoke<Picked | null>('photo_take'),
  takeIncoming: () => invoke<Incoming>('incoming_take'),
  onIncoming: async handler => {
    const listener = await addPluginListener(PLUGIN, 'incoming', () => handler());
    return () => {
      void listener.unregister();
    };
  },
  details: (picked, workspaceId) =>
    invoke<Omit<QueuedFile, 'kind'>>('file_details', { path: picked.path, name: picked.name, workspaceId }).then(file => ({ ...file, kind: 'file' as const })),
  discard: path => invoke<void>('file_discard', { path }),
  send: file => invoke<FileAdded>('file_send', { file }),
  sendPhoto: file => invoke<PhotoRead>('photo_send', { file }),
  onProgress: handler => listen<FileProgress>('transfer://progress', event => handler(event.payload)),
};

export const tauriDevice: Device = {
  outbox: tauriOutbox,
  updater: {
    check: () => invoke<UpdateCheck>('update_check'),
    install: () => invoke<boolean>('update_install'),
  },
  files: tauriFiles,
  calendar: {
    access: () => invoke<boolean>('calendar_access'),
    list: () => invoke<PhoneCalendarInfo[]>('calendar_list'),
    choice: () => invoke<CalendarChoice>('calendar_choice'),
    choose: choice => invoke<CalendarChoice>('calendar_choose', { choice }),
    events: (fromMs, toMs) => invoke<PhoneEvent[]>('calendar_events', { fromMs, toMs }),
    write: write => invoke<{ eventId?: number }>('calendar_write', { write }),
    onChanged: async handler => {
      const listener = await addPluginListener(PLUGIN, 'calendar', () => handler());
      return () => {
        void listener.unregister();
      };
    },
  },
  sealed: {
    read: <T,>(name: SealedName) => invoke<T | null>('sealed_read', { name }),
    write: (name, value) => invoke<void>('sealed_write', { name, value }),
  },
  systemBars: theme => invoke<void>('system_bars', { theme }),
  notifications: {
    mode: () => invoke<NotifyMode>('notifications_mode'),
    set: (mode, distributor) => invoke<NotifyMode>('notifications_set', { mode, distributor: distributor ?? null }),
    distributors: () => invoke<string[]>('push_distributors'),
    syncPush: () => invoke<boolean>('push_sync'),
    scheduleBriefs: briefs => invoke<void>('briefs_schedule', { briefs }),
  },
  voice: {
    listen: () => invoke<string>('voice_listen'),
    stopListening: () => invoke<void>('voice_stop_listening'),
    speak: text => invoke<void>('voice_speak', { text }),
    stop: () => invoke<void>('voice_stop'),
  },
  lock: {
    status: () => invoke<LockStatus>('lock_status'),
    setPin: pin => invoke<void>('lock_set_pin', { pin }),
    unlock: pin => invoke<void>('lock_unlock', { pin }),
    unlockBiometric: () => invoke<void>('lock_unlock_biometric'),
    configure: (pin, change) => invoke<void>('lock_configure', { pin, enabled: change.enabled ?? null, biometric: change.biometric ?? null }),
    hidden: () => invoke<void>('lock_hidden'),
    shown: () => invoke<boolean>('lock_shown'),
  },
};

/**
 * The phone's link to the computer, as the app's Rust side runs it
 * (mobile/src-tauri/src/link.rs): its state, pairing, and the shim's
 * transport. Screens reach it through `LinkSource` (lib/context.tsx), so
 * tests give them a scripted computer instead.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { LinkTransport } from './invoke';

/** The connection's state (`Status` in link.rs). */
export type LinkStatus =
  | { state: 'starting' }
  | { state: 'unpaired' }
  | { state: 'connecting' }
  | { state: 'connected' }
  | { state: 'offline'; message: string }
  | { state: 'refused'; message: string }
  | { state: 'update'; message: string }
  | { state: 'failed'; message: string };

export const STATUS_EVENT = 'link://status';
const DESKTOP_EVENT = 'link://event';

/** What the app needs of the link. */
export interface LinkSource {
  transport: LinkTransport;
  status(): Promise<LinkStatus>;
  onStatus(handler: (status: LinkStatus) => void): Promise<() => void>;
  /** Try the kept computer again. */
  reconnect(): Promise<void>;
  /** Pair with the computer of a scanned or pasted `shodh-pair:1:…` code. */
  pair(code: string): Promise<void>;
  /** Start over after secure storage could not be read: pair again as a new phone. */
  reset(): Promise<void>;
  /** Tell the app a screen is shown, in which theme (CI's launch test waits for it). */
  ready(screen: string, theme: 'light' | 'dark'): Promise<void>;
  /**
   * The screen a debug build was asked to open at (CI's screenshots); always
   * null in a release build, where the app has no such setting.
   */
  startScreen(): Promise<string | null>;
}

/** The phone's link, over the app's own commands. */
export const tauriTransport: LinkTransport = {
  call: (command, args) => invoke('link_invoke', { command, args }),
  subscribe: sessionIds => invoke<void>('link_subscribe', { sessionIds }),
  onEvent: handler =>
    listen<{ name: string; payload: unknown }>(DESKTOP_EVENT, event => handler(event.payload.name, event.payload.payload)),
};

export const tauriLink: LinkSource = {
  transport: tauriTransport,
  status: () => invoke<LinkStatus>('link_status'),
  onStatus: handler => listen<LinkStatus>(STATUS_EVENT, event => handler(event.payload)),
  reconnect: () => invoke<LinkStatus>('link_connect').then(() => undefined),
  pair: code => invoke<void>('link_pair', { code }),
  reset: () => invoke<void>('link_reset'),
  ready: (screen, theme) => invoke<void>('app_ready', { screen, theme }),
  startScreen: () => invoke<string | null>('start_screen'),
};

/** Plain words for an error from a command. */
export function errorText(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error && typeof error === 'object' && 'message' in error && typeof error.message === 'string') return error.message;
  return 'Something went wrong.';
}

/** Whether the computer can be asked now. */
export function isConnected(status: LinkStatus): boolean {
  return status.state === 'connected';
}

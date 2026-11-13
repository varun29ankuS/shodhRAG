/**
 * The phone's link to the computer, as the app's Rust side runs it
 * (mobile/src-tauri/src/link.rs): its state, pairing, and the shim's
 * transport.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { createShim, type LinkTransport } from './invoke';

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

/** The phone's link, over the app's own commands. */
export const tauriTransport: LinkTransport = {
  call: (command, args) => invoke('link_invoke', { command, args }),
  subscribe: sessionIds => invoke<void>('link_subscribe', { sessionIds }),
  onEvent: handler =>
    listen<{ name: string; payload: unknown }>(DESKTOP_EVENT, event => handler(event.payload.name, event.payload.payload)),
};

/** The computer's commands and events, for the screens. */
export const desktop = createShim(tauriTransport);

export const linkStatus = () => invoke<LinkStatus>('link_status');

export const onLinkStatus = (handler: (status: LinkStatus) => void) =>
  listen<LinkStatus>(STATUS_EVENT, event => handler(event.payload));

/** Pair with the computer of a scanned or pasted `shodh-pair:1:…` code. */
export const pair = (code: string) => invoke<void>('link_pair', { code });

/** Try the kept computer again. */
export const reconnect = () => invoke<LinkStatus>('link_connect');

/** Start over after secure storage could not be read: pair again as a new phone. */
export const resetLink = () => invoke<void>('link_reset');

/** Tell the app the first screen is shown (CI's launch test waits for it). */
export const screenShown = (screen: 'pair' | 'connected' | 'offline' | 'failed') => invoke<void>('app_ready', { screen });

/** Plain words for an error from a command. */
export function errorText(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error && typeof error === 'object' && 'message' in error && typeof error.message === 'string') return error.message;
  return 'Something went wrong.';
}

/**
 * Whether the floating chat's layer is up: the embedded Artifacts preview shows, with the
 * chat in its own webview over it (`EmbeddedPreview` sets it). While it is, the main window
 * runs that chat (`useDockBridge`) instead of drawing the in-page one, which the preview
 * would cover.
 */
import { useSyncExternalStore } from 'react';

let active = false;
const listeners = new Set<() => void>();

export const dockLayer = {
  get: (): boolean => active,
  set(next: boolean): void {
    if (next === active) return;
    active = next;
    listeners.forEach(listener => listener());
  },
  subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  },
};

export function useDockLayerActive(): boolean {
  return useSyncExternalStore(dockLayer.subscribe, dockLayer.get, dockLayer.get);
}

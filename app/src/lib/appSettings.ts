/**
 * App settings kept by the backend (`app_settings.rs`): preferences the
 * user and the agent may change, and the privacy policy only the user may
 * change. The UI applies them on load and on every `app-settings-changed`.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export const APP_SETTINGS_CHANGED = 'app-settings-changed';

/** Bounds of `searchMaxResults` (the search tool's own limits). */
export const MIN_SEARCH_RESULTS = 3;
export const MAX_SEARCH_RESULTS = 20;

export interface Preferences {
  theme: 'light' | 'dark';
  /** Passages a document search returns by default. */
  searchMaxResults: number;
}

export interface Policy {
  /** Nothing leaves this computer: no web tools, local models only. */
  localOnly: boolean;
  /** The agent may search the web and read web pages. */
  webAccess: boolean;
}

/** Long-term memory. User-only: the assistant has no tool to change it. */
export interface MemoryPrefs {
  /** Recall relevant memories into each answer. */
  injectMemories: boolean;
}

export interface AppSettings {
  preferences: Preferences;
  policy: Policy;
  memory: MemoryPrefs;
  seeded: boolean;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Validate a settings payload from the backend. */
export function parseAppSettings(value: unknown): AppSettings | null {
  if (!isRecord(value) || !isRecord(value.preferences) || !isRecord(value.policy)) return null;
  const { preferences, policy } = value;
  if (preferences.theme !== 'light' && preferences.theme !== 'dark') return null;
  if (typeof preferences.searchMaxResults !== 'number') return null;
  if (typeof policy.localOnly !== 'boolean' || typeof policy.webAccess !== 'boolean') return null;
  // Settings written before memory existed have no `memory` section: the default is on.
  let injectMemories = true;
  if (value.memory !== undefined) {
    if (!isRecord(value.memory) || typeof value.memory.injectMemories !== 'boolean') return null;
    injectMemories = value.memory.injectMemories;
  }
  return {
    preferences: { theme: preferences.theme, searchMaxResults: preferences.searchMaxResults },
    policy: { localOnly: policy.localOnly, webAccess: policy.webAccess },
    memory: { injectMemories },
    seeded: value.seeded === true,
  };
}

export function clampSearchResults(n: number): number {
  if (!Number.isFinite(n)) return MIN_SEARCH_RESULTS;
  return Math.min(MAX_SEARCH_RESULTS, Math.max(MIN_SEARCH_RESULTS, Math.round(n)));
}

export async function getAppSettings(): Promise<AppSettings | null> {
  return parseAppSettings(await invoke<unknown>('get_app_settings'));
}

/** Change some preferences. With `seed`, only applied if never seeded before. */
export async function updatePreferences(patch: Partial<Preferences>, seed = false): Promise<AppSettings | null> {
  return parseAppSettings(await invoke<unknown>('update_app_preferences', { preferences: patch, seed }));
}

export async function setPolicy(policy: Policy): Promise<AppSettings | null> {
  return parseAppSettings(await invoke<unknown>('set_app_policy', { policy }));
}

export async function setMemoryPreferences(memory: MemoryPrefs): Promise<AppSettings | null> {
  return parseAppSettings(await invoke<unknown>('set_memory_preferences', { memory }));
}

/** Listen for settings changes (from the UI or the agent). Returns the unsubscribe function. */
export function onAppSettingsChanged(handler: (settings: AppSettings) => void): () => void {
  let disposed = false;
  let unlisten: (() => void) | null = null;
  listen<unknown>(APP_SETTINGS_CHANGED, event => {
    const settings = parseAppSettings(event.payload);
    if (settings) handler(settings);
  })
    .then(fn => {
      if (disposed) fn();
      else unlisten = fn;
    })
    .catch(err => console.error(`Failed to listen for ${APP_SETTINGS_CHANGED}:`, err));
  return () => {
    disposed = true;
    if (unlisten) unlisten();
  };
}

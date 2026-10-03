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

export type LearnMode = 'off' | 'ask' | 'auto';

/** Daily limits on the learning model (`LearnCaps`). */
export interface LearnCaps {
  maxCallsPerDay: number;
  maxInputCharsPerDay: number;
  maxProposalsPerDay: number;
}

export const DEFAULT_LEARN_CAPS: LearnCaps = { maxCallsPerDay: 60, maxInputCharsPerDay: 400_000, maxProposalsPerDay: 40 };
export const DEFAULT_AUTO_MIN_CONFIDENCE = 0.85;

/** Long-term memory. User-only: the assistant has no tool to change it. */
export interface MemoryPrefs {
  /** Recall relevant memories into each answer. */
  injectMemories: boolean;
  /** Learn from conversations: off, ask (suggestions wait for you) or auto. */
  learnMode: LearnMode;
  /** Cheaper model id of the configured provider for learning; `null` uses the configured model. */
  learnModel: string | null;
  /** Lowest confidence applied automatically in auto mode (0.5 to 1). */
  autoMinConfidence: number;
  learnCaps: LearnCaps;
}

export const DEFAULT_MEMORY_PREFS: MemoryPrefs = {
  injectMemories: true,
  learnMode: 'ask',
  learnModel: null,
  autoMinConfidence: DEFAULT_AUTO_MIN_CONFIDENCE,
  learnCaps: DEFAULT_LEARN_CAPS,
};

function parseCaps(value: unknown): LearnCaps | null {
  if (value === undefined) return DEFAULT_LEARN_CAPS;
  if (!isRecord(value)) return null;
  const n = (x: unknown, fallback: number) => (x === undefined ? fallback : typeof x === 'number' && Number.isFinite(x) ? x : NaN);
  const caps = {
    maxCallsPerDay: n(value.maxCallsPerDay, DEFAULT_LEARN_CAPS.maxCallsPerDay),
    maxInputCharsPerDay: n(value.maxInputCharsPerDay, DEFAULT_LEARN_CAPS.maxInputCharsPerDay),
    maxProposalsPerDay: n(value.maxProposalsPerDay, DEFAULT_LEARN_CAPS.maxProposalsPerDay),
  };
  return Object.values(caps).some(Number.isNaN) ? null : caps;
}

/** Parse the memory section; settings written before a field existed get its default. */
export function parseMemoryPrefs(value: unknown): MemoryPrefs | null {
  if (value === undefined) return DEFAULT_MEMORY_PREFS;
  if (!isRecord(value)) return null;
  const injectMemories = value.injectMemories === undefined ? true : value.injectMemories;
  if (typeof injectMemories !== 'boolean') return null;
  const learnMode = value.learnMode === undefined ? 'ask' : value.learnMode;
  if (learnMode !== 'off' && learnMode !== 'ask' && learnMode !== 'auto') return null;
  const rawModel = value.learnModel;
  if (rawModel !== undefined && rawModel !== null && typeof rawModel !== 'string') return null;
  const learnModel = typeof rawModel === 'string' ? rawModel : null;
  const autoMinConfidence = value.autoMinConfidence === undefined ? DEFAULT_AUTO_MIN_CONFIDENCE : value.autoMinConfidence;
  if (typeof autoMinConfidence !== 'number' || !Number.isFinite(autoMinConfidence)) return null;
  const learnCaps = parseCaps(value.learnCaps);
  if (!learnCaps) return null;
  return { injectMemories, learnMode, learnModel, autoMinConfidence, learnCaps };
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
  // Settings written before memory existed have no `memory` section: the defaults apply.
  const memory = parseMemoryPrefs(value.memory);
  if (!memory) return null;
  return {
    preferences: { theme: preferences.theme, searchMaxResults: preferences.searchMaxResults },
    policy: { localOnly: policy.localOnly, webAccess: policy.webAccess },
    memory,
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

import { invoke } from '@tauri-apps/api/core';
import { notify } from './notify';

/** Builds before the keychain change kept provider API keys in localStorage under this key. */
const LEGACY_API_KEYS_STORAGE_KEY = 'llm_api_keys';

/** Provider ids the backend's `set_api_key` accepts. */
export const API_KEY_PROVIDERS = [
  'openai', 'anthropic', 'openrouter', 'kimi', 'grok', 'perplexity', 'google', 'baseten',
] as const;

let inFlight: Promise<void> | null = null;

async function runMigration(): Promise<void> {
  let raw: string | null;
  try {
    raw = localStorage.getItem(LEGACY_API_KEYS_STORAGE_KEY);
  } catch {
    return;
  }
  if (raw === null) return;

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    localStorage.removeItem(LEGACY_API_KEYS_STORAGE_KEY);
    notify.warning('Discarded unreadable saved API keys', {
      description: 'Re-enter your provider API key in Model settings.',
    });
    return;
  }

  const entries: [string, string][] = [];
  if (parsed !== null && typeof parsed === 'object') {
    for (const [provider, key] of Object.entries(parsed as Record<string, unknown>)) {
      if (
        (API_KEY_PROVIDERS as readonly string[]).includes(provider) &&
        typeof key === 'string' &&
        key.trim().length > 0
      ) {
        entries.push([provider, key.trim()]);
      }
    }
  }

  const failed: string[] = [];
  for (const [provider, apiKey] of entries) {
    try {
      await invoke('set_api_key', { provider, apiKey });
    } catch (error) {
      failed.push(`${provider}: ${error}`);
    }
  }

  if (failed.length > 0) {
    // Keep the legacy entry so no key is lost; the move is retried on next launch.
    notify.error('Could not move saved API keys to the system keychain', {
      description: failed.join('; '),
    });
    return;
  }

  localStorage.removeItem(LEGACY_API_KEYS_STORAGE_KEY);
  if (entries.length > 0) {
    notify.success('Saved API keys moved to the system keychain');
  }
}

/**
 * One-time move of API keys saved by older builds from localStorage into the
 * OS credential store (via the backend). The localStorage entry is removed
 * only after every key was stored. Concurrent callers share one run.
 */
export function migrateLegacyApiKeys(): Promise<void> {
  if (!inFlight) {
    inFlight = runMigration().finally(() => {
      inFlight = null;
    });
  }
  return inFlight;
}

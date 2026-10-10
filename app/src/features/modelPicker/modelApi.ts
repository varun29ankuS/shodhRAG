/**
 * Client for the model picker commands (`model_picker_commands.rs`) and the
 * shared state every picker on screen reads (the Ask chip, Settings → Model).
 */
import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { APP_SETTINGS_CHANGED } from '../../lib/appSettings';
import type { FallbackOffer, ModelPrefs, ModelRef, PickId, PickerError, PickerView, ProviderId } from './modelTypes';
import { isProviderId, readModelPrefs, toPickerError } from './modelTypes';

/** Window event sent after the active model changed (the app refreshes its status line). */
export const MODEL_CHANGED_EVENT = 'shodh:model-changed';

/** Window event that opens the model picker of the Ask composer ("Change model" on an error card). */
export const OPEN_MODEL_PICKER_EVENT = 'shodh:open-model-picker';

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw toPickerError(error);
  }
}

export interface SelectOptions {
  confirmStealth?: boolean;
  sessionOverride?: boolean;
}

export const modelApi = {
  view: (refresh = false) => call<PickerView>('model_picker_view', { refresh }),
  select: (model: ModelRef, options: SelectOptions = {}) =>
    call<PickerView>('model_select', {
      request: { model, confirmStealth: options.confirmStealth ?? false, sessionOverride: options.sessionOverride ?? false },
    }),
  setFavourite: (model: ModelRef, favourite: boolean) => call<ModelPrefs>('model_set_favourite', { model, favourite }),
  setFallback: (fallback: ModelRef | null, alwaysFallBack: boolean) =>
    call<ModelPrefs>('model_set_fallback', { fallback, alwaysFallBack }),
  fallbackOffer: (failed: ModelRef) => call<FallbackOffer | null>('model_fallback_offer', { failed }),
  choosePick: (pick: PickId, sessionOverride = false) => call<PickerView>('model_use_pick', { pick, sessionOverride }),
  setProviderOrder: (order: ProviderId[]) => call<ModelPrefs>('model_set_provider_order', { order }),
  setBaseUrl: (provider: ProviderId, url: string | null) => call<ModelPrefs>('model_set_base_url', { provider, url }),
};

/** A refusal from a connect command (`connect_commands.rs`). */
export interface ConnectError {
  code: 'ambiguous' | 'rejected' | 'unreachable' | 'invalid' | 'runtime_missing' | 'busy' | 'failed';
  message: string;
  /** For `ambiguous`: the providers the key may belong to. */
  candidates: ProviderId[];
}

const CONNECT_CODES: readonly ConnectError['code'][] = ['ambiguous', 'rejected', 'unreachable', 'invalid', 'runtime_missing', 'busy', 'failed'];

export function toConnectError(error: unknown): ConnectError {
  if (typeof error === 'object' && error !== null && typeof (error as { message?: unknown }).message === 'string') {
    const e = error as { code?: unknown; message: string; candidates?: unknown };
    const code = (CONNECT_CODES as readonly unknown[]).includes(e.code) ? (e.code as ConnectError['code']) : 'failed';
    const candidates = Array.isArray(e.candidates) ? e.candidates.filter(isProviderId) : [];
    return { code, message: e.message, candidates };
  }
  return { code: 'failed', message: error instanceof Error ? error.message : String(error), candidates: [] };
}

async function connectCall<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw toConnectError(error);
  }
}

/** Sign-in progress (`connect-sign-in` events). */
export const SIGN_IN_EVENT = 'connect-sign-in';

export type SignInStep =
  | { kind: 'open_page'; url: string }
  | { kind: 'device_code'; code: string }
  | { kind: 'progress'; message: string }
  | { kind: 'saved' };

export interface SignInEvent {
  provider: ProviderId;
  phase: 'step' | 'connected' | 'failed' | 'cancelled';
  step?: SignInStep;
  message?: string;
}

/** The three ways to connect (Settings → Model → Connect). */
export const connectApi = {
  signIn: (provider: ProviderId) => connectCall<void>('connect_sign_in', { provider }),
  cancelSignIn: () => connectCall<void>('connect_sign_in_cancel'),
  signOut: (provider: ProviderId) => connectCall<void>('connect_sign_out', { provider }),
  saveKey: (key: string, provider: ProviderId | null) =>
    connectCall<{ provider: ProviderId; label: string }>('connect_save_key', { key, provider }),
  removeKey: (provider: ProviderId) => connectCall<void>('connect_remove_key', { provider }),
  /** Download the agent runtime (needed before signing in). */
  installRuntime: () => connectCall<unknown>('agent_install_runtime'),
};

export function announceModelChange() {
  window.dispatchEvent(new CustomEvent(MODEL_CHANGED_EVENT));
}

export interface ModelPickerHandle {
  view: PickerView | null;
  loading: boolean;
  /** Why the list could not be loaded at all. */
  loadError: string | null;
  reload: (refresh?: boolean) => Promise<void>;
  /** Select a model; resolves with the refusal (stealth confirmation, environment, key…) or null. */
  select: (model: ModelRef, options?: SelectOptions) => Promise<PickerError | null>;
  setFavourite: (model: ModelRef, favourite: boolean) => Promise<void>;
  setFallback: (fallback: ModelRef | null, alwaysFallBack: boolean) => Promise<PickerError | null>;
  /** Use one of the four picks; resolves with the refusal or null. */
  choosePick: (pick: PickId, sessionOverride?: boolean) => Promise<PickerError | null>;
}

/**
 * The picker's state. `active` loads it on mount (and on every model change
 * anywhere in the app); settings changes update favourites and recent models
 * in place.
 */
export function useModelPicker(active = true): ModelPickerHandle {
  const [view, setView] = useState<PickerView | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  const reload = useCallback(async (refresh = false) => {
    setLoading(true);
    try {
      setView(await modelApi.view(refresh));
      setLoadError(null);
    } catch (error) {
      setLoadError(toPickerError(error).message);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void reload(false);
    const onChanged = () => void reload(false);
    window.addEventListener(MODEL_CHANGED_EVENT, onChanged);
    return () => window.removeEventListener(MODEL_CHANGED_EVENT, onChanged);
  }, [active, reload]);

  useEffect(() => {
    if (!active) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<unknown>(APP_SETTINGS_CHANGED, event => {
      const payload = event.payload;
      if (typeof payload !== 'object' || payload === null) return;
      const prefs = readModelPrefs((payload as { models?: unknown }).models);
      setView(v => (v ? { ...v, prefs } : v));
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(error => console.error('Model settings listener failed:', error));
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [active]);

  const select = useCallback(async (model: ModelRef, options: SelectOptions = {}) => {
    try {
      setView(await modelApi.select(model, options));
      announceModelChange();
      return null;
    } catch (error) {
      return toPickerError(error);
    }
  }, []);

  const setFavourite = useCallback(async (model: ModelRef, favourite: boolean) => {
    try {
      const prefs = await modelApi.setFavourite(model, favourite);
      setView(v => (v ? { ...v, prefs } : v));
    } catch (error) {
      console.error('Favourite not saved:', toPickerError(error).message);
    }
  }, []);

  const setFallback = useCallback(async (fallback: ModelRef | null, alwaysFallBack: boolean) => {
    try {
      const prefs = await modelApi.setFallback(fallback, alwaysFallBack);
      setView(v => (v ? { ...v, prefs } : v));
      return null;
    } catch (error) {
      return toPickerError(error);
    }
  }, []);

  const choosePick = useCallback(async (pick: PickId, sessionOverride = false) => {
    try {
      setView(await modelApi.choosePick(pick, sessionOverride));
      announceModelChange();
      return null;
    } catch (error) {
      return toPickerError(error);
    }
  }, []);

  return { view, loading, loadError, reload, select, setFavourite, setFallback, choosePick };
}

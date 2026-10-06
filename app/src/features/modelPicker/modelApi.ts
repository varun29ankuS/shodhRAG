/**
 * Client for the model picker commands (`model_picker_commands.rs`) and the
 * shared state every picker on screen reads (the Ask chip, Settings → Model).
 */
import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { APP_SETTINGS_CHANGED } from '../../lib/appSettings';
import type { FallbackOffer, ModelPrefs, ModelRef, PickerError, PickerView } from './modelTypes';
import { readModelPrefs, toPickerError } from './modelTypes';

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

  return { view, loading, loadError, reload, select, setFavourite, setFallback };
}

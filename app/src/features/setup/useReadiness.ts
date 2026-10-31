import { useCallback, useMemo } from 'react';
import { useSearchModels } from './SearchModelsContext';
import { useRuntimeSetup } from './RuntimeSetupContext';
import { partsToInstall, readiness } from './readiness';
import type { Readiness } from './readiness';

export interface ReadinessValue extends Readiness {
  /** Where the answer engine comes from ("GitHub"). */
  runtimeSource: string;
  /**
   * Start (or resume) every download still needed, at once: the search models and
   * the answer engine. Parts already downloading or installed are left alone.
   */
  startSetup: () => void;
  /** Read both statuses again. */
  refresh: () => void;
}

/** Search models + answer engine as one setup (see `readiness.ts`). */
export function useReadiness(): ReadinessValue {
  const search = useSearchModels();
  const runtime = useRuntimeSetup();

  const state = useMemo(
    () => readiness(
      {
        status: search.status,
        statusError: search.statusError,
        installing: search.installing,
        progress: search.progress,
        installError: search.installError,
        installOffline: search.installOffline,
      },
      {
        status: runtime.status,
        statusError: runtime.statusError,
        installing: runtime.installing,
        progress: runtime.progress,
        installError: runtime.installError,
        installOffline: runtime.installOffline,
      },
    ),
    [search.status, search.statusError, search.installing, search.progress, search.installError, search.installOffline,
      runtime.status, runtime.statusError, runtime.installing, runtime.progress, runtime.installError, runtime.installOffline],
  );

  const { install: installSearch, refresh: refreshSearch } = search;
  const { install: installRuntime, refresh: refreshRuntime } = runtime;
  const startSetup = useCallback(() => {
    const parts = partsToInstall(state);
    if (parts.includes('search')) void installSearch();
    if (parts.includes('runtime')) void installRuntime();
  }, [state, installSearch, installRuntime]);

  const refresh = useCallback(() => {
    void refreshSearch();
    void refreshRuntime();
  }, [refreshSearch, refreshRuntime]);

  return useMemo(
    () => ({ ...state, runtimeSource: runtime.status?.source ?? 'GitHub', startSetup, refresh }),
    [state, runtime.status?.source, startSetup, refresh],
  );
}

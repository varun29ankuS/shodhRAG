import React, { useState, useEffect } from 'react';
import { Sliders, RotateCcw } from 'lucide-react';
import { useTheme } from '../contexts/ThemeContext';
import {
  MAX_SEARCH_RESULTS,
  MIN_SEARCH_RESULTS,
  clampSearchResults,
  getAppSettings,
  onAppSettingsChanged,
  updatePreferences,
} from '../lib/appSettings';
import { useSystemStatusContext } from '../features/status/SystemStatusContext';
import { FASTER_SEARCH_NAME, newLibraryChoice } from '../features/status/systemStatus';

/**
 * Search preferences the user can change. Only the passage count exists:
 * retrieval is always hybrid (vector + BM25 fused with RRF, then ranked),
 * and fused scores are ranks, not calibrated relevance, so a "minimum
 * relevance" or a mode switch would be a setting with no honest meaning.
 */
interface SearchConfig {
  maxResults: number;
}

const DEFAULT_CONFIG: SearchConfig = {
  maxResults: 8,
};

const STORAGE_KEY = 'shodh_search_config';

function saveLocal(config: SearchConfig) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(config));
  } catch {
    // Storage unavailable; the backend keeps the passage count.
  }
}

/**
 * Search preferences. The passage count lives in the backend settings
 * store (the agent's searches use it, and the agent may change it); a
 * copy in local storage seeds that store on first run (ThemeContext).
 */
export function useSearchConfig() {
  const [config, setConfig] = useState<SearchConfig>(() => {
    try {
      const saved = localStorage.getItem(STORAGE_KEY);
      const parsed: unknown = saved ? JSON.parse(saved) : null;
      const stored = typeof parsed === 'object' && parsed !== null ? (parsed as { maxResults?: unknown }).maxResults : undefined;
      return { maxResults: clampSearchResults(typeof stored === 'number' ? stored : DEFAULT_CONFIG.maxResults) };
    } catch {
      return DEFAULT_CONFIG;
    }
  });

  useEffect(() => {
    let cancelled = false;
    const apply = (maxResults: number) =>
      setConfig(prev => (prev.maxResults === maxResults ? prev : { ...prev, maxResults }));
    getAppSettings()
      .then(settings => {
        if (!cancelled && settings?.seeded) apply(settings.preferences.searchMaxResults);
      })
      .catch(err => console.error('Loading search settings failed:', err));
    const unsubscribe = onAppSettingsChanged(settings => apply(settings.preferences.searchMaxResults));
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  const updateConfig = (updates: Partial<SearchConfig>) => {
    if (updates.maxResults !== undefined) {
      const searchMaxResults = clampSearchResults(updates.maxResults);
      updatePreferences({ searchMaxResults }).catch(err => console.error('Saving search settings failed:', err));
    }
    setConfig(prev => {
      const next = { maxResults: clampSearchResults(updates.maxResults ?? prev.maxResults) };
      saveLocal(next);
      return next;
    });
  };

  const resetConfig = () => {
    updatePreferences({ searchMaxResults: DEFAULT_CONFIG.maxResults })
      .catch(err => console.error('Saving search settings failed:', err));
    saveLocal(DEFAULT_CONFIG);
    setConfig(DEFAULT_CONFIG);
  };

  return { config, updateConfig, resetConfig };
}

interface SearchSettingsProps {
  config: SearchConfig;
  onUpdate: (updates: Partial<SearchConfig>) => void;
  onReset: () => void;
}

export default function SearchSettings({ config, onUpdate, onReset }: SearchSettingsProps) {
  const { colors } = useTheme();

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Sliders className="w-4 h-4" style={{ color: colors.primary }} />
          <span className="text-sm font-semibold" style={{ color: colors.text }}>Search</span>
        </div>
        <button
          onClick={onReset}
          className="text-caption flex items-center gap-1 px-2 py-1 rounded border transition-colors"
          style={{ borderColor: colors.border, color: colors.textMuted }}
        >
          <RotateCcw className="w-3 h-3" />
          Reset
        </button>
      </div>

      {/* Max results */}
      <div>
        <div className="flex items-center justify-between mb-1.5">
          <label className="text-xs font-medium" style={{ color: colors.textSecondary }}>
            Passages per search
          </label>
          <span className="text-xs font-bold" style={{ color: colors.text }}>{config.maxResults}</span>
        </div>
        <input
          type="range"
          min={MIN_SEARCH_RESULTS}
          max={MAX_SEARCH_RESULTS}
          value={config.maxResults}
          aria-label="Passages per search"
          onChange={e => onUpdate({ maxResults: Number(e.target.value) })}
          className="w-full h-1 rounded-full appearance-none cursor-pointer"
          style={{ accentColor: colors.primary }}
        />
        <div className="flex justify-between text-caption mt-0.5" style={{ color: colors.textMuted }}>
          <span>{MIN_SEARCH_RESULTS} (fast)</span>
          <span>{MAX_SEARCH_RESULTS} (thorough)</span>
        </div>
        <p className="text-caption mt-1.5" style={{ color: colors.textMuted }}>
          Each search combines meaning-based and keyword matching, then ranks the passages; the assistant may ask for more when a question needs it.
        </p>
      </div>

      <SearchModelChoice />
    </div>
  );
}

/**
 * The search model a new library is built with: the standard model by
 * default; "Faster search (slightly less accurate)" only where this processor
 * runs it exactly. A library that already has passages keeps its model and
 * moves with Re-index in System status.
 */
function SearchModelChoice() {
  const { colors } = useTheme();
  const system = useSystemStatusContext();
  const choice = newLibraryChoice(system?.status ?? null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (!system || !choice) return null;

  const choose = (faster: boolean) => {
    if (faster === choice.faster || saving) return;
    setSaving(true);
    setError(null);
    system
      .setFasterForNewLibraries(faster)
      .catch(err => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setSaving(false));
  };

  const option = (faster: boolean, title: string, detail: string) => (
    <label className="flex items-start gap-2 cursor-pointer">
      <input
        type="radio"
        name="search-model-new-library"
        checked={choice.faster === faster}
        disabled={saving}
        onChange={() => choose(faster)}
        className="mt-0.5"
        style={{ accentColor: colors.primary }}
      />
      <span className="flex flex-col">
        <span className="text-xs font-medium" style={{ color: colors.text }}>{title}</span>
        <span className="text-caption" style={{ color: colors.textMuted }}>{detail}</span>
      </span>
    </label>
  );

  return (
    <fieldset className="space-y-2 border-0 p-0 m-0" disabled={saving}>
      <legend className="text-xs font-medium mb-1" style={{ color: colors.textSecondary }}>
        Search model for a new library
      </legend>
      {option(false, 'Standard (recommended)', 'The most accurate ranking. Used unless you choose otherwise.')}
      {option(
        true,
        FASTER_SEARCH_NAME,
        'About 3 times faster indexing and search on this processor, but may rank some passages lower. Downloads the model (279 MB) when chosen.',
      )}
      {saving && (
        <p className="text-caption" style={{ color: colors.textMuted }} role="status">
          Saving…
        </p>
      )}
      {error && (
        <p className="text-caption" style={{ color: colors.error }} role="alert">
          {error}
        </p>
      )}
      {choice.current && (
        <p className="text-caption" style={{ color: colors.textMuted }}>
          {choice.current}
        </p>
      )}
    </fieldset>
  );
}

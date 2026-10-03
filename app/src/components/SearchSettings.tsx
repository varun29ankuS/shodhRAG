import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Search, Sliders, RotateCcw } from 'lucide-react';
import { useTheme } from '../contexts/ThemeContext';
import {
  MAX_SEARCH_RESULTS,
  MIN_SEARCH_RESULTS,
  clampSearchResults,
  getAppSettings,
  onAppSettingsChanged,
  updatePreferences,
} from '../lib/appSettings';

interface SearchConfig {
  maxResults: number;
  searchMode: 'hybrid' | 'semantic' | 'keyword';
  includeMetadata: boolean;
  minRelevanceScore: number;
}

const DEFAULT_CONFIG: SearchConfig = {
  maxResults: 8,
  searchMode: 'hybrid',
  includeMetadata: true,
  minRelevanceScore: 0.3,
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
 * store (the agent's searches use it, and the agent may change it); the
 * other fields are kept in local storage.
 */
export function useSearchConfig() {
  const [config, setConfig] = useState<SearchConfig>(() => {
    try {
      const saved = localStorage.getItem(STORAGE_KEY);
      const merged = saved ? { ...DEFAULT_CONFIG, ...JSON.parse(saved) } : DEFAULT_CONFIG;
      return { ...merged, maxResults: clampSearchResults(merged.maxResults) };
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
      const next = { ...prev, ...updates };
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
          <span className="text-sm font-semibold" style={{ color: colors.text }}>Search Settings</span>
        </div>
        <button
          onClick={onReset}
          className="text-[10px] flex items-center gap-1 px-2 py-1 rounded border transition-colors"
          style={{ borderColor: colors.border, color: colors.textMuted }}
        >
          <RotateCcw className="w-3 h-3" />
          Reset
        </button>
      </div>

      {/* Search mode */}
      <div>
        <label className="text-xs font-medium block mb-1.5" style={{ color: colors.textSecondary }}>
          Search Mode
        </label>
        <div className="flex gap-1.5">
          {(['hybrid', 'semantic', 'keyword'] as const).map(mode => (
            <button
              key={mode}
              onClick={() => onUpdate({ searchMode: mode })}
              className="text-[11px] px-3 py-1.5 rounded-md border transition-colors font-medium"
              style={{
                borderColor: config.searchMode === mode ? colors.primary : colors.border,
                backgroundColor: config.searchMode === mode ? `${colors.primary}14` : 'transparent',
                color: config.searchMode === mode ? colors.primary : colors.textMuted,
              }}
            >
              {mode.charAt(0).toUpperCase() + mode.slice(1)}
            </button>
          ))}
        </div>
        <p className="text-[10px] mt-1" style={{ color: colors.textMuted }}>
          {config.searchMode === 'hybrid' && 'Combines semantic + keyword search (BM25) for best results'}
          {config.searchMode === 'semantic' && 'Uses vector embeddings for meaning-based search'}
          {config.searchMode === 'keyword' && 'Uses BM25 keyword matching for exact term search'}
        </p>
      </div>

      {/* Max results */}
      <div>
        <div className="flex items-center justify-between mb-1.5">
          <label className="text-xs font-medium" style={{ color: colors.textSecondary }}>
            Max Results
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
        <div className="flex justify-between text-[10px] mt-0.5" style={{ color: colors.textMuted }}>
          <span>{MIN_SEARCH_RESULTS} (fast)</span>
          <span>{MAX_SEARCH_RESULTS} (thorough)</span>
        </div>
      </div>

      {/* Min relevance score */}
      <div>
        <div className="flex items-center justify-between mb-1.5">
          <label className="text-xs font-medium" style={{ color: colors.textSecondary }}>
            Min Relevance Score
          </label>
          <span className="text-xs font-bold" style={{ color: colors.text }}>{(config.minRelevanceScore * 100).toFixed(0)}%</span>
        </div>
        <input
          type="range"
          min={0}
          max={90}
          step={5}
          value={config.minRelevanceScore * 100}
          onChange={e => onUpdate({ minRelevanceScore: Number(e.target.value) / 100 })}
          className="w-full h-1 rounded-full appearance-none cursor-pointer"
          style={{ accentColor: colors.primary }}
        />
        <div className="flex justify-between text-[10px] mt-0.5" style={{ color: colors.textMuted }}>
          <span>0% (all results)</span>
          <span>90% (high confidence only)</span>
        </div>
      </div>

      {/* Include metadata */}
      <div className="flex items-center justify-between">
        <div>
          <label className="text-xs font-medium" style={{ color: colors.textSecondary }}>
            Include file metadata in context
          </label>
          <p className="text-[10px]" style={{ color: colors.textMuted }}>
            Sends file paths, types, and dates to the LLM
          </p>
        </div>
        <button
          onClick={() => onUpdate({ includeMetadata: !config.includeMetadata })}
          className="w-8 h-4 rounded-full transition-colors relative"
          style={{
            backgroundColor: config.includeMetadata ? colors.primary : colors.bgTertiary,
          }}
        >
          <div
            className="w-3 h-3 rounded-full bg-white absolute top-0.5 transition-all"
            style={{ left: config.includeMetadata ? '1.125rem' : '0.125rem' }}
          />
        </button>
      </div>
    </div>
  );
}

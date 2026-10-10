import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Download, ShieldCheck } from 'lucide-react';
import { cn } from '../lib/utils';
import { getAppSettings, onAppSettingsChanged, setAnswerPreferences } from '../lib/appSettings';
import type { AnswerPrefs } from '../lib/appSettings';
import { isInstallProgress, isSearchModelsStatus } from '../features/setup/searchModels';
import type { InstallProgress } from '../features/setup/searchModels';
import { SwitchRow } from './PrivacySettings';

const ANSWER_CHECK_PROGRESS_EVENT = 'answer-check-progress';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

interface ModelStatus {
  ready: boolean;
  installing: boolean;
  totalBytes: number;
}

function megabytes(bytes: number): string {
  return `${Math.max(1, Math.round(bytes / 1_000_000))} MB`;
}

/** The optional answer checking (entailment) model: its status and its install. */
function useAnswerCheckModel() {
  const [status, setStatus] = useState<ModelStatus | null>(null);
  const [progress, setProgress] = useState<InstallProgress | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const value = await invoke<unknown>('answer_check_status');
      if (isSearchModelsStatus(value)) {
        const v = value as unknown as ModelStatus;
        setStatus({ ready: v.ready, installing: v.installing, totalBytes: v.totalBytes });
      }
    } catch (e) {
      setError(`The answer checking model's status could not be read: ${String(e)}`);
    }
  }, []);

  useEffect(() => {
    let alive = true;
    void refresh();
    let unlisten: (() => void) | null = null;
    listen<unknown>(ANSWER_CHECK_PROGRESS_EVENT, event => {
      if (isInstallProgress(event.payload)) setProgress(event.payload);
    })
      .then(fn => {
        if (alive) unlisten = fn;
        else fn();
      })
      .catch(e => console.error('Failed to listen for answer check progress:', e));
    return () => {
      alive = false;
      if (unlisten) unlisten();
    };
  }, [refresh]);

  const install = useCallback(async () => {
    setError(null);
    setStatus(s => (s ? { ...s, installing: true } : s));
    try {
      await invoke<unknown>('install_answer_check_model');
    } catch (e) {
      setError(`The answer checking model could not be installed: ${String(e)}`);
    } finally {
      setProgress(null);
      await refresh();
    }
  }, [refresh]);

  const percent = progress && progress.overallTotal > 0
    ? Math.min(100, Math.round((progress.overallBytes / progress.overallTotal) * 100))
    : null;

  return { status, percent, error, install };
}

/**
 * The answer checking model: installed, or what it adds and an Install
 * button. Shown in Settings → Search and offered during first-run setup.
 */
export function AnswerCheckModelRow() {
  const { status, percent, error, install } = useAnswerCheckModel();
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-start justify-between gap-6">
        <div className="min-w-0">
          <p className="m-0 text-[13.5px] font-semibold text-shodh-text">Answer checking model</p>
          <p className="m-0 mt-0.5 text-[12.5px] text-shodh-text-muted">
            {status?.ready
              ? 'Installed. Statements are checked for what their passage actually says, including reversed or swapped facts.'
              : `Without it, statements are only checked for topic and numbers. A small local model (${megabytes(status?.totalBytes ?? 96_034_745)}, verified download).`}
          </p>
          {status?.installing && (
            <p className="m-0 mt-1 text-[12px] text-shodh-text-secondary tabular-nums" role="status" aria-live="polite">
              {percent !== null ? `Downloading… ${percent}%` : 'Installing…'}
            </p>
          )}
        </div>
        {status?.ready ? (
          <span className="shrink-0 inline-flex items-center gap-1.5 text-[12.5px] text-shodh-success">
            <ShieldCheck className="w-4 h-4" aria-hidden="true" />
            Ready
          </span>
        ) : (
          <button
            type="button"
            onClick={() => void install()}
            disabled={status === null || status.installing}
            className={cn(
              'shrink-0 h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border bg-shodh-surface text-[12.5px] text-shodh-text',
              'hover:bg-shodh-raised disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
              FOCUS_RING,
            )}
          >
            <Download className="w-3.5 h-3.5" aria-hidden="true" />
            Install
          </button>
        )}
      </div>
      {error && (
        <p role="alert" className="m-0 text-[12.5px] text-shodh-error break-words">
          {error}
        </p>
      )}
    </div>
  );
}

/**
 * Settings → Search → Answer checking: whether flagged statements are sent
 * back to the model once, and the optional local model that checks whether a
 * passage actually states what an answer cites it for. User-only: the
 * assistant has no tool to change either.
 */
export default function AnswerCheckSettings() {
  const [answers, setAnswers] = useState<AnswerPrefs | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let alive = true;
    getAppSettings()
      .then(settings => {
        if (alive && settings) setAnswers(settings.answers);
      })
      .catch(e => setError(`Settings could not be read: ${String(e)}`));
    const unsubscribe = onAppSettingsChanged(settings => setAnswers(settings.answers));
    return () => {
      alive = false;
      unsubscribe();
    };
  }, []);

  const toggleRepair = useCallback(async (autoRepair: boolean) => {
    setSaving(true);
    setError(null);
    try {
      const settings = await setAnswerPreferences({ autoRepair });
      if (settings) setAnswers(settings.answers);
    } catch (e) {
      setError(`The setting could not be saved: ${String(e)}`);
    } finally {
      setSaving(false);
    }
  }, []);

  return (
    <div className="mt-6 pt-6 border-t border-shodh-border-subtle flex flex-col gap-4">
      <div className="flex flex-col gap-1">
        <h3 className="m-0 text-[15px] font-semibold text-shodh-text">Answer checking</h3>
        <p className="m-0 text-[12.5px] text-shodh-text-muted">
          Every answer is checked on this computer, statement by statement, against the passages it cites. Statements
          a passage does not support are flagged in the answer.
        </p>
      </div>
      <SwitchRow
        label="Re-check flagged statements"
        description="When statements are flagged, ask the model once to cite the right passage, correct them or remove them. Costs one extra turn; the earlier draft stays in the conversation."
        checked={answers?.autoRepair ?? true}
        disabled={answers === null || saving}
        onChange={value => void toggleRepair(value)}
      />
      <AnswerCheckModelRow />
      {error && (
        <p role="alert" className="m-0 text-[12.5px] text-shodh-error break-words">
          {error}
        </p>
      )}
    </div>
  );
}

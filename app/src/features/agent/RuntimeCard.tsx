import React, { useCallback, useState } from 'react';
import { CheckCircle2, Download, Loader2, ShieldCheck, TriangleAlert } from 'lucide-react';
import { cn } from '../../lib/utils';
import { agentApi, toAgentError, useRuntimeProgress } from './useAgentSession';
import type { RuntimeInstall, RuntimeProgress } from './useAgentSession';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

type InstallState =
  | { status: 'idle' }
  | { status: 'installing'; progress: RuntimeProgress | null }
  | { status: 'done'; result: RuntimeInstall }
  | { status: 'failed'; message: string };

function megabytes(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

interface RuntimeCardProps {
  /** `invalid`: present but failed its checksum. */
  reason: 'missing' | 'invalid';
  /** Detail from the failed start, if any. */
  message?: string | null;
  onInstalled: () => void;
  compact?: boolean;
}

/**
 * Shown instead of an answer when the agent runtime is not installed (or
 * failed verification): downloads the pinned release, shows progress and the
 * checksum it matched.
 */
export function RuntimeCard({ reason, message, onInstalled, compact = false }: RuntimeCardProps) {
  const [state, setState] = useState<InstallState>({ status: 'idle' });

  useRuntimeProgress(progress => {
    setState(s => (s.status === 'installing' ? { status: 'installing', progress } : s));
  });

  const install = useCallback(async () => {
    setState({ status: 'installing', progress: null });
    try {
      const result = await agentApi.installRuntime();
      setState({ status: 'done', result });
      onInstalled();
    } catch (error) {
      setState({ status: 'failed', message: toAgentError(error).message });
    }
  }, [onInstalled]);

  const progress = state.status === 'installing' ? state.progress : null;
  const fraction = progress && progress.total ? Math.min(1, progress.downloaded / progress.total) : null;

  return (
    <section
      aria-label="Agent runtime"
      className={cn('ask-rise rounded-xl border border-shodh-border bg-shodh-surface flex flex-col gap-3', compact ? 'p-3' : 'p-4')}
    >
      <div className="flex items-start gap-2.5">
        {state.status === 'done' ? (
          <CheckCircle2 className="w-[18px] h-[18px] mt-px shrink-0 text-shodh-success" aria-hidden="true" />
        ) : (
          <TriangleAlert className="w-[18px] h-[18px] mt-px shrink-0 text-shodh-warning" aria-hidden="true" />
        )}
        <div className="min-w-0 flex flex-col gap-1">
          <p className="text-[13.5px] font-semibold text-shodh-text">
            {state.status === 'done'
              ? 'Agent runtime installed'
              : reason === 'invalid'
                ? 'The agent runtime failed its integrity check'
                : 'The agent runtime is not installed'}
          </p>
          <p className="text-[12.5px] leading-relaxed text-shodh-text-muted">
            {state.status === 'done'
              ? 'Ask your question again.'
              : 'Answers are produced by a pinned agent runtime that runs on this computer. Shodh downloads the pinned release once from GitHub and checks its SHA-256 before every launch.'}
          </p>
          {message && state.status === 'idle' && (
            <p className="text-[12px] text-shodh-text-faint break-words">{message}</p>
          )}
        </div>
      </div>

      {state.status === 'installing' && (
        <div className="flex flex-col gap-1.5" aria-live="polite">
          <div
            role="progressbar"
            aria-label="Downloading the agent runtime"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={fraction === null ? undefined : Math.round(fraction * 100)}
            className="h-1.5 rounded-full bg-shodh-raised-2 overflow-hidden"
          >
            <div
              className={cn('h-full w-full origin-left bg-shodh-accent transition-transform duration-panel', fraction === null && 'ask-breathe')}
              style={{ transform: `scaleX(${fraction ?? 0.15})` }}
            />
          </div>
          <p className="text-[11.5px] font-mono text-shodh-text-muted">
            {progress
              ? progress.total
                ? `${megabytes(progress.downloaded)} of ${megabytes(progress.total)}`
                : megabytes(progress.downloaded)
              : 'Connecting…'}
          </p>
        </div>
      )}

      {state.status === 'done' && (
        <p className="flex items-center gap-1.5 text-[11.5px] font-mono text-shodh-text-muted break-all">
          <ShieldCheck className="w-3.5 h-3.5 shrink-0 text-shodh-success" aria-hidden="true" />
          {`Verified omp ${state.result.version} · sha256 ${state.result.sha256.slice(0, 16)}…`}
        </p>
      )}

      {state.status === 'failed' && (
        <p role="alert" className="text-[12.5px] text-shodh-error break-words">{state.message}</p>
      )}

      {state.status !== 'done' && (
        <div>
          <button
            type="button"
            onClick={install}
            disabled={state.status === 'installing'}
            className={cn(
              'inline-flex items-center gap-2 h-8 px-3.5 rounded-lg bg-shodh-accent text-shodh-on-accent text-[12.5px] font-semibold hover:bg-shodh-accent-hover disabled:opacity-60 disabled:cursor-not-allowed transition-colors duration-micro',
              FOCUS_RING,
            )}
          >
            {state.status === 'installing' ? (
              <Loader2 className="w-3.5 h-3.5 agent-spin" aria-hidden="true" />
            ) : (
              <Download className="w-3.5 h-3.5" aria-hidden="true" />
            )}
            {state.status === 'installing' ? 'Installing…' : state.status === 'failed' ? 'Try again' : 'Install agent runtime'}
          </button>
        </div>
      )}
    </section>
  );
}

export default RuntimeCard;

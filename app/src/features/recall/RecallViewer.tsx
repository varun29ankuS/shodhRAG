import { useCallback, useEffect, useRef, useState } from 'react';
import { ArrowLeft, ChevronDown, ChevronUp, EyeOff, Trash2, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { agentItemKey, revealAgentItem } from '../../lib/agentHighlight';
import { recallApi } from './api';
import type { RecallOpened } from './api';
import { roleLabel, timeOf, toolLabel } from './recallModel';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';
const QUIET_BUTTON = cn(
  'h-8 px-2.5 inline-flex items-center gap-1.5 rounded-lg text-small text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro disabled:opacity-50',
  FOCUS_RING,
);

/** Turns shown before and after the receipt's turn. */
const AROUND = 4;

function errorText(err: unknown): string {
  if (typeof err === 'string') return err;
  if (err instanceof Error) return err.message;
  return String(err);
}

/**
 * A coding session opened at a receipt: the exact turn, highlighted, with the turns
 * around it, read again from the transcript (redacted). Earlier and later turns open
 * on request; the session can be removed from the index here.
 */
export function RecallViewer({ receipt, onBack }: { receipt: string; onBack: () => void }) {
  const [current, setCurrent] = useState(receipt);
  const [opened, setOpened] = useState<RecallOpened | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const live = useRef(true);

  useEffect(() => setCurrent(receipt), [receipt]);

  useEffect(() => {
    live.current = true;
    setError(null);
    recallApi
      .open(current, AROUND)
      .then(o => {
        if (!live.current) return;
        setOpened(o);
        revealAgentItem(agentItemKey('recall-turn', `${o.fileId}:${o.turn}`), {
          missed: 'Shodh opened a turn that the viewer does not show now.',
        });
      })
      .catch(e => live.current && setError(errorText(e)));
    return () => {
      live.current = false;
    };
  }, [current]);

  const move = useCallback(
    (to: number) => {
      if (!opened) return;
      const turn = Math.max(0, Math.min(opened.turnsTotal - 1, to));
      setCurrent(`${opened.fileId}:${turn}`);
    },
    [opened],
  );

  const remove = async () => {
    if (!opened) return;
    try {
      await recallApi.removeSession(opened.tool, opened.sessionId);
      notify.success('Session removed from Recall', { description: 'It is not indexed again. The transcript is not touched.' });
      onBack();
    } catch (e) {
      notify.error('The session was not removed', { description: errorText(e) });
    }
  };

  const first = opened?.turns[0]?.turn ?? 0;
  const last = opened?.turns[opened.turns.length - 1]?.turn ?? 0;

  return (
    <div className="flex flex-col gap-4">
      <header className="flex items-start justify-between gap-4">
        <div className="min-w-0 flex flex-col gap-1">
          <h1 className="m-0 text-2xl font-bold text-shodh-text break-words">{opened?.title ?? 'Coding session'}</h1>
          {opened && (
            <p className="m-0 text-sm text-shodh-text-muted">
              {[toolLabel(opened.tool), opened.project, opened.branch ? `branch ${opened.branch}` : null, `${opened.turnsTotal.toLocaleString()} turns`]
                .filter(Boolean)
                .join(' · ')}
            </p>
          )}
        </div>
        <button type="button" onClick={onBack} className={QUIET_BUTTON}>
          <ArrowLeft className="w-3.5 h-3.5" aria-hidden="true" />
          Back to Library
        </button>
      </header>

      {error && <p className="m-0 text-small text-shodh-error" role="alert">{error}</p>}

      {opened && (
        <>
          {first > 0 && (
            <div>
              <button type="button" className={QUIET_BUTTON} onClick={() => move(first - AROUND - 1)}>
                <ChevronUp className="w-3.5 h-3.5" aria-hidden="true" />
                Earlier turns
              </button>
            </div>
          )}
          <ol className="m-0 p-0 list-none flex flex-col gap-2" aria-label="Turns of the session">
            {opened.turns.map(t => (
              <li
                key={t.turn}
                data-agent-item={agentItemKey('recall-turn', `${opened.fileId}:${t.turn}`)}
                aria-current={t.target ? 'true' : undefined}
                className={cn(
                  'rounded-lg border p-3 flex flex-col gap-1.5',
                  t.target ? 'border-shodh-accent ring-1 ring-shodh-accent bg-shodh-surface' : 'border-shodh-border bg-shodh-surface',
                )}
              >
                <div className="flex flex-wrap items-baseline justify-between gap-2">
                  <span className="text-small font-semibold text-shodh-text">{`${t.turn + 1}. ${roleLabel(t.role, t.toolName)}`}</span>
                  {timeOf(t.at) && <span className="text-caption text-shodh-text-faint tabular-nums">{timeOf(t.at)}</span>}
                </div>
                {t.keptBack ? (
                  <p className="m-0 text-small text-shodh-text-muted flex items-start gap-1.5">
                    <EyeOff className="w-3.5 h-3.5 mt-0.5 shrink-0" aria-hidden="true" />
                    <span>{t.keptBack}</span>
                  </p>
                ) : (
                  <pre
                    className={cn(
                      'm-0 whitespace-pre-wrap break-words text-small text-shodh-text-secondary max-h-96 overflow-y-auto scrollbar-thin',
                      t.role === 'tool_call' || t.role === 'tool_output' ? 'font-mono' : 'font-sans',
                    )}
                  >
                    {t.text}
                  </pre>
                )}
                {t.leftOut > 0 && !t.keptBack && (
                  <p className="m-0 text-caption text-shodh-text-faint">{`${t.leftOut.toLocaleString()} characters not shown`}</p>
                )}
              </li>
            ))}
          </ol>
          {last < opened.turnsTotal - 1 && (
            <div>
              <button type="button" className={QUIET_BUTTON} onClick={() => move(last + AROUND + 1)}>
                <ChevronDown className="w-3.5 h-3.5" aria-hidden="true" />
                Later turns
              </button>
            </div>
          )}
          {opened.unreadableLines > 0 && (
            <p className="m-0 text-caption text-shodh-text-muted">
              {`${opened.unreadableLines.toLocaleString()} ${opened.unreadableLines === 1 ? 'line' : 'lines'} of this transcript could not be read.`}
            </p>
          )}
          <div className="pt-2 border-t border-shodh-border">
            {confirmRemove ? (
              <div className="flex flex-wrap items-center gap-2" role="group" aria-label="Remove this session">
                <span className="text-small text-shodh-text">Remove this session from Recall? It is not indexed again.</span>
                <button type="button" className={cn(QUIET_BUTTON, 'text-shodh-error hover:text-shodh-error border border-shodh-border')} onClick={() => void remove()}>
                  <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
                  Remove
                </button>
                <button type="button" className={QUIET_BUTTON} onClick={() => setConfirmRemove(false)}>
                  <X className="w-3.5 h-3.5" aria-hidden="true" />
                  Keep it
                </button>
              </div>
            ) : (
              <button type="button" className={cn(QUIET_BUTTON, 'text-shodh-error hover:text-shodh-error')} onClick={() => setConfirmRemove(true)}>
                <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
                Remove this session from Recall
              </button>
            )}
          </div>
        </>
      )}
    </div>
  );
}

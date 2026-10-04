import React, { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, Loader2 } from 'lucide-react';
import { notifyDepthLimit, useFocus } from '../focus/FocusContext';
import type { FocusTarget } from '../focus/focusTypes';
import { tableTarget } from '../focus/targets';
import { onResearchChanged, researchApi, toResearchError } from './api';
import { SnippetDetail } from './SnippetDetail';
import type { Snippet } from './types';

type State = { status: 'loading' } | { status: 'ready'; snippet: Snippet } | { status: 'deleted' } | { status: 'error'; message: string };

/** A snippet in the focus pop-out, loaded by id (the target keeps only id, place and text). */
export function SnippetStage({ target }: { target: Extract<FocusTarget, { kind: 'snippet' }> }) {
  const focus = useFocus();
  const [state, setState] = useState<State>({ status: 'loading' });
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let cancelled = false;
    researchApi
      .getSnippet(target.snippetId)
      .then(snippet => {
        if (!cancelled) setState({ status: 'ready', snippet });
      })
      .catch(error => {
        if (cancelled) return;
        const failure = toResearchError(error);
        setState(failure.code === 'not_found' ? { status: 'deleted' } : { status: 'error', message: failure.message });
      });
    return () => {
      cancelled = true;
    };
  }, [target.snippetId, tick]);

  // The assistant (or another view) changed it.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    onResearchChanged(change => {
      if (change.kind !== 'result' && (change.filePath === null || change.filePath === target.filePath)) setTick(t => t + 1);
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [target.filePath]);

  const openTable = useCallback(
    (rows: string[][]) => {
      const table = tableTarget(rows);
      const stack = focus?.session?.stack;
      const here = stack ? stack.levels[stack.index] : null;
      if (!focus || !table || !here) return;
      if (focus.drillDown({ target: table, parentThreadId: here.threadId, parentTurnId: null }) === 'depth') notifyDepthLimit();
    },
    [focus],
  );

  return (
    <div className="flex-1 min-h-0 overflow-y-auto scrollbar-thin bg-shodh-raised-2 p-6 flex justify-center">
      {state.status === 'ready' ? (
        <SnippetDetail
          snippet={state.snippet}
          mode="focus"
          onChanged={next => setState(next ? { status: 'ready', snippet: next } : { status: 'deleted' })}
          onOpenTable={openTable}
        />
      ) : state.status === 'loading' ? (
        <p role="status" className="flex items-center gap-2 text-[12.5px] text-shodh-text-muted">
          <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
          Opening the snippet…
        </p>
      ) : (
        <article className="w-full max-w-[640px] h-fit rounded-2xl border border-shodh-border bg-shodh-surface p-5 flex flex-col gap-3">
          <p role="alert" className="flex items-start gap-2 text-[13px] text-shodh-text-secondary">
            <AlertTriangle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
            {state.status === 'deleted'
              ? 'This snippet was deleted. Its text, as it was saved with this discussion:'
              : `The snippet could not be opened: ${state.message}. Its text, as it was saved with this discussion:`}
          </p>
          <p className="text-[13.5px] leading-relaxed text-shodh-text-secondary whitespace-pre-wrap break-words">{target.text || '(no text)'}</p>
          <p className="text-[12px] text-shodh-text-muted">{`${target.fileName || target.filePath}, page ${target.page}`}</p>
        </article>
      )}
    </div>
  );
}

import { useCallback, useEffect, useId, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { AlertTriangle, Loader2, Square, Wand2 } from 'lucide-react';
import { cn } from '../../../lib/utils';
import { FocusFrame } from '../../focus/FocusFrame';
import { useFocus, useFocusAnchor, useFocusDrill } from '../../focus/FocusContext';
import type { DiagramPlace } from '../../focus/FocusContext';
import { mermaidTarget } from '../../focus/targets';
import { useAnswerBlocks } from './answerContext';
import { checkDiagram, diagramErrorMessage, drawDiagram } from './mermaidRender';
import {
  composeMermaidFixRequest,
  createRepairLedger,
  extractMermaidReply,
  parseErrorText,
  repairOffer,
  sourceKey,
} from './mermaidRetry';
import type { RepairAttempt, RepairLedger } from './mermaidRetry';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

let ledger: RepairLedger | null = null;

/** The app-wide correction ledger, kept in localStorage when it is available. */
function repairLedger(): RepairLedger {
  if (!ledger) {
    let storage: Storage | null = null;
    try {
      storage = typeof window !== 'undefined' ? window.localStorage : null;
    } catch {
      storage = null;
    }
    ledger = createRepairLedger(storage);
  }
  return ledger;
}

function useRepairAttempt(key: string): RepairAttempt {
  const store = repairLedger();
  return useSyncExternalStore(store.subscribe, () => store.get(key), () => store.get(key));
}

type DrawState =
  | { status: 'loading' }
  | { status: 'ready'; svg: string; source: string; repaired: boolean }
  | { status: 'error'; message: string };

/**
 * A ```mermaid block. Source the parser rejects is repaired
 * deterministically first; when that is not enough, a diagram in a model
 * answer may be sent back to the model once for a correction (by itself
 * for the newest answer, else from a button). A diagram that does not draw
 * keeps "Expand & ask", with the parser's message as context.
 */
export function MermaidBlock({ source, dark }: { source: string; dark: boolean }) {
  const written = source.trim();
  const reactId = useId();
  const domId = `mmd-${reactId.replace(/[^a-zA-Z0-9]/g, '')}`;
  const focus = useFocus();
  const anchor = useFocusAnchor();
  const drill = useFocusDrill();
  const { modelAnswer } = useAnswerBlocks();
  const key = useMemo(() => sourceKey(written), [written]);
  const attempt = useRepairAttempt(key);
  const corrected = attempt.status === 'fixed' ? attempt.source : null;
  const toDraw = corrected ?? written;
  const [state, setState] = useState<DrawState>({ status: 'loading' });
  const [notice, setNotice] = useState<string | null>(null);

  // As FocusFrame: inside a side answer the side thread is where it was found.
  const place = useMemo<DiagramPlace | null>(() => {
    if (drill) return { kind: 'side', parentThreadId: drill.parentThreadId, parentTurnId: drill.parentTurnId };
    if (anchor) return { kind: 'answer', conversationId: anchor.conversationId, messageId: anchor.messageId };
    return null;
  }, [anchor, drill]);
  const eligible = modelAnswer && focus !== null && place !== null;

  useEffect(() => {
    let cancelled = false;
    setState({ status: 'loading' });
    drawDiagram(domId, toDraw, dark)
      .then(drawn => {
        if (!cancelled) setState({ status: 'ready', svg: drawn.svg, source: drawn.source, repaired: drawn.repaired });
      })
      .catch(error => {
        if (!cancelled) setState({ status: 'error', message: diagramErrorMessage(error) });
      });
    return () => {
      cancelled = true;
    };
  }, [domId, toDraw, dark]);

  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const runRepair = useCallback(async (error: string) => {
    if (!focus || !place) return;
    const store = repairLedger();
    // Recorded before the model is asked: the cap holds through remounts and restarts.
    if (!store.begin(key)) return;
    setNotice(null);
    const outcome = await focus.repairDiagram({ place, key, source: written, message: composeMermaidFixRequest(written, error) });
    if (!outcome.ok) {
      if (outcome.reason === 'busy') {
        // Nothing was sent: the attempt is not used up.
        store.release(key);
        if (mountedRef.current) setNotice(outcome.message);
        return;
      }
      store.fail(key, outcome.message);
      return;
    }
    const fixed = extractMermaidReply(outcome.reply);
    if (!fixed) {
      store.fail(key, 'The model did not reply with a diagram.');
      return;
    }
    if (fixed === written) {
      store.fail(key, 'The model returned the diagram unchanged.');
      return;
    }
    const check = await checkDiagram(fixed);
    if (check.ok) store.succeed(key, check.source);
    else store.fail(key, `Its correction does not draw either: ${parseErrorText(check.error).short}`);
  }, [focus, place, key, written]);

  const errorMessage = state.status === 'error' && toDraw === written ? state.message : null;
  const offer = errorMessage ? repairOffer({ eligible, latest: false, attempt, sourceChars: written.length }) : 'none';

  // The newest answer asks by itself, once per mount; older answers wait for a click.
  const autoTriedRef = useRef(false);
  useEffect(() => {
    if (!errorMessage || autoTriedRef.current || !focus || !place) return;
    if (repairOffer({ eligible, latest: focus.isLatestAnswer(place), attempt, sourceChars: written.length }) !== 'auto') return;
    autoTriedRef.current = true;
    void runRepair(errorMessage);
  }, [errorMessage, eligible, focus, place, attempt, written.length, runRepair]);

  const drawnSource = state.status === 'ready' ? state.source : toDraw;
  const getTarget = useCallback(() => mermaidTarget(drawnSource), [drawnSource]);

  if (state.status === 'error') {
    const shownError = state.message;
    const running = attempt.status === 'pending' && focus?.sideLive?.threadId === `${key}-repair`;
    return (
      <MermaidFailure
        source={written}
        error={shownError}
        attempt={attempt}
        running={running}
        canFix={offer !== 'none'}
        notice={notice}
        onFix={() => void runRepair(shownError)}
        onStop={() => focus?.stop()}
      />
    );
  }

  const note = state.status === 'ready'
    ? corrected
      ? 'Corrected by the model: the diagram as written did not draw.'
      : state.repaired
        ? 'Syntax repaired automatically.'
        : null
    : null;

  return (
    <FocusFrame noun="diagram" getTarget={getTarget} className="my-4">
      <figure className="m-0 rounded-xl border border-shodh-border bg-shodh-surface p-4 overflow-x-auto scrollbar-thin" aria-busy={state.status === 'loading'}>
        {state.status === 'loading' ? (
          <div className="flex items-center gap-2 h-24 justify-center text-[12.5px] text-shodh-text-muted">
            <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
            Drawing diagram…
          </div>
        ) : (
          // SVG produced by mermaid with securityLevel 'strict' (sanitised, no scripts).
          <div role="img" aria-label="Diagram" className="flex justify-center [&_svg]:max-w-full [&_svg]:h-auto" dangerouslySetInnerHTML={{ __html: state.svg }} />
        )}
        {note && (
          <figcaption className="mt-3 flex items-center gap-1.5 text-[11.5px] text-shodh-text-muted">
            <Wand2 className="w-3 h-3 shrink-0" aria-hidden="true" />
            {note}
          </figcaption>
        )}
      </figure>
    </FocusFrame>
  );
}

interface MermaidFailureProps {
  source: string;
  error: string;
  attempt: RepairAttempt;
  /** This diagram's correction is the run in progress (it can be stopped). */
  running: boolean;
  canFix: boolean;
  notice: string | null;
  onFix: () => void;
  onStop: () => void;
}

/**
 * A diagram that did not draw: the parser's message, the correction state
 * and the source. Keeps "Expand & ask" with the source and the message as
 * context, so the reader can ask what the diagram meant.
 */
function MermaidFailure({ source, error, attempt, running, canFix, notice, onFix, onStop }: MermaidFailureProps) {
  const { full, short } = useMemo(() => parseErrorText(error), [error]);
  const getTarget = useCallback(() => mermaidTarget(source, full), [source, full]);
  const detailed = full !== short;

  return (
    <FocusFrame noun="diagram" getTarget={getTarget} doubleClick={false} className="my-4">
      <figure className="m-0 rounded-xl border border-shodh-border bg-shodh-surface overflow-hidden">
        {/* Right padding keeps the text clear of the "Expand & ask" button. */}
        <figcaption className="flex items-start gap-2 pl-3 pr-32 py-2 text-[12.5px] text-shodh-text-secondary border-b border-shodh-border-subtle">
          <AlertTriangle className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
          <span>Diagram not drawn: {short}</span>
        </figcaption>
        <div role="status" aria-live="polite" className="empty:p-0 empty:border-0 flex flex-wrap items-center gap-x-3 gap-y-1.5 px-3 py-2 text-[12px] text-shodh-text-muted border-b border-shodh-border-subtle">
          {attempt.status === 'pending' && (
            <>
              <span className="inline-flex items-center gap-1.5">
                <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />
                Asking the model to correct the diagram…
              </span>
              {running && (
                <button
                  type="button"
                  onClick={onStop}
                  className={cn(
                    'inline-flex items-center gap-1 h-6 px-2 rounded-md border border-shodh-border text-[11.5px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
                    FOCUS_RING,
                  )}
                >
                  <Square className="w-3 h-3" aria-hidden="true" />
                  Stop
                </button>
              )}
            </>
          )}
          {attempt.status === 'failed' && <span>The model could not correct it: {attempt.message}</span>}
          {attempt.status === 'none' && canFix && (
            <>
              <button
                type="button"
                onClick={onFix}
                className={cn(
                  'inline-flex items-center gap-1.5 h-7 px-2.5 rounded-lg border border-shodh-border bg-shodh-raised text-[12px] font-medium text-shodh-text-secondary hover:bg-shodh-raised-2 hover:text-shodh-text transition-colors duration-micro',
                  FOCUS_RING,
                )}
              >
                <Wand2 className="w-3.5 h-3.5" aria-hidden="true" />
                Fix with the model
              </button>
              <span>{notice ?? 'Sends the diagram and the parser message to the model once.'}</span>
            </>
          )}
        </div>
        {detailed && (
          <details className="border-b border-shodh-border-subtle">
            <summary className={cn('px-3 py-1.5 text-[12px] text-shodh-text-muted cursor-pointer hover:text-shodh-text-secondary rounded-sm', FOCUS_RING)}>
              Parser message
            </summary>
            <pre className="m-0 px-3 pb-2 max-h-40 overflow-auto scrollbar-thin text-[12px] leading-relaxed font-mono text-shodh-text-muted whitespace-pre-wrap">{full}</pre>
          </details>
        )}
        <pre className="m-0 p-3 max-h-60 overflow-auto scrollbar-thin text-[12px] leading-relaxed font-mono text-shodh-text-muted whitespace-pre-wrap">{source}</pre>
      </figure>
    </FocusFrame>
  );
}

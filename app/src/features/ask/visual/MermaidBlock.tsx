import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import type { FormEvent, KeyboardEvent, ReactNode, RefObject } from 'react';
import { AlertTriangle, ChevronLeft, ChevronRight, List, Loader2, PenLine, RotateCcw, Square, Wand2 } from 'lucide-react';
import { cn } from '../../../lib/utils';
import { focusIsFree } from '../../../lib/focusReturn';
import { FRAME_ACTION, FocusFrame } from '../../focus/FocusFrame';
import { useFocus, useFocusAnchor, useFocusDrill } from '../../focus/FocusContext';
import type { DiagramPlace } from '../../focus/FocusContext';
import { mermaidTarget } from '../../focus/targets';
import { useAnswerBlocks } from './answerContext';
import { diagramLedger, useDiagramRecord } from './diagramLedger';
import { checkDiagram, diagramErrorMessage, drawDiagram } from './mermaidRender';
import {
  MAX_ATTEMPTS,
  MAX_HINT_CHARS,
  adjustOffer,
  attemptNumber,
  chosenSource,
  listOffer,
  parseDiagramList,
  parseErrorText,
  repairOffer,
  runAttempt,
  sourceKey,
  versionCaption,
  versionView,
} from './mermaidRetry';
import type { AttemptKind, DiagramList, DiagramRecord, ListOffer, RepairOffer } from './mermaidRetry';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

const QUIET_BUTTON = cn(
  'inline-flex items-center gap-1.5 h-7 px-2.5 rounded-lg border border-shodh-border bg-shodh-raised text-[12px] font-medium text-shodh-text-secondary',
  'hover:bg-shodh-raised-2 hover:text-shodh-text disabled:opacity-50 disabled:pointer-events-none transition-colors duration-micro',
  FOCUS_RING,
);

const PRIMARY_BUTTON = cn(
  'inline-flex items-center gap-1.5 h-7 px-2.5 rounded-lg bg-shodh-accent text-shodh-on-accent text-[12px] font-medium',
  'hover:bg-shodh-accent-hover disabled:opacity-50 disabled:pointer-events-none transition-colors duration-micro',
  FOCUS_RING,
);

const TEXT_BUTTON = cn(
  'inline-flex items-center h-6 px-1.5 rounded-md text-[11.5px] font-medium text-shodh-text-secondary hover:text-shodh-text hover:bg-shodh-raised transition-colors duration-micro',
  FOCUS_RING,
);

const DETAILS_SUMMARY = cn('px-3 py-1.5 text-[12px] text-shodh-text-muted cursor-pointer hover:text-shodh-text-secondary rounded-sm', FOCUS_RING);
const DETAILS_PRE = 'm-0 px-3 pb-2 max-h-40 overflow-auto scrollbar-thin text-[12px] leading-relaxed font-mono text-shodh-text-muted whitespace-pre-wrap';
const SOURCE_PRE = 'm-0 p-3 max-h-60 overflow-auto scrollbar-thin text-[12px] leading-relaxed font-mono text-shodh-text-muted whitespace-pre-wrap';

/** Notes offered under "Adjust": the changes asked for most. */
const ADJUST_SUGGESTIONS = ['Arrows the other way', 'Simpler', 'Left to right'] as const;

const RUNNING_TEXT: Record<AttemptKind, string> = {
  fix: 'Fixing the diagram',
  adjust: 'Redrawing the diagram',
  list: 'Writing it as a list',
};

const RUN_LABEL: Record<AttemptKind, string> = {
  fix: 'fixing a diagram',
  adjust: 'adjusting a diagram',
  list: 'writing a diagram as a list',
};

function lowerFirst(text: string): string {
  return text ? text[0].toLowerCase() + text.slice(1) : text;
}

function runningText(kind: AttemptKind, attempt: number): string {
  return kind === 'list' ? `${RUNNING_TEXT.list}…` : `${RUNNING_TEXT[kind]}… attempt ${attempt} of ${MAX_ATTEMPTS}`;
}

type DrawState =
  | { status: 'loading' }
  /** `stale`: another version is being drawn; this one stays until it is ready. */
  | { status: 'ready'; svg: string; source: string; repaired: boolean; stale: boolean }
  | { status: 'error'; message: string };

/**
 * A ```mermaid block. Source the parser rejects is repaired
 * deterministically first; when that is not enough, a diagram in a model
 * answer can be sent back to the model: the first fix by itself on the
 * newest answer, every later one from "Try again" with an optional note,
 * up to `MAX_ATTEMPTS`, then "Show as a list instead". A diagram that draws
 * can be adjusted ("Adjust", with a note). Each version the model drew is
 * kept; the reader steps between them, and the one chosen is the one
 * shown, printed and exported. A diagram that does not draw keeps
 * "Expand & ask", with the parser's message as context.
 */
export function MermaidBlock({ source, dark }: { source: string; dark: boolean }) {
  const written = source.trim();
  const reactId = useId();
  const domId = `mmd-${reactId.replace(/[^a-zA-Z0-9]/g, '')}`;
  const panelId = `${domId}-adjust`;
  const focus = useFocus();
  const anchor = useFocusAnchor();
  const drill = useFocusDrill();
  const { modelAnswer, renderInline } = useAnswerBlocks();
  const key = useMemo(() => sourceKey(written), [written]);
  const record = useDiagramRecord(key);
  const toDraw = chosenSource(written, record);
  const shownList = useMemo(() => (record.list ? parseDiagramList(record.list) : null), [record.list]);
  const [state, setState] = useState<DrawState>({ status: 'loading' });
  const [notice, setNotice] = useState<string | null>(null);
  const [adjusting, setAdjusting] = useState(false);
  // An attempt was asked for while this block is shown: its failure is news (a stored one is not, under a drawn diagram).
  const [ranHere, setRanHere] = useState(false);
  const adjustButtonRef = useRef<HTMLButtonElement>(null);
  const failureStatusRef = useRef<HTMLDivElement>(null);
  const focusAdjustWhenDrawnRef = useRef(false);
  const rootRef = useRef<HTMLDivElement>(null);

  // As FocusFrame: inside a side answer the side thread is where it was found.
  const place = useMemo<DiagramPlace | null>(() => {
    if (drill) return { kind: 'side', parentThreadId: drill.parentThreadId, parentTurnId: drill.parentTurnId };
    if (anchor) return { kind: 'answer', conversationId: anchor.conversationId, messageId: anchor.messageId };
    return null;
  }, [anchor, drill]);
  const eligible = modelAnswer && focus !== null && place !== null;

  useEffect(() => {
    let cancelled = false;
    // Another version of a drawn diagram: keep the one shown until the next is drawn (no jump).
    setState(s => (s.status === 'ready' ? { ...s, stale: true } : { status: 'loading' }));
    drawDiagram(domId, toDraw, dark)
      .then(drawn => {
        if (!cancelled) setState({ status: 'ready', svg: drawn.svg, source: drawn.source, repaired: drawn.repaired, stale: false });
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

  /**
   * One attempt (`runAttempt` in mermaidRetry): `base` is the diagram it is
   * about (as written for a fix or the list, the version shown for an
   * adjustment). Resolves once its outcome is in the ledger.
   */
  const attempt = useCallback(async (kind: AttemptKind, base: string, note: string, error: string | null) => {
    if (!focus || !place) return;
    setNotice(null);
    setRanHere(true);
    const result = await runAttempt({
      ledger: diagramLedger(),
      key,
      kind,
      base,
      note,
      error,
      context: () => focus.answerContext(place),
      ask: message => focus.repairDiagram({ place, key, source: base, message, label: RUN_LABEL[kind] }),
      check: checkDiagram,
    });
    if (result.busy && mountedRef.current) setNotice(result.busy);
  }, [focus, place, key]);

  const errorMessage = state.status === 'error' && toDraw === written ? state.message : null;
  const latest = errorMessage !== null && focus !== null && place !== null && focus.isLatestAnswer(place);

  // The newest answer asks for the first fix by itself, once per mount; every later attempt is the reader's.
  const autoTriedRef = useRef(false);
  useEffect(() => {
    if (!errorMessage || autoTriedRef.current || !focus || !place) return;
    if (repairOffer({ eligible, latest: focus.isLatestAnswer(place), record, sourceChars: written.length }) !== 'auto') return;
    autoTriedRef.current = true;
    void attempt('fix', written, '', errorMessage);
  }, [errorMessage, eligible, focus, place, record, written, attempt]);

  const drawnSource = state.status === 'ready' ? state.source : toDraw;
  const getTarget = useCallback(() => mermaidTarget(drawnSource), [drawnSource]);
  const live = record.running !== null && focus?.sideLive?.threadId === `${key}-repair`;
  const choose = useCallback((index: number) => diagramLedger().choose(key, index), [key]);
  const stop = useCallback(() => focus?.stop(), [focus]);

  const closeAdjust = useCallback(() => {
    setAdjusting(false);
    // Back to the trigger, which stays in place while the redraw runs.
    requestAnimationFrame(() => adjustButtonRef.current?.focus({ preventScroll: true }));
  }, []);

  const submitAdjust = useCallback((note: string) => {
    if (state.status !== 'ready') return;
    const base = state.source;
    closeAdjust();
    void attempt('adjust', base, note, null);
  }, [state, closeAdjust, attempt]);

  // A fix asked for from the failure view: once its version draws, focus goes to "Adjust" (the failure view is
  // gone), unless the reader has moved focus elsewhere meanwhile.
  useEffect(() => {
    if (state.status !== 'ready' || state.stale || !focusAdjustWhenDrawnRef.current) return;
    focusAdjustWhenDrawnRef.current = false;
    if (focusIsFree(document.activeElement, document.body, rootRef.current)) adjustButtonRef.current?.focus({ preventScroll: true });
  }, [state]);

  /** A fix from the failure view; focus follows its version only if one is drawn. */
  const fixFromFailure = useCallback(async (note: string, error: string) => {
    const before = diagramLedger().get(key).versions.length;
    focusAdjustWhenDrawnRef.current = true;
    await attempt('fix', written, note, error);
    // No new version (failed, stopped, busy): nothing will draw, so nothing may take focus later.
    if (diagramLedger().get(key).versions.length === before) focusAdjustWhenDrawnRef.current = false;
  }, [attempt, key, written]);

  if (shownList && state.status === 'error' && toDraw === written) {
    return <DiagramListView written={written} error={state.message} list={shownList} renderInline={renderInline} />;
  }

  if (state.status === 'error') {
    const shownError = state.message;
    const failing = toDraw === written;
    let offer: RepairOffer = failing ? repairOffer({ eligible, latest, record, sourceChars: written.length }) : 'none';
    // The automatic fix runs once per mount; when it could not start (another answer ran), the button is offered.
    if (offer === 'auto' && autoTriedRef.current) offer = 'button';
    const list: ListOffer = failing ? listOffer({ eligible, record, sourceChars: written.length }) : 'none';
    const toStatus = () => requestAnimationFrame(() => failureStatusRef.current?.focus({ preventScroll: true }));
    return (
      <MermaidFailure
        source={toDraw}
        error={shownError}
        record={record}
        offer={offer}
        list={list}
        live={live}
        notice={notice}
        eligible={eligible}
        statusRef={failureStatusRef}
        onAttempt={note => {
          void fixFromFailure(note, shownError);
          toStatus();
        }}
        onList={() => {
          void attempt('list', written, '', null);
          toStatus();
        }}
        onStop={stop}
        onChoose={choose}
      />
    );
  }

  const adjust = state.status === 'ready' ? adjustOffer({ eligible, record, sourceChars: drawnSource.length }) : 'none';
  const { total } = versionView(record);
  const note = state.status === 'ready' ? versionCaption(record, state.repaired) : null;
  const runningKind = record.running === 'fix' || record.running === 'adjust' ? record.running : null;
  const adjustFailure = ranHere && !record.running && record.failure && record.failure.kind !== 'list' ? record.failure : null;
  const failureText = adjustFailure ? `Attempt ${record.attempts} did not work: ${lowerFirst(adjustFailure.message)}` : null;
  const status = eligible
    ? [runningKind ? runningText(runningKind, attemptNumber(record)) : null, failureText, notice].filter(Boolean).join(' ')
    : '';
  const showBar = state.status === 'ready' && (note !== null || (eligible && (total > 1 || status !== '')));

  const actions = adjust !== 'none' ? (
    <button
      ref={adjustButtonRef}
      type="button"
      data-answer-ui=""
      aria-expanded={adjusting}
      aria-controls={adjusting ? panelId : undefined}
      aria-disabled={adjust === 'running' || undefined}
      title={adjust === 'running' ? 'The diagram is being redrawn' : 'Change this diagram: say what to change'}
      onClick={() => {
        if (adjust === 'running') return;
        if (adjusting) closeAdjust();
        else setAdjusting(true);
      }}
      className={cn(FRAME_ACTION, FOCUS_RING, adjusting && 'opacity-100 bg-shodh-raised text-shodh-text', adjust === 'running' && 'cursor-default')}
    >
      <PenLine className="w-3.5 h-3.5" aria-hidden="true" />
      Adjust
    </button>
  ) : null;

  return (
    // `contents`: no box of its own; it bounds where focus may be returned (focusIsFree).
    <div ref={rootRef} className="contents">
      <FocusFrame noun="diagram" getTarget={getTarget} actions={actions} className="my-4">
        <figure
          className="m-0 rounded-xl border border-shodh-border bg-shodh-surface overflow-hidden"
          aria-busy={state.status === 'loading' || (state.status === 'ready' && state.stale) || runningKind !== null}
        >
          <div className="p-4 overflow-x-auto scrollbar-thin">
            {state.status === 'loading' ? (
              <div className="flex items-center gap-2 h-24 justify-center text-[12.5px] text-shodh-text-muted">
                <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
                Drawing diagram…
              </div>
            ) : (
              // SVG produced by mermaid with securityLevel 'strict' (sanitised, no scripts).
              <div
                role="img"
                aria-label="Diagram"
                className={cn(
                  'flex justify-center [&_svg]:max-w-full [&_svg]:h-auto transition-opacity duration-200 motion-reduce:transition-none',
                  (state.stale || runningKind !== null) && 'opacity-50',
                )}
                dangerouslySetInnerHTML={{ __html: state.svg }}
              />
            )}
          </div>
          {eligible && (
            // Always present, so a change of state is announced.
            <span role="status" aria-live="polite" className="sr-only">{status}</span>
          )}
          {showBar && (
            <figcaption className="flex flex-wrap items-center gap-x-3 gap-y-1.5 px-3 py-2 border-t border-shodh-border-subtle text-[11.5px] text-shodh-text-muted">
              {eligible && total > 1 && <VersionStepper record={record} onChoose={choose} />}
              {note && (
                <span className="inline-flex items-center gap-1.5 min-w-0">
                  <Wand2 className="w-3 h-3 shrink-0" aria-hidden="true" />
                  <span className="truncate">{note}</span>
                </span>
              )}
              {eligible && runningKind && <RunningLine kind={runningKind} attempt={attemptNumber(record)} live={live} onStop={stop} />}
              {eligible && failureText && <span data-answer-ui="" className="text-shodh-text-secondary">{failureText}</span>}
              {eligible && notice && <span data-answer-ui="" className="text-shodh-text-secondary">{notice}</span>}
            </figcaption>
          )}
          {adjusting && adjust !== 'none' && (
            <div
              id={panelId}
              data-answer-ui=""
              // Esc anywhere in the panel (the note, a suggestion, Redraw, Cancel) closes it and nothing around it.
              data-esc-local=""
              onKeyDown={e => {
                if (e.key !== 'Escape' || e.nativeEvent.isComposing) return;
                e.preventDefault();
                e.stopPropagation();
                closeAdjust();
              }}
              className="border-t border-shodh-border-subtle bg-shodh-surface-2 px-3 py-2.5"
            >
              {adjust === 'cap' ? (
                <CapNotice onClose={closeAdjust} />
              ) : (
                <HintField
                  label="What should change?"
                  placeholder="What should change? e.g. add the decoder step"
                  submitLabel="Redraw"
                  required
                  busy={adjust === 'running'}
                  attempt={attemptNumber(record)}
                  suggestions={ADJUST_SUGGESTIONS}
                  onSubmit={submitAdjust}
                  onCancel={closeAdjust}
                  autoFocus
                />
              )}
            </div>
          )}
        </figure>
      </FocusFrame>
    </div>
  );
}

/** "Version 2 of 3" with ‹ › to step between the versions, and "Use original". */
function VersionStepper({ record, onChoose }: { record: DiagramRecord; onChoose: (index: number) => void }) {
  const { index, total } = versionView(record);
  const step = 'inline-flex items-center justify-center w-6 h-6 rounded-md text-shodh-text-secondary hover:text-shodh-text hover:bg-shodh-raised disabled:opacity-40 disabled:pointer-events-none transition-colors duration-micro';
  return (
    <span role="group" aria-label="Diagram versions" data-answer-ui="" className="inline-flex items-center gap-0.5">
      <button type="button" aria-label="Previous version" disabled={index <= 1} onClick={() => onChoose(record.chosen - 1)} className={cn(step, FOCUS_RING)}>
        <ChevronLeft className="w-3.5 h-3.5" aria-hidden="true" />
      </button>
      <span aria-live="polite" className="px-0.5 tabular-nums font-medium text-shodh-text-secondary whitespace-nowrap">
        Version {index} of {total}
      </span>
      <button type="button" aria-label="Next version" disabled={index >= total} onClick={() => onChoose(record.chosen + 1)} className={cn(step, FOCUS_RING)}>
        <ChevronRight className="w-3.5 h-3.5" aria-hidden="true" />
      </button>
      {record.chosen > 0 && (
        <button type="button" onClick={() => onChoose(0)} className={cn(TEXT_BUTTON, 'ml-1')}>
          <RotateCcw className="w-3 h-3 mr-1" aria-hidden="true" />
          Use original
        </button>
      )}
    </span>
  );
}

/** "Fixing the diagram… attempt 2 of 5" with Stop while this diagram's run is the one in progress. */
function RunningLine({ kind, attempt, live, onStop }: { kind: AttemptKind; attempt: number; live: boolean; onStop: () => void }) {
  return (
    <span data-answer-ui="" className="inline-flex flex-wrap items-center gap-2">
      <span className="inline-flex items-center gap-1.5 text-shodh-text-secondary">
        <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        {runningText(kind, attempt)}
      </span>
      {live && (
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
    </span>
  );
}

interface HintFieldProps {
  label: string;
  placeholder: string;
  submitLabel: string;
  /** A note is needed to submit (Adjust); else it is optional (Fix, Try again). */
  required: boolean;
  /** An attempt is running: nothing can be submitted. */
  busy: boolean;
  /** The number the attempt submitted will have. */
  attempt: number;
  suggestions?: readonly string[];
  onSubmit: (note: string) => void;
  /** Esc and "Cancel"; without it Esc clears the note. */
  onCancel?: () => void;
  autoFocus?: boolean;
}

/**
 * The one-line note sent with an attempt: Enter submits, Esc cancels (or
 * clears), suggestions fill it. Esc stays here (`data-esc-local`): it never
 * closes the pop-out or a panel around the answer.
 */
function HintField({ label, placeholder, submitLabel, required, busy, attempt, suggestions, onSubmit, onCancel, autoFocus }: HintFieldProps) {
  const [value, setValue] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);
  const inputId = useId();
  const metaId = `${inputId}-meta`;

  useEffect(() => {
    if (autoFocus) inputRef.current?.focus({ preventScroll: true });
  }, [autoFocus]);

  const disabled = busy || (required && !value.trim());

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (disabled) return;
    onSubmit(value);
    setValue('');
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key !== 'Escape' || e.nativeEvent.isComposing) return;
    e.preventDefault();
    e.stopPropagation();
    if (onCancel) onCancel();
    else setValue('');
  };

  return (
    <form onSubmit={submit} data-answer-ui="" className="flex flex-col gap-2 min-w-0 w-full">
      <div className="flex flex-wrap items-center gap-2">
        <label htmlFor={inputId} className="sr-only">{label}</label>
        <input
          ref={inputRef}
          id={inputId}
          type="text"
          value={value}
          maxLength={MAX_HINT_CHARS}
          onChange={e => setValue(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder={placeholder}
          aria-describedby={metaId}
          autoComplete="off"
          spellCheck
          data-esc-local=""
          className={cn(
            'flex-1 min-w-[12rem] h-8 px-2.5 rounded-lg border border-shodh-border bg-shodh-ground text-[12.5px] text-shodh-text placeholder:text-shodh-text-faint',
            'focus-visible:outline-none focus-visible:border-shodh-border-strong focus-visible:ring-2 focus-visible:ring-ring',
          )}
        />
        <button type="submit" disabled={disabled} className={PRIMARY_BUTTON}>
          {submitLabel}
        </button>
        {onCancel && (
          <button type="button" onClick={onCancel} className={QUIET_BUTTON}>
            Cancel
          </button>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-1.5">
        {suggestions?.map(s => (
          <button
            key={s}
            type="button"
            aria-pressed={value === s}
            onClick={() => {
              setValue(s);
              inputRef.current?.focus({ preventScroll: true });
            }}
            className={cn(
              'h-6 px-2 rounded-full border border-shodh-border-subtle bg-shodh-surface text-[11.5px] text-shodh-text-secondary',
              'hover:border-shodh-border hover:text-shodh-text transition-colors duration-micro',
              value === s && 'border-shodh-border-strong text-shodh-text',
              FOCUS_RING,
            )}
          >
            {s}
          </button>
        ))}
        <span id={metaId} className="ml-auto text-[11px] text-shodh-text-muted tabular-nums">
          Attempt {attempt} of {MAX_ATTEMPTS}
        </span>
      </div>
    </form>
  );
}

/** "Adjust" with no attempts left. */
function CapNotice({ onClose }: { onClose: () => void }) {
  const closeRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    closeRef.current?.focus({ preventScroll: true });
  }, []);
  return (
    <div
      data-esc-local=""
      onKeyDown={e => {
        if (e.key !== 'Escape') return;
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }}
      className="flex flex-wrap items-center gap-2 text-[12px] text-shodh-text-secondary"
    >
      <span className="flex-1 min-w-[12rem]">
        {`This diagram has been reworked ${MAX_ATTEMPTS} times, the most it can be. Ask a follow-up question to change it further.`}
      </span>
      <button ref={closeRef} type="button" onClick={onClose} className={QUIET_BUTTON}>
        Close
      </button>
    </div>
  );
}

interface MermaidFailureProps {
  /** The source that did not draw. */
  source: string;
  error: string;
  record: DiagramRecord;
  offer: RepairOffer;
  list: ListOffer;
  /** This diagram's attempt is the run in progress (it can be stopped). */
  live: boolean;
  notice: string | null;
  eligible: boolean;
  statusRef: RefObject<HTMLDivElement | null>;
  onAttempt: (note: string) => void;
  onList: () => void;
  onStop: () => void;
  onChoose: (index: number) => void;
}

/**
 * A diagram that did not draw: why, what the model can still do (fix it,
 * try again with a note, or write it as a list), the versions when the
 * reader stepped back to the one as written, and the source. Keeps
 * "Expand & ask" with the source and the message as context, so the
 * reader can ask what the diagram meant.
 */
function MermaidFailure({ source, error, record, offer, list, live, notice, eligible, statusRef, onAttempt, onList, onStop, onChoose }: MermaidFailureProps) {
  const { full, short } = useMemo(() => parseErrorText(error), [error]);
  const getTarget = useCallback(() => mermaidTarget(source, full), [source, full]);
  const detailed = full !== short;
  const { total } = versionView(record);
  const failure = record.failure;
  const lastFix = failure && failure.kind !== 'list' ? failure : null;
  const lastList = failure && failure.kind === 'list' ? failure : null;
  const showControls = offer !== 'none' || list !== 'none' || notice !== null;

  let body: ReactNode = null;
  if (offer === 'running' || offer === 'auto' || list === 'running') {
    // 'auto': the first fix starts right after this render.
    body = <RunningLine kind={record.running ?? 'fix'} attempt={attemptNumber(record)} live={live} onStop={onStop} />;
  } else if (offer === 'button' || offer === 'retry') {
    body = (
      <div className="flex flex-col gap-2 w-full">
        <p className="m-0 text-shodh-text-secondary">
          {offer === 'retry' && lastFix
            ? `Attempt ${record.attempts} did not work: ${lowerFirst(lastFix.message)}`
            : 'The model can try to fix it. Say what it should show, or leave the note empty.'}
        </p>
        <HintField
          label="What should it show? (optional)"
          placeholder="What should it show? (optional)"
          submitLabel={offer === 'button' ? 'Fix' : 'Try again'}
          required={false}
          busy={false}
          attempt={attemptNumber(record)}
          onSubmit={onAttempt}
        />
      </div>
    );
  } else if (list === 'offer' || list === 'retry') {
    body = (
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <span className="text-shodh-text-secondary">
          {list === 'retry' && lastList
            ? `The list did not work either: ${lowerFirst(lastList.message)}`
            : `Tried ${MAX_ATTEMPTS} times; it still does not draw.`}
        </span>
        <button type="button" onClick={onList} className={QUIET_BUTTON}>
          <List className="w-3.5 h-3.5" aria-hidden="true" />
          {list === 'retry' ? 'Try the list again' : 'Show as a list instead'}
        </button>
      </div>
    );
  } else if (list === 'exhausted') {
    body = <span className="text-shodh-text-secondary">It could not be written as a list either. Use Expand &amp; ask to ask what it meant.</span>;
  }

  return (
    <FocusFrame noun="diagram" getTarget={getTarget} doubleClick={false} className="my-4">
      <figure className="m-0 rounded-xl border border-shodh-border bg-shodh-surface overflow-hidden">
        {/* Right padding keeps the text clear of the "Expand & ask" button. */}
        <figcaption className="flex items-start gap-2 pl-3 pr-32 py-2 text-[12.5px] text-shodh-text-secondary border-b border-shodh-border-subtle">
          <AlertTriangle className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
          <span>Diagram not drawn: {short}</span>
        </figcaption>
        {eligible && total > 1 && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-1.5 text-[11.5px] text-shodh-text-muted border-b border-shodh-border-subtle">
            <VersionStepper record={record} onChoose={onChoose} />
            <span>{versionCaption(record, false)}</span>
          </div>
        )}
        {showControls && (
          <div
            ref={statusRef}
            tabIndex={-1}
            role="status"
            aria-live="polite"
            data-answer-ui=""
            className="flex flex-wrap items-center gap-x-3 gap-y-1.5 px-3 py-2.5 text-[12px] text-shodh-text-muted border-b border-shodh-border-subtle focus:outline-none"
          >
            {body}
            {notice && <span className="text-shodh-text-secondary">{notice}</span>}
          </div>
        )}
        {lastFix?.detail && offer === 'retry' && (
          <details className="border-b border-shodh-border-subtle">
            <summary className={DETAILS_SUMMARY}>Why the model's version did not draw</summary>
            <pre className={DETAILS_PRE}>{lastFix.detail}</pre>
          </details>
        )}
        {detailed && (
          <details className="border-b border-shodh-border-subtle">
            <summary className={DETAILS_SUMMARY}>Parser message</summary>
            <pre className={DETAILS_PRE}>{full}</pre>
          </details>
        )}
        <pre className={SOURCE_PRE}>{source}</pre>
      </figure>
    </FocusFrame>
  );
}

/** A list, nested as the model wrote it; item text keeps its citation pills. */
function ListTree({ list, renderInline, depth }: { list: DiagramList; renderInline: (text: string) => ReactNode; depth: number }) {
  const Tag = list.ordered ? 'ol' : 'ul';
  return (
    <Tag
      className={cn(
        'm-0 pl-5 space-y-1 marker:text-shodh-text-muted',
        list.ordered ? 'list-decimal' : depth === 0 ? 'list-disc' : 'list-[circle]',
        depth > 0 && 'mt-1',
      )}
    >
      {list.items.map((item, i) => (
        <li key={i} className="pl-0.5">
          {renderInline(item.text)}
          {item.children && <ListTree list={item.children} renderInline={renderInline} depth={depth + 1} />}
        </li>
      ))}
    </Tag>
  );
}

/** A diagram that never drew, shown as the list of its content the model wrote; the source stays a click away. */
function DiagramListView({ written, error, list, renderInline }: { written: string; error: string; list: DiagramList; renderInline: (text: string) => ReactNode }) {
  const { full } = useMemo(() => parseErrorText(error), [error]);
  const getTarget = useCallback(() => mermaidTarget(written, full), [written, full]);
  return (
    <FocusFrame noun="diagram" getTarget={getTarget} doubleClick={false} className="my-4">
      <figure className="m-0 rounded-xl border border-shodh-border bg-shodh-surface overflow-hidden">
        {/* Right padding keeps the first item clear of the "Expand & ask" button. */}
        <div className="pl-4 pr-32 py-3 text-[13.5px] leading-relaxed text-shodh-text-secondary">
          <ListTree list={list} renderInline={renderInline} depth={0} />
        </div>
        <figcaption className="flex items-center gap-1.5 px-3 py-2 border-t border-shodh-border-subtle text-[11.5px] text-shodh-text-muted">
          <List className="w-3 h-3 shrink-0" aria-hidden="true" />
          Shown as a list: the diagram could not be drawn.
        </figcaption>
        <details data-answer-ui="" className="border-t border-shodh-border-subtle">
          <summary className={DETAILS_SUMMARY}>Diagram source</summary>
          <pre className={DETAILS_PRE}>{full}</pre>
          <pre className={SOURCE_PRE}>{written}</pre>
        </details>
      </figure>
    </FocusFrame>
  );
}

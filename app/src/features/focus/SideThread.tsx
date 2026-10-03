import React, { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { CornerLeftUp, FileText, Loader2, MessageSquarePlus, Sparkles, TextSelect, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { AgentComposer } from '../agent/AgentComposer';
import type { AgentComposerHandle } from '../agent/AgentComposer';
import { passageHits } from '../agent/citations';
import { fromPersisted, pendingApproval } from '../agent/reducer';
import type { TranscriptState } from '../agent/reducer';
import { Transcript } from '../agent/Transcript';
import { useChatSession } from '../ask/ChatSessionContext';
import { MessageContentRenderer } from '../ask/MessageContentRenderer';
import type { SearchHit } from '../ask/types';
import { UserText } from '../ask/UserText';
import { contextLabel } from './contextBlock';
import type { FocusExtras, FocusKind, FocusThread, ThreadTurn } from './focusTypes';
import { FocusDrillProvider, useFocus } from './FocusContext';
import type { OpenFocus, SummaryResult } from './FocusContext';
import { splitFollowups, stripFollowups } from './followups';
import { threadSummary } from './threadStore';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const STICK_TO_BOTTOM_PX = 120;

const SUGGESTIONS: Record<FocusKind, string[]> = {
  mermaid: ['Explain this diagram step by step', 'What is missing or ambiguous here?'],
  chart: ['What stands out in this chart?', 'Which values are outliers?'],
  svg: ['Walk me through this sketch', 'Is anything in this drawing physically wrong or missing?'],
  plot: ['How do the sliders change the curve, and why?', 'Derive the plotted formula step by step'],
  simulation: ['Explain the physics this simulation shows', 'What happens at the extremes of the sliders?'],
  equation: ['Explain each term of this equation', 'Where does this come from?'],
  table: ['Summarize this table', 'Which rows stand out, and why?'],
  image: ['Describe what this image shows'],
  source: ['Explain this passage in plain words', 'What does the rest of the document say about this?'],
  task: ['Break this task into concrete steps', 'What do I need before I can start?'],
  selection: ['Explain this in plain words', 'Why does this matter here?'],
};

const noCitation = () => undefined;

/** The transcript without its followups block (also while it streams in). */
function withoutFollowups(transcript: TranscriptState): TranscriptState {
  let changed = false;
  const blocks = transcript.blocks.map(block => {
    if (block.kind !== 'text') return block;
    const text = stripFollowups(block.text);
    if (text === block.text) return block;
    changed = true;
    return { ...block, text };
  });
  return changed ? { ...transcript, blocks } : transcript;
}

function UserTurn({ turn, onOpenThread }: { turn: ThreadTurn; onOpenThread: (threadId: string) => void }) {
  if (turn.summaryOf) {
    const from = turn.summaryOf;
    return (
      <div className="ask-rise self-stretch rounded-[14px] border border-shodh-border bg-shodh-surface-2 px-3 py-2.5 flex flex-col gap-1.5">
        <div className="flex items-center gap-1.5 min-w-0 text-[11.5px] text-shodh-text-muted">
          <CornerLeftUp className="w-3.5 h-3.5 shrink-0 text-shodh-accent-text" aria-hidden="true" />
          <span className="truncate">{`Brought back from “${from.label}”`}</span>
          <button
            type="button"
            onClick={() => onOpenThread(from.threadId)}
            className={cn('ml-auto shrink-0 h-6 px-2 rounded-md text-[11.5px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text', FOCUS_RING)}
          >
            Open
          </button>
        </div>
        <div className="text-[13.5px] leading-[1.5] text-shodh-text">
          <UserText text={turn.content} compact markdown />
        </div>
      </div>
    );
  }
  return (
    <div className="ask-rise self-end max-w-[92%] flex flex-col items-end gap-1">
      <div className="px-3 py-2 rounded-[14px] bg-shodh-raised-2 text-[13.5px] leading-[1.5] text-shodh-text">
        <UserText text={turn.content} compact />
      </div>
      {(turn.selection || typeof turn.page === 'number') && (
        <span className="text-[11px] text-shodh-text-faint">
          {[turn.selection ? 'with selected text' : null, typeof turn.page === 'number' ? `page ${turn.page}` : null].filter(Boolean).join(' · ')}
        </span>
      )}
    </div>
  );
}

const AnswerTurn = React.memo(function AnswerTurn({
  transcript,
  content,
  onOpenCitation,
  onDecide,
  onRuntimeInstalled,
  onOpenSettings,
}: {
  transcript: TranscriptState | null;
  content: string;
  onOpenCitation: (hit: SearchHit) => void;
  onDecide?: (stepId: string, approved: boolean) => void;
  onRuntimeInstalled: () => void;
  onOpenSettings: () => void;
}) {
  const shown = useMemo(() => (transcript ? withoutFollowups(transcript) : null), [transcript]);
  const hits = useMemo(() => (shown ? passageHits(shown.passages) : []), [shown]);
  const open = useCallback((hit: SearchHit) => onOpenCitation(hit), [onOpenCitation]);
  if (!shown) {
    return <MessageContentRenderer compact content={stripFollowups(content)} hits={[]} onOpenCitation={open} />;
  }
  return (
    <Transcript
      compact
      transcript={shown}
      hits={hits}
      onOpenCitation={open}
      onDecide={onDecide}
      onRuntimeInstalled={onRuntimeInstalled}
      onOpenSettings={onOpenSettings}
    />
  );
});

type SummaryState =
  | { status: 'writing' }
  | { status: 'editing'; text: string; note: string | null };

function fallbackNote(result: Extract<SummaryResult, { ok: false }>): string {
  switch (result.reason) {
    case 'busy':
      return 'Another answer is running, so the summary below is a shortened excerpt. Edit it before posting.';
    case 'stopped':
      return 'Stopped. The summary below is a shortened excerpt instead.';
    case 'failed':
      return `The summary could not be written${result.message ? ` (${result.message})` : ''}. Below is a shortened excerpt instead.`;
  }
}

export interface SideThreadProps {
  /** The level shown. */
  open: OpenFocus;
  /** 0 for the object opened from the conversation; nested levels count up. */
  depth: number;
  /** Object of the level above, for "Bring back up". */
  parentLabel: string | null;
  thread: FocusThread | null;
  extras: FocusExtras;
  onClearSelection: () => void;
  /** A citation in a side answer was opened. */
  onOpenCitation: (hit: SearchHit) => void;
  /** The pop-out should close (summary posted, settings opened). */
  onDone: () => void;
}

/**
 * "Ask about this": a sub-discussion about the focused object. Questions go
 * through the agent with the object (and, when nested, the outer levels)
 * attached as context; answers render like the main conversation, and
 * visuals or selected text inside them open as nested levels. Answers end
 * with up to three suggested next questions. A summary of the discussion
 * can be brought back to the level above (or the main conversation).
 * Only one answer runs at a time.
 */
export function SideThread({ open, depth, parentLabel, thread, extras, onClearSelection, onOpenCitation, onDone }: SideThreadProps) {
  const focus = useFocus();
  const session = useChatSession();
  const headingId = useId();
  const [draft, setDraft] = useState('');
  const [summary, setSummary] = useState<SummaryState | null>(null);
  const [posting, setPosting] = useState(false);
  const composerRef = useRef<AgentComposerHandle>(null);
  const summaryRef = useRef<HTMLTextAreaElement>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const mine = focus?.sideLive?.threadId === open.threadId ? focus.sideLive : null;
  const live = mine?.purpose === 'answer' ? mine.transcript : null;
  const writingSummary = mine?.purpose === 'summary';
  const running = live !== null;
  const waitingStep = live ? pendingApproval(live) : null;
  const turns = useMemo(() => thread?.turns ?? [], [thread]);
  // Restored once per thread change, not on every keystroke in the composer.
  const restored = useMemo(() => new Map(turns.map(t => [t.id, fromPersisted(t.transcript)])), [turns]);
  const label = open.target.label;
  const nested = depth > 0;
  const parentName = parentLabel ?? 'the level above';

  const mainRunning = session.streamingConversationId !== null;
  const otherSide = session.sideRun !== null && session.sideRun.threadId !== open.threadId;
  const blockedReason = running
    ? null
    : writingSummary
      ? 'Writing the summary. Ask again once it is ready.'
      : mainRunning
        ? 'Waiting for the current answer in the conversation to finish. Your question stays here; send it once that answer is done.'
        : otherSide
          ? `Waiting for the side question about “${session.sideRun?.label}” to finish.`
          : null;

  const hasAnswer = turns.some(t => t.role === 'assistant' && t.content.trim().length > 0);
  const inActiveConversation = session.activeConversationId === open.conversationId;
  // Posting to the conversation starts a main answer; bringing up only adds to the parent thread.
  const canPost = nested
    ? hasAnswer && !running && !writingSummary
    : hasAnswer && !running && !writingSummary && !mainRunning && session.sideRun === null && inActiveConversation;
  const postBlocked = !hasAnswer
    ? 'Ask a question first.'
    : nested
      ? running || writingSummary ? 'Available when this answer is done.' : null
      : !inActiveConversation
        ? 'Open this conversation to add to it.'
        : running || mainRunning || session.sideRun !== null
          ? 'Available when no answer is running.'
          : null;

  // Suggested next questions of the latest answer.
  const lastAnswer = useMemo(() => {
    for (let i = turns.length - 1; i >= 0; i--) if (turns[i].role === 'assistant') return turns[i];
    return null;
  }, [turns]);
  const followups = useMemo(() => {
    if (!lastAnswer) return [];
    return lastAnswer.followups ?? splitFollowups(lastAnswer.content).followups;
  }, [lastAnswer]);

  // Keep the newest turn in view unless the reader scrolled up.
  const lastCount = useRef(0);
  useLayoutEffect(() => {
    const el = scrollerRef.current;
    if (!el) return;
    const isNew = turns.length !== lastCount.current;
    lastCount.current = turns.length;
    const distance = el.scrollHeight - el.scrollTop - el.clientHeight;
    if (isNew || distance < STICK_TO_BOTTOM_PX) el.scrollTop = el.scrollHeight;
  }, [turns.length, live]);

  const editingSummary = summary?.status === 'editing';
  useEffect(() => {
    if (editingSummary) summaryRef.current?.focus();
  }, [editingSummary]);

  const send = useCallback(async (question: string): Promise<boolean> => {
    const text = question.trim();
    if (!text || !focus || running || blockedReason) return false;
    const started = await focus.ask(open, text, extras);
    if (!started) {
      notify.error('The question was not sent', { description: 'Another answer started first. Try again when it finishes.' });
      return false;
    }
    onClearSelection();
    return true;
  }, [focus, running, blockedReason, open, extras, onClearSelection]);

  const submit = useCallback(async () => {
    if (await send(draft)) {
      setDraft('');
      composerRef.current?.focus();
    }
  }, [draft, send]);

  const askSuggested = useCallback(async (question: string) => {
    if (blockedReason || running) {
      setDraft(question);
      composerRef.current?.focus();
      return;
    }
    if (await send(question)) composerRef.current?.focus();
  }, [blockedReason, running, send]);

  // Each summary request has a number; Cancel retires it, so a result that arrives later is dropped.
  const summaryRun = useRef(0);
  const startSummary = useCallback(async () => {
    if (!focus || !thread || !canPost) return;
    const run = ++summaryRun.current;
    setSummary({ status: 'writing' });
    const result = await focus.summarize(open, nested ? { kind: 'parent', label: parentName } : { kind: 'conversation' });
    if (!mounted.current || run !== summaryRun.current) return;
    if ('text' in result) setSummary({ status: 'editing', text: result.text, note: null });
    else setSummary({ status: 'editing', text: threadSummary(thread), note: fallbackNote(result) });
  }, [focus, thread, canPost, open, nested, parentName]);

  const cancelSummary = useCallback(() => {
    summaryRun.current += 1;
    if (summary?.status === 'writing') focus?.stop();
    setSummary(null);
  }, [summary, focus]);

  const post = useCallback(async () => {
    if (summary?.status !== 'editing') return;
    const text = summary.text.trim();
    if (!text || !canPost || !focus) return;
    if (nested) {
      if (focus.bringBack(open, text)) {
        notify.success(`Added to the discussion about “${parentName}”`);
        setSummary(null);
      } else {
        notify.error('The summary could not be added', { description: 'The discussion above is no longer available.' });
      }
      return;
    }
    setPosting(true);
    try {
      await session.send(text, { spaceId: null, spaceName: null }, {
        sideSummary: { threadId: open.threadId, label, parentMessageId: open.parentMessageId },
      });
      notify.success('Added to the conversation');
      setSummary(null);
      onDone();
    } finally {
      if (mounted.current) setPosting(false);
    }
  }, [summary, canPost, focus, nested, open, parentName, session, label, onDone]);

  const { setRuntimeInstalled } = session;
  const markInstalled = useCallback(() => setRuntimeInstalled(true), [setRuntimeInstalled]);
  const openSettings = useCallback(() => {
    window.dispatchEvent(new CustomEvent('switchTab', { detail: 'settings' }));
    onDone();
  }, [onDone]);
  const openThread = useCallback((threadId: string) => focus?.jumpToThread(threadId), [focus]);

  const contextNote = contextLabel(open.target, extras);
  const selection = extras.selection?.trim() || null;
  const destination = nested ? `the discussion about “${parentName}”` : 'the conversation';

  return (
    <section aria-labelledby={headingId} className="h-full min-h-0 flex flex-col bg-shodh-surface">
      <header className="shrink-0 flex items-center gap-2 px-4 pt-3 pb-2 border-b border-shodh-border-subtle">
        <h3 id={headingId} className="text-[13px] font-semibold text-shodh-text mr-auto">
          Ask about this
        </h3>
        <button
          type="button"
          onClick={() => void startSummary()}
          disabled={!canPost || summary !== null}
          title={postBlocked ?? `Post a summary of this side discussion to ${destination}`}
          className={cn(
            'h-7 px-2 inline-flex items-center gap-1.5 rounded-lg text-[12px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-45 disabled:cursor-not-allowed transition-colors duration-micro',
            FOCUS_RING,
          )}
        >
          {nested ? <CornerLeftUp className="w-3.5 h-3.5" aria-hidden="true" /> : <MessageSquarePlus className="w-3.5 h-3.5" aria-hidden="true" />}
          {nested ? 'Bring back up' : 'Add to main conversation'}
        </button>
      </header>

      {summary !== null && (
        <div className="shrink-0 max-h-[65%] overflow-y-auto scrollbar-thin px-4 py-3 border-b border-shodh-border-subtle bg-shodh-surface-2 flex flex-col gap-2">
          {summary.status === 'writing' ? (
            <div className="flex items-center gap-2 text-[12.5px] text-shodh-text-secondary" role="status">
              <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />
              <span className="mr-auto">Writing a summary that keeps the key equations and terms…</span>
              <button
                type="button"
                onClick={cancelSummary}
                className={cn('h-7 px-2.5 rounded-lg text-[12px] text-shodh-text-secondary hover:bg-shodh-raised', FOCUS_RING)}
              >
                Stop
              </button>
            </div>
          ) : (
            <>
              <label htmlFor={`${headingId}-summary`} className="text-[12px] font-medium text-shodh-text-secondary">
                {nested
                  ? `Added to ${destination} as a note; it goes with your next question there. Edit it first if you like.`
                  : 'Posted to the conversation as a side-discussion card; the agent replies there. Edit it first if you like.'}
              </label>
              {summary.note && <p className="text-[12px] text-shodh-warning">{summary.note}</p>}
              <textarea
                id={`${headingId}-summary`}
                ref={summaryRef}
                data-esc-local=""
                value={summary.text}
                onChange={e => setSummary({ status: 'editing', text: e.target.value, note: summary.note })}
                onKeyDown={e => {
                  if (e.key === 'Escape') {
                    e.preventDefault();
                    setSummary(null);
                  }
                }}
                rows={6}
                className="w-full resize-y rounded-lg border border-shodh-border-strong bg-shodh-surface px-3 py-2 font-mono text-[12.5px] leading-[1.5] text-shodh-text focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              />
              {summary.text.trim() && (
                <div className="rounded-lg border border-shodh-border-subtle bg-shodh-surface px-3 py-2">
                  <p className="mb-1 text-[11px] font-medium uppercase tracking-wider text-shodh-text-faint">Preview</p>
                  <div className="text-[13px] text-shodh-text">
                    <MessageContentRenderer compact content={summary.text} hits={[]} onOpenCitation={noCitation} citations={false} />
                  </div>
                </div>
              )}
              <div className="flex items-center justify-end gap-2">
                <button
                  type="button"
                  onClick={cancelSummary}
                  className={cn('h-8 px-3 rounded-lg text-[12.5px] text-shodh-text-secondary hover:bg-shodh-raised transition-colors duration-micro', FOCUS_RING)}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  onClick={() => void post()}
                  disabled={!summary.text.trim() || !canPost || posting}
                  className={cn(
                    'h-8 px-3 rounded-lg bg-shodh-accent text-shodh-on-accent text-[12.5px] font-semibold hover:bg-shodh-accent-hover disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
                    FOCUS_RING,
                  )}
                >
                  {nested ? 'Add to the discussion above' : 'Post to conversation'}
                </button>
              </div>
            </>
          )}
        </div>
      )}

      <div ref={scrollerRef} className="flex-1 min-h-0 overflow-y-auto scrollbar-thin px-4 py-3 flex flex-col gap-3" role="log" aria-label={`Side discussion about ${label}`}>
        {turns.length === 0 && !live && (
          <div className="flex flex-col gap-3 py-2">
            <p className="text-[13px] leading-relaxed text-shodh-text-muted">
              Ask a question about <span className="text-shodh-text-secondary">{label}</span>. Each question includes the {contextNote}
              {nested ? ' and how you got here' : ''}, and the discussion stays attached to it.
            </p>
            <ul className="flex flex-col gap-1.5" aria-label="Suggested questions">
              {SUGGESTIONS[open.target.kind].map(s => (
                <li key={s}>
                  <button
                    type="button"
                    onClick={() => {
                      setDraft(s);
                      composerRef.current?.focus();
                    }}
                    className={cn(
                      'w-full text-left px-3 py-2 rounded-[10px] border border-shodh-border bg-shodh-surface-2 text-[12.5px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
                      FOCUS_RING,
                    )}
                  >
                    {s}
                  </button>
                </li>
              ))}
            </ul>
          </div>
        )}
        {turns.map(turn =>
          turn.role === 'user' ? (
            <UserTurn key={turn.id} turn={turn} onOpenThread={openThread} />
          ) : (
            <FocusDrillProvider key={turn.id} value={thread ? { parentThreadId: thread.id, parentTurnId: turn.id } : null}>
              {/* Visuals and selected text in this answer open as a nested level. */}
              <div
                className="ask-rise"
                data-ask-scope={thread ? 'thread' : undefined}
                data-thread-id={thread?.id}
                data-turn-id={turn.id}
              >
                <AnswerTurn
                  transcript={restored.get(turn.id) ?? null}
                  content={turn.content}
                  onOpenCitation={onOpenCitation}
                  onRuntimeInstalled={markInstalled}
                  onOpenSettings={openSettings}
                />
              </div>
            </FocusDrillProvider>
          ),
        )}
        {live && (
          <div aria-busy="true">
            <AnswerTurn
              transcript={live}
              content=""
              onOpenCitation={onOpenCitation}
              onDecide={focus?.approve}
              onRuntimeInstalled={markInstalled}
              onOpenSettings={openSettings}
            />
          </div>
        )}
        {!live && followups.length > 0 && (
          <ul className="flex flex-col items-start gap-1.5" aria-label="Suggested next questions">
            {followups.map(q => (
              <li key={q} className="max-w-full">
                <button
                  type="button"
                  onClick={() => void askSuggested(q)}
                  title={blockedReason ? 'Put this question in the composer' : 'Ask this'}
                  className={cn(
                    'max-w-full inline-flex items-start gap-1.5 px-2.5 py-1.5 rounded-[10px] border border-shodh-border bg-shodh-surface text-left text-[12.5px] leading-snug text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
                    FOCUS_RING,
                  )}
                >
                  <Sparkles className="w-3.5 h-3.5 mt-[1px] shrink-0 text-shodh-accent-text" aria-hidden="true" />
                  <span className="break-words">{q}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="shrink-0 border-t border-shodh-border px-3 pt-2 pb-3 flex flex-col gap-1.5">
        <div className="flex items-center gap-1.5 min-w-0 px-1 text-[11.5px] text-shodh-text-muted">
          {selection ? (
            <span className="inline-flex items-center gap-1 min-w-0 max-w-full h-6 pl-2 pr-0.5 rounded-full bg-shodh-accent-soft text-shodh-text-secondary">
              <TextSelect className="w-3 h-3 shrink-0" aria-hidden="true" />
              <span className="truncate" title={selection}>{`Selected: “${selection}”`}</span>
              <button
                type="button"
                onClick={onClearSelection}
                aria-label="Do not attach the selected text"
                className={cn('w-5 h-5 shrink-0 inline-flex items-center justify-center rounded-full hover:bg-shodh-raised', FOCUS_RING)}
              >
                <X className="w-3 h-3" aria-hidden="true" />
              </button>
            </span>
          ) : (
            <span className="inline-flex items-center gap-1 min-w-0">
              <FileText className="w-3 h-3 shrink-0" aria-hidden="true" />
              <span className="truncate">{`Includes the ${contextNote}${nested ? ' and how you got here' : ''}`}</span>
            </span>
          )}
        </div>
        <AgentComposer
          id={`focus-composer-${open.seq}`}
          ref={composerRef}
          value={draft}
          onChange={setDraft}
          onSubmit={() => void submit()}
          onSteer={() => undefined}
          onStop={() => focus?.stop()}
          running={running}
          canSteer={false}
          approvalPending={waitingStep !== null}
          blockedReason={blockedReason}
          placeholder={turns.length === 0 ? 'Ask about this…' : 'Ask a follow-up…'}
          runningPlaceholder="Answering… Esc to stop"
          compact
        />
      </div>
    </section>
  );
}

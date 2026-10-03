import React, { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { FileText, MessageSquarePlus, TextSelect, X } from 'lucide-react';
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
import { contextLabel } from './contextBlock';
import type { FocusExtras, FocusKind, FocusThread, ThreadTurn } from './focusTypes';
import { useFocus } from './FocusContext';
import type { OpenFocus } from './FocusContext';
import { threadSummary } from './threadStore';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const STICK_TO_BOTTOM_PX = 120;

const SUGGESTIONS: Record<FocusKind, string[]> = {
  mermaid: ['Explain this diagram step by step', 'What is missing or ambiguous here?'],
  chart: ['What stands out in this chart?', 'Which values are outliers?'],
  equation: ['Explain each term of this equation', 'Where does this come from?'],
  table: ['Summarize this table', 'Which rows stand out, and why?'],
  image: ['Describe what this image shows'],
  source: ['Explain this passage in plain words', 'What does the rest of the document say about this?'],
  task: ['Break this task into concrete steps', 'What do I need before I can start?'],
  selection: ['Explain this in plain words', 'Why does this matter here?'],
};

function UserTurn({ turn }: { turn: ThreadTurn }) {
  return (
    <div className="ask-rise self-end max-w-[92%] flex flex-col items-end gap-1">
      <div className="px-3 py-2 rounded-[14px] bg-shodh-raised-2 text-[13.5px] leading-[1.5] text-shodh-text whitespace-pre-wrap break-words">
        {turn.content}
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
  const hits = useMemo(() => (transcript ? passageHits(transcript.passages) : []), [transcript]);
  const open = useCallback((hit: SearchHit) => onOpenCitation(hit), [onOpenCitation]);
  if (!transcript) {
    return <MessageContentRenderer compact content={content} hits={[]} onOpenCitation={open} />;
  }
  return (
    <Transcript
      compact
      transcript={transcript}
      hits={hits}
      onOpenCitation={open}
      onDecide={onDecide}
      onRuntimeInstalled={onRuntimeInstalled}
      onOpenSettings={onOpenSettings}
    />
  );
});

export interface SideThreadProps {
  open: OpenFocus;
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
 * through the agent with the object attached as context; answers render
 * like the main conversation (citations, visuals). Only one answer runs at
 * a time: while the main conversation is answering, the composer keeps the
 * draft and explains why it cannot send yet.
 */
export function SideThread({ open, thread, extras, onClearSelection, onOpenCitation, onDone }: SideThreadProps) {
  const focus = useFocus();
  const session = useChatSession();
  const headingId = useId();
  const [draft, setDraft] = useState('');
  const [summary, setSummary] = useState<string | null>(null);
  const [posting, setPosting] = useState(false);
  const composerRef = useRef<AgentComposerHandle>(null);
  const summaryRef = useRef<HTMLTextAreaElement>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);

  const live = focus?.sideLive?.threadId === open.threadId ? focus.sideLive.transcript : null;
  const running = live !== null;
  const waitingStep = live ? pendingApproval(live) : null;
  const turns = useMemo(() => thread?.turns ?? [], [thread]);
  // Restored once per thread change, not on every keystroke in the composer.
  const restored = useMemo(() => new Map(turns.map(t => [t.id, fromPersisted(t.transcript)])), [turns]);
  const label = open.target.label;

  const mainRunning = session.streamingConversationId !== null;
  const otherSide = session.sideRun !== null && session.sideRun.threadId !== open.threadId;
  const blockedReason = running
    ? null
    : mainRunning
      ? 'Waiting for the current answer in the conversation to finish. Your question stays here; send it once that answer is done.'
      : otherSide
        ? `Waiting for the side question about “${session.sideRun?.label}” to finish.`
        : null;

  const hasAnswer = turns.some(t => t.role === 'assistant' && t.content.trim().length > 0);
  const inActiveConversation = session.activeConversationId === open.conversationId;
  const canPost = hasAnswer && !running && !mainRunning && session.sideRun === null && inActiveConversation;
  const postBlocked = !hasAnswer
    ? 'Ask a question first.'
    : !inActiveConversation
      ? 'Open this conversation to add to it.'
      : running || mainRunning || session.sideRun !== null
        ? 'Available when no answer is running.'
        : null;

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

  const editingSummary = summary !== null;
  useEffect(() => {
    if (editingSummary) summaryRef.current?.focus();
  }, [editingSummary]);

  const submit = useCallback(async () => {
    const text = draft.trim();
    if (!text || !focus || running || blockedReason) return;
    const started = await focus.ask(open, text, extras);
    if (!started) {
      notify.error('The question was not sent', { description: 'Another answer started first. Try again when it finishes.' });
      return;
    }
    setDraft('');
    onClearSelection();
    composerRef.current?.focus();
  }, [draft, focus, running, blockedReason, open, extras, onClearSelection]);

  const post = useCallback(async () => {
    const text = summary?.trim();
    if (!text || !canPost) return;
    setPosting(true);
    try {
      await session.send(text, { spaceId: null, spaceName: null });
      notify.success('Added to the conversation');
      setSummary(null);
      onDone();
    } finally {
      setPosting(false);
    }
  }, [summary, canPost, session, onDone]);

  const { setRuntimeInstalled } = session;
  const markInstalled = useCallback(() => setRuntimeInstalled(true), [setRuntimeInstalled]);
  const openSettings = useCallback(() => {
    window.dispatchEvent(new CustomEvent('switchTab', { detail: 'settings' }));
    onDone();
  }, [onDone]);

  const contextNote = contextLabel(open.target, extras);
  const selection = extras.selection?.trim() || null;

  return (
    <section aria-labelledby={headingId} className="h-full min-h-0 flex flex-col bg-shodh-surface">
      <header className="shrink-0 flex items-center gap-2 px-4 pt-3 pb-2 border-b border-shodh-border-subtle">
        <h3 id={headingId} className="text-[13px] font-semibold text-shodh-text mr-auto">
          Ask about this
        </h3>
        <button
          type="button"
          onClick={() => setSummary(thread ? threadSummary(thread) : '')}
          disabled={!canPost || summary !== null}
          title={postBlocked ?? 'Post a summary of this side discussion to the conversation'}
          className={cn(
            'h-7 px-2 inline-flex items-center gap-1.5 rounded-lg text-[12px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-45 disabled:cursor-not-allowed transition-colors duration-micro',
            FOCUS_RING,
          )}
        >
          <MessageSquarePlus className="w-3.5 h-3.5" aria-hidden="true" />
          Add to main conversation
        </button>
      </header>

      {summary !== null && (
        <div className="shrink-0 px-4 py-3 border-b border-shodh-border-subtle bg-shodh-surface-2 flex flex-col gap-2">
          <label htmlFor={`${headingId}-summary`} className="text-[12px] font-medium text-shodh-text-secondary">
            Posted to the conversation as your message; the agent replies there. Edit it first if you like.
          </label>
          <textarea
            id={`${headingId}-summary`}
            ref={summaryRef}
            value={summary}
            onChange={e => setSummary(e.target.value)}
            rows={5}
            className="w-full resize-y rounded-lg border border-shodh-border-strong bg-shodh-surface px-3 py-2 text-[13px] leading-[1.5] text-shodh-text focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          />
          <div className="flex items-center justify-end gap-2">
            <button
              type="button"
              onClick={() => setSummary(null)}
              className={cn('h-8 px-3 rounded-lg text-[12.5px] text-shodh-text-secondary hover:bg-shodh-raised transition-colors duration-micro', FOCUS_RING)}
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={post}
              disabled={!summary.trim() || !canPost || posting}
              className={cn(
                'h-8 px-3 rounded-lg bg-shodh-accent text-shodh-on-accent text-[12.5px] font-semibold hover:bg-shodh-accent-hover disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
                FOCUS_RING,
              )}
            >
              Post to conversation
            </button>
          </div>
        </div>
      )}

      <div ref={scrollerRef} className="flex-1 min-h-0 overflow-y-auto scrollbar-thin px-4 py-3 flex flex-col gap-3" role="log" aria-label={`Side discussion about ${label}`}>
        {turns.length === 0 && !live && (
          <div className="flex flex-col gap-3 py-2">
            <p className="text-[13px] leading-relaxed text-shodh-text-muted">
              Ask a question about <span className="text-shodh-text-secondary">{label}</span>. Each question includes the {contextNote}, and the discussion stays attached to it.
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
            <UserTurn key={turn.id} turn={turn} />
          ) : (
            <div key={turn.id} className="ask-rise">
              <AnswerTurn
                transcript={restored.get(turn.id) ?? null}
                content={turn.content}
                onOpenCitation={onOpenCitation}
                onRuntimeInstalled={markInstalled}
                onOpenSettings={openSettings}
              />
            </div>
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
              <span className="truncate">{`Includes the ${contextNote}`}</span>
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

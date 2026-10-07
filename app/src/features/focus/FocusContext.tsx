import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { notify } from '../../lib/notify';
import type { AgentEventEnvelope } from '../agent/events';
import { answerText, initialTranscript, isLive, reduceAll, toPersisted } from '../agent/reducer';
import type { TranscriptAction, TranscriptState } from '../agent/reducer';
import { toAgentError, useAgentSession } from '../agent/useAgentSession';
import { useChatSession } from '../ask/ChatSessionContext';
import { composeSideQuestion } from './contextBlock';
import { answerScope } from '../workspaces/model';
import type { FocusExtras, FocusTarget, FocusThread, ThreadAnchor, ThreadTurn } from './focusTypes';
import { canPush, initialStack, stackReducer } from './focusStack';
import type { StackState } from './focusStack';
import { splitFollowups } from './followups';
import { cleanSummary, composeSummaryRequest } from './summary';
import type { SummaryDestination } from './summary';
import {
  alternateTurns,
  appendTurn,
  createLocalThreadStore,
  MAX_LOCAL_THREADS,
  mergeThreads,
  readThreads,
  scopeForLevels,
  sideSessionKey,
  sideSessionKeys,
  threadHistory,
  threadsFromMetadata,
  upsertThread,
} from './threadStore';
import type { HistoryTurnLike } from './threadStore';
import { ancestorsFor, findChildThread, threadPath } from './threadTree';
import { toVisualError, visualsApi } from '../visuals/api';
import { composeRefineRequest, parseRefineResponse } from '../visuals/refine';
import type { RefineResult } from '../visuals/refine';
import type { VisualRecord, VisualRef } from '../visuals/model';
import { recordTarget, visualRef } from '../visuals/model';
import { recordAnswerVisuals } from '../visuals/recording';
import { FocusOverlay } from './FocusOverlay';
import { SelectionAsk } from './SelectionAsk';
import { currentValues } from './liveValues';
import { onApprovalAnswered } from '../inbox/approvalBus';

/** How long an interrupt may take before the side answer is closed locally. */
const INTERRUPT_TIMEOUT_MS = 5_000;
/** How long a diagram correction may run before it is stopped. */
const REPAIR_TIMEOUT_MS = 120_000;

function newId(prefix: string): string {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

function browserStorage(): Storage | null {
  try {
    return typeof window !== 'undefined' ? window.localStorage : null;
  } catch {
    return null;
  }
}

/** What to show in the pop-out. */
export interface FocusRequest {
  target: FocusTarget;
  /** Conversation the side thread belongs to; the active one when omitted. */
  conversationId?: string | null;
  /** Message the object came from; null for objects outside the conversation. */
  parentMessageId: string | null;
  /** Reopen this thread (from a "replies" chip); nested threads open with their outer levels. */
  threadId?: string | null;
  /** Element focus returns to when the pop-out closes. */
  trigger?: HTMLElement | null;
  /** The gallery visual shown (version switcher, refine, export). */
  visual?: VisualRef | null;
}

/** An object opened inside the current level (a visual or a selection in a side answer). */
export interface DrillRequest {
  target: FocusTarget;
  /** Thread of the level it was found in. */
  parentThreadId: string;
  /** Answer it was found in; null when found in the level's object itself. */
  parentTurnId: string | null;
}

/** "opened", "depth" when the depth cap is reached, "closed" when no pop-out is open. */
export type DrillResult = 'opened' | 'depth' | 'closed';

/** One level of the pop-out: the object, resolved to a conversation and thread. */
export interface OpenFocus {
  target: FocusTarget;
  conversationId: string;
  parentMessageId: string | null;
  threadId: string;
  /** Thread this level was opened from (null at the first level). */
  parentThreadId: string | null;
  /** Answer of the parent thread it was opened from. */
  parentTurnId: string | null;
  trigger: HTMLElement | null;
  /** Unique per level, so a reopened object remounts its state. */
  seq: number;
  /** The gallery visual this level shows, when it was opened from the gallery. */
  visual: VisualRef | null;
}

/** The pop-out as opened: its levels. `key` stays while levels change. */
export interface FocusSession {
  key: number;
  stack: StackState<OpenFocus>;
}

export type FocusNavigation = { type: 'back' } | { type: 'forward' } | { type: 'jump'; index: number };

export type SummaryResult =
  | { ok: true; text: string }
  | { ok: false; reason: 'busy' | 'stopped' | 'failed'; message?: string };

type SidePurpose = 'answer' | 'summary' | 'refine' | 'repair';

/** What a refine run produced. */
export type RefineOutcome =
  | RefineResult
  | { ok: false; reason: 'busy' | 'stopped' | 'failed'; message: string };

/** Where a diagram that did not draw was found: a main answer, or an answer in a side thread. */
export type DiagramPlace =
  | { kind: 'answer'; conversationId: string; messageId: string }
  | { kind: 'side'; parentThreadId: string; parentTurnId: string };

/** A request for the model to correct a diagram the mermaid parser rejected. */
export interface DiagramRepairRequest {
  place: DiagramPlace;
  /** Identifies the diagram (a hash of its source); one run per key at a time. */
  key: string;
  /** The diagram as written. */
  source: string;
  /** The message sent (source, parser message and the reply format asked for). */
  message: string;
}

/** The model's reply, or why there is none. `busy`: another answer holds the agent, nothing was sent. */
export type DiagramRepairOutcome =
  | { ok: true; reply: string }
  | { ok: false; reason: 'busy' | 'stopped' | 'failed'; message: string };

/** The side answer (or summary) being produced. */
interface SideLive {
  runId: string;
  threadId: string;
  anchor: ThreadAnchor;
  purpose: SidePurpose;
  sessionId: string | null;
  transcript: TranscriptState;
  queue: TranscriptAction[];
  frame: number | null;
  settled: boolean;
  abortTimer: number | null;
  /** Thread links stored with a thread created by this question. */
  links: { parentThreadId: string | null; parentTurnId: string | null };
  /** Receives the result of a summary run. */
  onSummary: ((result: SummaryResult) => void) | null;
  /** Receives the result of a refine run, with the kind and source it revised. */
  onRefine: { kind: VisualRecord['kind']; previous: string; resolve: (result: RefineOutcome) => void } | null;
  /** Receives the reply of a diagram correction run. */
  onRepair: ((outcome: DiagramRepairOutcome) => void) | null;
}

export interface SideLiveView {
  threadId: string;
  purpose: SidePurpose;
  transcript: TranscriptState;
}

export interface FocusContextValue {
  openFocus: (request: FocusRequest) => void;
  closeFocus: () => void;
  /** Open an object found inside the current level as a new level. */
  drillDown: (request: DrillRequest) => DrillResult;
  /** Move between the pop-out's levels. */
  navigate: (move: FocusNavigation) => void;
  /** Show a thread of the open pop-out's answer, with its outer levels (exploration map). */
  jumpToThread: (threadId: string) => void;
  /** The open pop-out's levels, or null. */
  session: FocusSession | null;
  /**
   * Threads with no parent message (about a task, or a document the agent
   * opened), with their nested threads, stored on the conversation.
   */
  localThreads: (conversationId: string) => FocusThread[];
  /** Every thread kept with a message (or on the conversation when `parentMessageId` is null). */
  locationThreads: (conversationId: string, parentMessageId: string | null) => FocusThread[];
  /** A thread by id, wherever it is kept. */
  findThread: (conversationId: string, parentMessageId: string | null, threadId: string) => FocusThread | null;
  /** Ask a side question. False when it could not start (another answer is running). */
  ask: (open: OpenFocus, question: string, extras: FocusExtras) => Promise<boolean>;
  /** Ask the agent for a summary of a level's discussion (not added to the thread). */
  summarize: (open: OpenFocus, destination: SummaryDestination) => Promise<SummaryResult>;
  /** Post a summary of a nested level into its parent discussion and go up to it. */
  bringBack: (open: OpenFocus, text: string) => boolean;
  /**
   * Ask the agent for a revised version of a gallery visual (not added to the
   * discussion). Resolves to the revised source or why there is none.
   */
  refine: (open: OpenFocus, instruction: string) => Promise<RefineOutcome>;
  /** Show another version of the gallery visual as the pop-out's only level. */
  showVisualVersion: (record: VisualRecord) => void;
  /**
   * Ask the model, in a session of its own, for a corrected version of a
   * diagram that did not draw. Never enters the conversation or a thread;
   * resolves `busy` without sending anything when another answer is running.
   */
  repairDiagram: (request: DiagramRepairRequest) => Promise<DiagramRepairOutcome>;
  /** Whether the answer at `place` is the newest one where it is shown (read on call, no re-render). */
  isLatestAnswer: (place: DiagramPlace) => boolean;
  /** Interrupt the side answer. */
  stop: () => void;
  /** Answer a pending approval of the side answer. */
  approve: (stepId: string, approved: boolean) => void;
  /** The side answer being produced, for rendering. */
  sideLive: SideLiveView | null;
}

const FocusContext = createContext<FocusContextValue | null>(null);

/** The pop-out API, or null outside the provider. */
export function useFocus(): FocusContextValue | null {
  return useContext(FocusContext);
}

/**
 * Where visuals inside an answer come from. Provided around each answer in
 * the Ask view; visuals show their focus affordance only inside it (or
 * inside a side answer, see `FocusDrillProvider`).
 */
export interface FocusAnchorValue {
  conversationId: string;
  messageId: string;
}

const FocusAnchorContext = createContext<FocusAnchorValue | null>(null);

/**
 * Anchors visuals to an answer. Always rendered (toggle `enabled`), so an
 * answer that finishes streaming keeps its subtree instead of remounting.
 */
export function FocusAnchorProvider({
  conversationId,
  messageId,
  enabled,
  children,
}: { conversationId: string | null; messageId: string; enabled: boolean; children: React.ReactNode }) {
  const value = useMemo(
    () => (enabled && conversationId ? { conversationId, messageId } : null),
    [enabled, conversationId, messageId],
  );
  return <FocusAnchorContext.Provider value={value}>{children}</FocusAnchorContext.Provider>;
}

export function useFocusAnchor(): FocusAnchorValue | null {
  return useContext(FocusAnchorContext);
}

/** A finished side answer, so visuals inside it open as a nested level. */
export interface FocusDrillValue {
  parentThreadId: string;
  parentTurnId: string;
}

const FocusDrillContext = createContext<FocusDrillValue | null>(null);

export function FocusDrillProvider({ value, children }: { value: FocusDrillValue | null; children: React.ReactNode }) {
  return <FocusDrillContext.Provider value={value}>{children}</FocusDrillContext.Provider>;
}

export function useFocusDrill(): FocusDrillValue | null {
  return useContext(FocusDrillContext);
}

/** The message shown when the depth cap is reached. */
export function notifyDepthLimit(): void {
  notify.info('This is as deep as the pop-out goes', {
    description: 'Ask about it here, or go up a level (Alt+←) and open it from there.',
  });
}

/**
 * Hosts the focus pop-out and runs side-thread questions.
 *
 * A side question goes through the same agent session commands as the Ask
 * view (`agent_start` / `agent_send`), in a session of its own keyed by the
 * conversation and thread, so side questions never enter the main
 * conversation's agent memory. The first question of a fresh session
 * replays the main conversation up to the parent message plus the thread's
 * earlier turns. Only one answer runs at a time across main and side
 * (`claimSideRun`).
 *
 * The pop-out has levels: an object inside a side answer opens as a nested
 * level whose thread records the thread and answer it came from. Each
 * nested question carries a compact chain of its outer levels.
 */
export function FocusProvider({ children }: { children: React.ReactNode }) {
  const chat = useChatSession();
  const { conversations, activeConversationId, messages, updateThreads, updateFocusThreads, claimSideRun, releaseSideRun, setRuntimeInstalled } = chat;

  const [session, setSession] = useState<FocusSession | null>(null);
  const [sideLive, setSideLive] = useState<SideLiveView | null>(null);
  const seqRef = useRef(0);
  const sessionRef = useRef(session);
  sessionRef.current = session;
  const liveRef = useRef<SideLive | null>(null);

  const conversationsRef = useRef(conversations);
  conversationsRef.current = conversations;
  /** The workspace of a conversation: side questions search what its chats search. */
  const workspaceOf = (conversationId: string): string | null =>
    conversationsRef.current.find(c => c.id === conversationId)?.workspaceId ?? null;
  const messagesRef = useRef(messages);
  messagesRef.current = messages;
  const activeIdRef = useRef(activeConversationId);
  activeIdRef.current = activeConversationId;

  const nextSeq = () => {
    seqRef.current += 1;
    return seqRef.current;
  };

  // Identity changes with the conversations (not with a streaming answer's
  // tokens, which stay in the chat view until the answer is committed), so
  // consumers (e.g. a task's reply count) re-render when threads change.
  const localThreads = useCallback(
    (conversationId: string): FocusThread[] =>
      readThreads(conversations.find(c => c.id === conversationId)?.focusThreads),
    [conversations],
  );

  // Once per launch, after conversations load: move threads that earlier
  // versions kept in this device's localStorage into their conversation.
  // The old key is removed only after the conversation saved; if that save
  // is superseded or fails, the next launch merges again (by thread id, so
  // nothing is duplicated). Keys of conversations deleted since are dropped.
  const migratedRef = useRef(false);
  useEffect(() => {
    if (migratedRef.current || conversations.length === 0) return;
    migratedRef.current = true;
    const legacy = createLocalThreadStore(browserStorage());
    for (const conversation of conversations) {
      const old = legacy.list(conversation.id);
      if (old.length === 0) continue;
      updateFocusThreads(
        conversation.id,
        prev => mergeThreads(readThreads(prev), old),
        { touch: false, onSaved: () => legacy.clear(conversation.id) },
      );
    }
    legacy.prune(new Set(conversations.map(c => c.id)));
  }, [conversations, updateFocusThreads]);

  /** Threads stored on a message: the visible copy first, else the stored conversation. */
  const messageThreads = useCallback((conversationId: string, messageId: string): FocusThread[] => {
    if (activeIdRef.current === conversationId) {
      const visible = messagesRef.current.find(m => m.id === messageId);
      if (visible) return visible.threads ?? [];
    }
    const stored = conversationsRef.current.find(c => c.id === conversationId)?.messages.find(m => m.id === messageId);
    return stored ? threadsFromMetadata(stored.metadata) : [];
  }, []);

  const locationThreads = useCallback(
    (conversationId: string, parentMessageId: string | null): FocusThread[] =>
      parentMessageId ? messageThreads(conversationId, parentMessageId) : localThreads(conversationId),
    // Message threads are read through refs: the reader keeps its identity
    // while an answer streams, so answers' visuals do not re-render per token.
    // The pop-out re-renders with this provider and always reads fresh threads.
    [messageThreads, localThreads],
  );

  const findThread = useCallback((conversationId: string, parentMessageId: string | null, threadId: string): FocusThread | null =>
    locationThreads(conversationId, parentMessageId).find(t => t.id === threadId) ?? null,
  [locationThreads]);

  /** Change one thread wherever it is kept, creating it (with its links) on first use. */
  const mutateThread = useCallback((
    anchor: ThreadAnchor,
    threadId: string,
    change: (thread: FocusThread) => FocusThread,
    links: { parentThreadId: string | null; parentTurnId: string | null } = { parentThreadId: null, parentTurnId: null },
  ) => {
    const now = new Date().toISOString();
    const fresh = (): FocusThread => ({
      id: threadId,
      anchor,
      turns: [],
      createdAt: now,
      updatedAt: now,
      ...(links.parentThreadId ? { parentThreadId: links.parentThreadId } : {}),
      ...(links.parentThreadId && links.parentTurnId ? { parentTurnId: links.parentTurnId } : {}),
    });
    if (anchor.parentMessageId) {
      updateThreads(anchor.conversationId, anchor.parentMessageId, list => {
        const current = list.find(t => t.id === threadId) ?? fresh();
        return upsertThread(list, change(current));
      });
      return;
    }
    // Applied to the latest stored list, so two changes in one tick compose.
    updateFocusThreads(anchor.conversationId, prev => {
      const list = readThreads(prev);
      const current = list.find(t => t.id === threadId) ?? fresh();
      return upsertThread(list, change(current)).slice(-MAX_LOCAL_THREADS);
    });
  }, [updateThreads, updateFocusThreads]);

  /** Main-conversation turns up to (and including) the parent message. */
  const mainTurns = useCallback((conversationId: string, parentMessageId: string | null): HistoryTurnLike[] => {
    const stored = conversationsRef.current.find(c => c.id === conversationId)?.messages ?? [];
    const end = parentMessageId ? stored.findIndex(m => m.id === parentMessageId) : stored.length - 1;
    const upTo = end >= 0 ? stored.slice(0, end + 1) : stored;
    return upTo
      .filter((m): m is typeof m & { role: 'user' | 'assistant' } => m.role === 'user' || m.role === 'assistant')
      .map(m => ({ role: m.role, content: m.content }));
  }, []);

  const settle = useCallback((live: SideLive) => {
    live.settled = true;
    if (live.abortTimer !== null) {
      window.clearTimeout(live.abortTimer);
      live.abortTimer = null;
    }
    if (live.frame !== null) {
      cancelAnimationFrame(live.frame);
      live.frame = null;
    }
    const transcript = live.transcript;
    if (live.purpose === 'refine') {
      const refine = live.onRefine;
      if (refine) {
        const result: RefineOutcome = transcript.status === 'completed'
          ? parseRefineResponse(answerText(transcript), refine.kind, refine.previous)
          : transcript.status === 'aborted'
            ? { ok: false, reason: 'stopped', message: 'The refinement was stopped.' }
            : { ok: false, reason: 'failed', message: transcript.error ?? 'The refinement failed.' };
        refine.resolve(result);
      }
    } else if (live.purpose === 'repair') {
      // Not part of any discussion: the reply goes back to the diagram only.
      const reply = answerText(transcript);
      live.onRepair?.(transcript.status === 'completed' && reply.trim()
        ? { ok: true, reply }
        : {
            ok: false,
            reason: transcript.status === 'aborted' ? 'stopped' : 'failed',
            message: transcript.error ?? (transcript.status === 'aborted' ? 'The correction was stopped.' : 'The model gave no correction.'),
          });
    } else if (live.purpose === 'summary') {
      const text = cleanSummary(answerText(transcript));
      const result: SummaryResult = transcript.status === 'completed' && text
        ? { ok: true, text }
        : { ok: false, reason: transcript.status === 'aborted' ? 'stopped' : 'failed', message: transcript.error ?? undefined };
      live.onSummary?.(result);
    } else {
      // The followups block is kept out of the stored answer; its questions are kept beside it.
      const { body, followups } = splitFollowups(answerText(transcript));
      const answer: ThreadTurn = {
        id: newId('turn'),
        role: 'assistant',
        content: body,
        timestamp: new Date().toISOString(),
        transcript: toPersisted(transcript) as unknown as Record<string, unknown>,
        ...(followups.length > 0 && transcript.status === 'completed' ? { followups } : {}),
      };
      mutateThread(live.anchor, live.threadId, thread => appendTurn(thread, answer), live.links);
      if (transcript.status === 'completed') {
        recordAnswerVisuals({
          conversationId: live.anchor.conversationId,
          messageId: live.anchor.parentMessageId,
          threadId: live.threadId,
          turnId: answer.id,
        }, body);
      }
    }
    if (transcript.errorCode === 'runtime_missing') setRuntimeInstalled(false);
    if (liveRef.current === live) liveRef.current = null;
    setSideLive(null);
    releaseSideRun(live.threadId);
  }, [mutateThread, releaseSideRun, setRuntimeInstalled]);

  const flush = useCallback((live: SideLive) => {
    if (live.frame !== null) {
      cancelAnimationFrame(live.frame);
      live.frame = null;
    }
    if (live.settled || live.queue.length === 0) return;
    const actions = live.queue;
    live.queue = [];
    const next = reduceAll(live.transcript, actions);
    if (next === live.transcript) return;
    live.transcript = next;
    if (isLive(next)) {
      setSideLive({ threadId: live.threadId, purpose: live.purpose, transcript: next });
      return;
    }
    settle(live);
  }, [settle]);

  const enqueue = useCallback((live: SideLive, action: TranscriptAction, immediate = false) => {
    if (live.settled) return;
    live.queue.push(action);
    if (immediate) {
      flush(live);
    } else if (live.frame === null) {
      live.frame = requestAnimationFrame(() => {
        live.frame = null;
        flush(live);
      });
    }
  }, [flush]);

  const onEnvelope = useCallback((envelope: AgentEventEnvelope) => {
    const live = liveRef.current;
    if (!live || live.settled) return;
    const { event } = envelope;
    if (event.runId !== live.runId) return;
    if (live.sessionId !== null && envelope.sessionId !== live.sessionId) return;
    // A side question never switches the app's view: the pop-out stays put.
    if (event.type === 'navigated') return;
    const immediate = event.type === 'run_started' || event.type === 'run_finished' || event.type === 'approval_requested';
    enqueue(live, event, immediate);
  }, [enqueue]);

  const api = useAgentSession(onEnvelope);

  useEffect(() => () => {
    const live = liveRef.current;
    if (live && live.frame !== null) cancelAnimationFrame(live.frame);
    if (live && live.abortTimer !== null) window.clearTimeout(live.abortTimer);
  }, []);

  /** Starts a side run (answer or summary) and returns it, or null when another run holds the agent. */
  const beginRun = useCallback((
    level: OpenFocus,
    purpose: SidePurpose,
    label: string,
    onSummary: ((result: SummaryResult) => void) | null,
    onRefine: SideLive['onRefine'] = null,
    onRepair: SideLive['onRepair'] = null,
  ): SideLive | null => {
    if (liveRef.current) return null;
    const { conversationId, parentMessageId, threadId } = level;
    if (!claimSideRun({ conversationId, threadId, label })) return null;
    const runId = newId('run');
    const live: SideLive = {
      runId,
      threadId,
      anchor: { conversationId, parentMessageId, target: level.target },
      purpose,
      sessionId: null,
      transcript: initialTranscript(runId, Date.now()),
      queue: [],
      frame: null,
      settled: false,
      abortTimer: null,
      links: { parentThreadId: level.parentThreadId, parentTurnId: level.parentTurnId },
      onSummary,
      onRefine,
      onRepair,
    };
    liveRef.current = live;
    setSideLive({ threadId, purpose, transcript: live.transcript });
    return live;
  }, [claimSideRun]);

  /**
   * Objects of the levels from the outermost down to `level`. Read from the
   * open pop-out, which holds every level even before its thread is saved;
   * else from the stored thread chain.
   */
  const levelTargets = useCallback((level: OpenFocus, list: readonly FocusThread[]): FocusTarget[] => {
    const levels = sessionRef.current?.stack.levels ?? [];
    const at = levels.findIndex(l => l.seq === level.seq);
    if (at >= 0) return levels.slice(0, at + 1).map(l => l.target);
    const outer = level.parentThreadId ? threadPath(list, level.parentThreadId).map(t => t.anchor.target) : [];
    return [...outer, level.target];
  }, []);

  const ask = useCallback(async (level: OpenFocus, question: string, extras: FocusExtras): Promise<boolean> => {
    const text = question.trim();
    if (!text || liveRef.current) return false;
    const { conversationId, parentMessageId, threadId } = level;
    const list = locationThreads(conversationId, parentMessageId);
    const prior = list.find(t => t.id === threadId)?.turns ?? [];
    const live = beginRun(level, 'answer', level.target.label, null);
    if (!live) return false;

    const selection = extras.selection?.trim() || undefined;
    const userTurn: ThreadTurn = {
      id: newId('turn'),
      role: 'user',
      content: text,
      timestamp: new Date().toISOString(),
      ...(selection ? { selection } : {}),
      ...(typeof extras.page === 'number' ? { page: extras.page } : {}),
    };
    mutateThread(live.anchor, threadId, thread => appendTurn({ ...thread, anchor: live.anchor }, userTurn), live.links);

    // Summaries brought back since the last answer have not reached the
    // agent yet: they travel with this question, not in replayed history.
    let lastAnswer = -1;
    prior.forEach((t, i) => {
      if (t.role === 'assistant') lastAnswer = i;
    });
    const fresh = prior.slice(lastAnswer + 1).filter(t => t.role === 'user' && t.summaryOf);
    const notes = fresh.map(t => ({ label: t.summaryOf?.label ?? '', text: t.content }));
    const replay = prior
      .filter(t => !fresh.includes(t))
      .map(t => ({
        role: t.role,
        content: t.summaryOf ? `Brought back from the nested discussion about "${t.summaryOf.label}":\n\n${t.content}` : t.content,
      }));
    const history = alternateTurns(threadHistory(mainTurns(conversationId, parentMessageId), replay));
    const ancestors = ancestorsFor(list, level.parentThreadId, level.parentTurnId);
    const scopeTargets = levelTargets(level, list);
    const instructions = conversationsRef.current.find(c => c.id === conversationId)?.systemPrompt?.trim() || null;
    try {
      const sessionId = await api.start(sideSessionKey(conversationId, threadId), instructions, conversationId);
      if (live.settled) return true;
      live.sessionId = sessionId;
      setRuntimeInstalled(true);
      const message = composeSideQuestion(level.target, text, extras, { ancestors, notes, followups: true });
      await api.send(sessionId, message, live.runId, history, answerScope(scopeForLevels(scopeTargets, extras), workspaceOf(conversationId), false));
    } catch (error) {
      const failure = toAgentError(error);
      enqueue(live, { type: 'local_failed', error: failure.message, code: failure.code, atMs: Date.now() }, true);
    }
    return true;
  }, [api, beginRun, enqueue, levelTargets, locationThreads, mainTurns, mutateThread, setRuntimeInstalled]);

  const summarize = useCallback((level: OpenFocus, destination: SummaryDestination): Promise<SummaryResult> => {
    const thread = findThread(level.conversationId, level.parentMessageId, level.threadId);
    if (!thread || !thread.turns.some(t => t.role === 'assistant' && t.content.trim())) {
      return Promise.resolve({ ok: false, reason: 'failed', message: 'There is no answer to summarise yet.' });
    }
    return new Promise<SummaryResult>(resolve => {
      const live = beginRun(level, 'summary', `summary of ${level.target.label}`, resolve);
      if (!live) {
        resolve({ ok: false, reason: 'busy' });
        return;
      }
      const instructions = conversationsRef.current.find(c => c.id === level.conversationId)?.systemPrompt?.trim() || null;
      // A session of its own: the request never enters the thread's agent memory.
      const key = sideSessionKey(level.conversationId, `${level.threadId}-summary`);
      void (async () => {
        try {
          const sessionId = await api.start(key, instructions, level.conversationId);
          if (live.settled) return;
          live.sessionId = sessionId;
          setRuntimeInstalled(true);
          await api.send(sessionId, composeSummaryRequest(thread, destination), live.runId, [], answerScope(null, workspaceOf(level.conversationId), false));
        } catch (error) {
          const failure = toAgentError(error);
          enqueue(live, { type: 'local_failed', error: failure.message, code: failure.code, atMs: Date.now() }, true);
        }
      })();
    });
  }, [api, beginRun, enqueue, findThread, setRuntimeInstalled]);

  const refine = useCallback((level: OpenFocus, instruction: string): Promise<RefineOutcome> => {
    const text = instruction.trim();
    const visual = level.visual;
    if (!visual || !text) {
      return Promise.resolve({ ok: false, reason: 'failed', message: 'Only a visual from the gallery can be refined.' });
    }
    if (liveRef.current) {
      return Promise.resolve({ ok: false, reason: 'busy', message: 'Another answer is running. Try again when it has finished.' });
    }
    return new Promise<RefineOutcome>(resolve => {
      void (async () => {
        let record: VisualRecord;
        try {
          // The stored version: the request carries its exact source.
          record = (await visualsApi.get(visual.id)).visual;
        } catch (error) {
          resolve({ ok: false, reason: 'failed', message: toVisualError(error).message });
          return;
        }
        const live = beginRun(level, 'refine', `refining ${level.target.label}`, null, { kind: record.kind, previous: record.source, resolve });
        if (!live) {
          resolve({ ok: false, reason: 'busy', message: 'Another answer is running. Try again when it has finished.' });
          return;
        }
        const target = level.target;
        const values = currentValues(level.seq, target);
        const request = composeRefineRequest({ kind: record.kind, title: record.title, source: record.source, values, instruction: text });
        const instructions = conversationsRef.current.find(c => c.id === level.conversationId)?.systemPrompt?.trim() || null;
        // A session of its own: the request never enters the discussion's agent memory.
        const key = sideSessionKey(level.conversationId, `${level.threadId}-refine`);
        try {
          const sessionId = await api.start(key, instructions, level.conversationId);
          if (live.settled) return;
          live.sessionId = sessionId;
          setRuntimeInstalled(true);
          await api.send(sessionId, request, live.runId, [], answerScope(null, workspaceOf(level.conversationId), false));
        } catch (error) {
          const failure = toAgentError(error);
          enqueue(live, { type: 'local_failed', error: failure.message, code: failure.code, atMs: Date.now() }, true);
        }
      })();
    });
  }, [api, beginRun, enqueue, setRuntimeInstalled]);

  const showVisualVersion = useCallback((record: VisualRecord) => {
    const target = recordTarget(record);
    if (!target) return;
    const parentMessageId = record.messageId;
    const list = locationThreads(record.conversationId, parentMessageId);
    const threadId = findChildThread(list, target, null)?.id ?? newId('thread');
    const seq = nextSeq();
    setSession(s => {
      if (!s) return s;
      const level: OpenFocus = {
        target,
        conversationId: record.conversationId,
        parentMessageId,
        threadId,
        parentThreadId: null,
        parentTurnId: null,
        trigger: s.stack.levels[0].trigger,
        seq,
        visual: visualRef(record),
      };
      return { ...s, stack: initialStack(level) };
    });
  }, [locationThreads]);

  const stop = useCallback(() => {
    const live = liveRef.current;
    if (!live || live.settled) return;
    if (live.sessionId === null) {
      enqueue(live, { type: 'local_interrupted', atMs: Date.now() }, true);
      return;
    }
    if (live.transcript.interrupting) return;
    enqueue(live, { type: 'local_interrupt_requested' }, true);
    live.abortTimer = window.setTimeout(() => {
      live.abortTimer = null;
      enqueue(live, { type: 'local_interrupted', atMs: Date.now() }, true);
    }, INTERRUPT_TIMEOUT_MS);
    api.abort(live.sessionId).catch(error => {
      console.error('Side answer interrupt failed:', toAgentError(error).message);
      enqueue(live, { type: 'local_interrupted', atMs: Date.now() }, true);
    });
  }, [api, enqueue]);

  /** The conversation and message a diagram's answer belongs to, or null when it is not shown any more. */
  const placeOf = useCallback((place: DiagramPlace): { conversationId: string; parentMessageId: string | null } | null => {
    if (place.kind === 'answer') return { conversationId: place.conversationId, parentMessageId: place.messageId };
    const level = sessionRef.current?.stack.levels.find(l => l.threadId === place.parentThreadId);
    return level ? { conversationId: level.conversationId, parentMessageId: level.parentMessageId } : null;
  }, []);

  const isLatestAnswer = useCallback((place: DiagramPlace): boolean => {
    if (place.kind === 'answer') {
      if (activeIdRef.current !== place.conversationId) return false;
      const answers = messagesRef.current.filter(m => m.role === 'assistant');
      return answers.length > 0 && answers[answers.length - 1].id === place.messageId;
    }
    const where = placeOf(place);
    if (!where) return false;
    const turns = findThread(where.conversationId, where.parentMessageId, place.parentThreadId)?.turns ?? [];
    const answers = turns.filter(t => t.role === 'assistant');
    return answers.length > 0 && answers[answers.length - 1].id === place.parentTurnId;
  }, [findThread, placeOf]);

  const repairDiagram = useCallback((request: DiagramRepairRequest): Promise<DiagramRepairOutcome> => {
    const busy: DiagramRepairOutcome = { ok: false, reason: 'busy', message: 'Another answer is running. Try again when it has finished.' };
    const where = placeOf(request.place);
    if (!where) return Promise.resolve({ ok: false, reason: 'failed', message: 'The answer this diagram belongs to is no longer open.' });
    if (liveRef.current) return Promise.resolve(busy);
    const { conversationId, parentMessageId } = where;
    // A session of its own per diagram: the request never enters the
    // conversation's or a discussion's agent memory.
    const threadId = `${request.key}-repair`;
    const key = sideSessionKey(conversationId, threadId);
    const level: OpenFocus = {
      target: { kind: 'mermaid', label: 'Diagram correction', source: request.source },
      conversationId,
      parentMessageId,
      threadId,
      parentThreadId: null,
      parentTurnId: null,
      trigger: null,
      seq: nextSeq(),
      visual: null,
    };
    const closeRepairSession = () => {
      api.closeSession(key).catch(error => console.error('Diagram correction session not closed:', toAgentError(error).message));
    };
    return new Promise<DiagramRepairOutcome>(resolve => {
      let timer: number | null = null;
      const finish = (outcome: DiagramRepairOutcome) => {
        if (timer !== null) window.clearTimeout(timer);
        timer = null;
        resolve(outcome);
        closeRepairSession();
      };
      const live = beginRun(level, 'repair', 'correcting a diagram', null, null, finish);
      if (!live) {
        resolve(busy);
        return;
      }
      timer = window.setTimeout(() => {
        timer = null;
        if (liveRef.current === live && !live.settled) stop();
      }, REPAIR_TIMEOUT_MS);
      void (async () => {
        try {
          const sessionId = await api.start(key, null, conversationId);
          if (live.settled) {
            closeRepairSession();
            return;
          }
          live.sessionId = sessionId;
          setRuntimeInstalled(true);
          await api.send(sessionId, request.message, live.runId, []);
        } catch (error) {
          const failure = toAgentError(error);
          enqueue(live, { type: 'local_failed', error: failure.message, code: failure.code, atMs: Date.now() }, true);
        }
      })();
    });
  }, [api, beginRun, enqueue, placeOf, setRuntimeInstalled, stop]);

  const approve = useCallback((stepId: string, approved: boolean) => {
    const live = liveRef.current;
    if (!live || live.settled || live.sessionId === null) return;
    enqueue(live, { type: 'local_approval', stepId, approved }, true);
    api.approve(live.sessionId, stepId, approved)
      .catch(error => notify.error('The decision did not reach the agent', { description: toAgentError(error).message }));
  }, [api, enqueue]);

  // Answered in the Inbox: show the decision here too.
  useEffect(() => onApprovalAnswered(({ sessionId, stepId, approved }) => {
    const live = liveRef.current;
    if (!live || live.settled || live.sessionId !== sessionId) return;
    enqueue(live, { type: 'local_approval', stepId, approved }, true);
  }), [enqueue]);

  /** The levels from a root thread down to `threadId`, or null when it is not kept. */
  const levelsFor = useCallback((
    conversationId: string,
    parentMessageId: string | null,
    threadId: string,
    trigger: HTMLElement | null,
  ): OpenFocus[] | null => {
    const path = threadPath(locationThreads(conversationId, parentMessageId), threadId);
    if (path.length === 0) return null;
    return path.map(t => ({
      target: t.anchor.target,
      conversationId,
      parentMessageId,
      threadId: t.id,
      parentThreadId: t.parentThreadId ?? null,
      parentTurnId: t.parentTurnId ?? null,
      trigger,
      seq: nextSeq(),
      visual: null,
    }));
  }, [locationThreads]);

  const openFocus = useCallback((request: FocusRequest) => {
    const conversationId = request.conversationId ?? activeIdRef.current;
    if (!conversationId) return;
    const trigger = request.trigger ?? null;
    if (request.threadId) {
      const levels = levelsFor(conversationId, request.parentMessageId, request.threadId, trigger);
      if (levels) {
        setSession({ key: nextSeq(), stack: stackReducer(initialStack(levels[0]), { type: 'reset', levels }) });
        return;
      }
    }
    const list = locationThreads(conversationId, request.parentMessageId);
    const threadId = request.threadId ?? findChildThread(list, request.target, null)?.id ?? newId('thread');
    setSession({
      key: nextSeq(),
      stack: initialStack({
        target: request.target,
        conversationId,
        parentMessageId: request.parentMessageId,
        threadId,
        parentThreadId: null,
        parentTurnId: null,
        trigger,
        seq: nextSeq(),
        visual: request.visual ?? null,
      }),
    });
  }, [levelsFor, locationThreads]);

  const drillDown = useCallback((request: DrillRequest): DrillResult => {
    const current = sessionRef.current;
    if (!current) return 'closed';
    if (!canPush(current.stack)) return 'depth';
    const here = current.stack.levels[current.stack.index];
    const { conversationId, parentMessageId } = here;
    const list = locationThreads(conversationId, parentMessageId);
    // The level it is opened from must exist for the link to hold (a
    // selection in a document can be asked about before any question).
    if (!list.some(t => t.id === request.parentThreadId) && request.parentThreadId === here.threadId) {
      mutateThread({ conversationId, parentMessageId, target: here.target }, here.threadId, t => t, {
        parentThreadId: here.parentThreadId,
        parentTurnId: here.parentTurnId,
      });
    }
    const threadId = findChildThread(list, request.target, request.parentThreadId)?.id ?? newId('thread');
    const level: OpenFocus = {
      target: request.target,
      conversationId,
      parentMessageId,
      threadId,
      parentThreadId: request.parentThreadId,
      parentTurnId: request.parentTurnId,
      trigger: here.trigger,
      seq: nextSeq(),
      visual: null,
    };
    setSession(s => (s ? { ...s, stack: stackReducer(s.stack, { type: 'push', level }) } : s));
    return 'opened';
  }, [locationThreads, mutateThread]);

  const navigate = useCallback((move: FocusNavigation) => {
    setSession(s => (s ? { ...s, stack: stackReducer(s.stack, move) } : s));
  }, []);

  const jumpToThread = useCallback((threadId: string) => {
    const current = sessionRef.current;
    if (!current) return;
    const root = current.stack.levels[0];
    const levels = levelsFor(root.conversationId, root.parentMessageId, threadId, root.trigger);
    if (!levels) return;
    setSession(s => (s ? { ...s, stack: stackReducer(s.stack, { type: 'reset', levels }) } : s));
  }, [levelsFor]);

  const bringBack = useCallback((level: OpenFocus, text: string): boolean => {
    const body = text.trim();
    if (!body || !level.parentThreadId) return false;
    const parent = findThread(level.conversationId, level.parentMessageId, level.parentThreadId);
    if (!parent) return false;
    const turn: ThreadTurn = {
      id: newId('turn'),
      role: 'user',
      content: body,
      timestamp: new Date().toISOString(),
      summaryOf: { threadId: level.threadId, label: level.target.label },
    };
    mutateThread(parent.anchor, parent.id, t => appendTurn(t, turn), {
      parentThreadId: parent.parentThreadId ?? null,
      parentTurnId: parent.parentTurnId ?? null,
    });
    // Up to the parent, where the summary now shows.
    setSession(s => {
      if (!s) return s;
      const at = s.stack.levels.findIndex(l => l.threadId === parent.id);
      return at >= 0 ? { ...s, stack: stackReducer(s.stack, { type: 'jump', index: at }) } : s;
    });
    return true;
  }, [findThread, mutateThread]);

  const closeFocus = useCallback(() => {
    // The pop-out's side sessions are stopped (each is one agent process). The
    // backend keeps any session whose answer is still running and stops it once idle.
    const levels = sessionRef.current?.stack.levels ?? [];
    const keys = new Set(levels.flatMap(l => sideSessionKeys(l.conversationId, l.threadId)));
    for (const key of keys) {
      api.closeSession(key).catch(error => console.error('Side session not closed:', toAgentError(error).message));
    }
    setSession(null);
  }, [api]);

  const value = useMemo<FocusContextValue>(() => ({
    openFocus,
    closeFocus,
    drillDown,
    navigate,
    jumpToThread,
    session,
    localThreads,
    locationThreads,
    findThread,
    ask,
    summarize,
    refine,
    showVisualVersion,
    repairDiagram,
    isLatestAnswer,
    bringBack,
    stop,
    approve,
    sideLive,
  }), [openFocus, closeFocus, drillDown, navigate, jumpToThread, session, localThreads, locationThreads, findThread, ask, summarize, refine, showVisualVersion, repairDiagram, isLatestAnswer, bringBack, stop, approve, sideLive]);

  return (
    <FocusContext.Provider value={value}>
      {children}
      {session && <FocusOverlay key={session.key} session={session} onClose={closeFocus} />}
      <SelectionAsk />
    </FocusContext.Provider>
  );
}

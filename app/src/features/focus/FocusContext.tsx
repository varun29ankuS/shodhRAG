import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { notify } from '../../lib/notify';
import type { AgentEventEnvelope } from '../agent/events';
import { answerText, initialTranscript, isLive, reduceAll, toPersisted } from '../agent/reducer';
import type { TranscriptAction, TranscriptState } from '../agent/reducer';
import { toAgentError, useAgentSession } from '../agent/useAgentSession';
import { useChatSession } from '../ask/ChatSessionContext';
import { composeSideQuestion } from './contextBlock';
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
  sideSessionKey,
  threadHistory,
  threadsFromMetadata,
  upsertThread,
} from './threadStore';
import type { HistoryTurnLike, LocalThreadStore } from './threadStore';
import { ancestorsFor, findChildThread, threadPath } from './threadTree';
import { FocusOverlay } from './FocusOverlay';
import { SelectionAsk } from './SelectionAsk';

/** How long an interrupt may take before the side answer is closed locally. */
const INTERRUPT_TIMEOUT_MS = 5_000;

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

type SidePurpose = 'answer' | 'summary';

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
  /** Threads with no parent message, kept on this device for a conversation. */
  localThreads: (conversationId: string) => FocusThread[];
  /** Every thread kept with a message (or on this device when `parentMessageId` is null). */
  locationThreads: (conversationId: string, parentMessageId: string | null) => FocusThread[];
  /** A thread by id, wherever it is kept. */
  findThread: (conversationId: string, parentMessageId: string | null, threadId: string) => FocusThread | null;
  /** Ask a side question. False when it could not start (another answer is running). */
  ask: (open: OpenFocus, question: string, extras: FocusExtras) => Promise<boolean>;
  /** Ask the agent for a summary of a level's discussion (not added to the thread). */
  summarize: (open: OpenFocus, destination: SummaryDestination) => Promise<SummaryResult>;
  /** Post a summary of a nested level into its parent discussion and go up to it. */
  bringBack: (open: OpenFocus, text: string) => boolean;
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
  const { conversations, activeConversationId, messages, updateThreads, claimSideRun, releaseSideRun, setRuntimeInstalled } = chat;

  const [session, setSession] = useState<FocusSession | null>(null);
  const [local, setLocal] = useState<Record<string, FocusThread[]>>({});
  const [sideLive, setSideLive] = useState<SideLiveView | null>(null);
  const seqRef = useRef(0);
  const sessionRef = useRef(session);
  sessionRef.current = session;
  const liveRef = useRef<SideLive | null>(null);
  const storeRef = useRef<LocalThreadStore | null>(null);
  if (!storeRef.current) storeRef.current = createLocalThreadStore(browserStorage());
  const localRef = useRef(local);
  localRef.current = local;

  const conversationsRef = useRef(conversations);
  conversationsRef.current = conversations;
  const messagesRef = useRef(messages);
  messagesRef.current = messages;
  const activeIdRef = useRef(activeConversationId);
  activeIdRef.current = activeConversationId;

  const nextSeq = () => {
    seqRef.current += 1;
    return seqRef.current;
  };

  /** Always reads the latest local threads (used while mutating). */
  const readLocal = useCallback((conversationId: string): FocusThread[] => {
    const cached = localRef.current[conversationId];
    if (cached) return cached;
    return storeRef.current?.list(conversationId) ?? [];
  }, []);
  // Public reader whose identity changes with the local threads, so
  // consumers (e.g. a task's reply count) re-render when they change.
  const localThreads = useCallback(
    (conversationId: string): FocusThread[] => local[conversationId] ?? readLocal(conversationId),
    [local, readLocal],
  );

  // Once per launch, after conversations load: drop device-kept threads of
  // conversations deleted since (not at delete time, so Undo keeps them).
  const prunedRef = useRef(false);
  useEffect(() => {
    if (prunedRef.current || conversations.length === 0) return;
    prunedRef.current = true;
    storeRef.current?.prune(new Set(conversations.map(c => c.id)));
  }, [conversations]);

  // Load a conversation's local threads into state once it is looked at.
  useEffect(() => {
    if (!activeConversationId || local[activeConversationId]) return;
    const loaded = storeRef.current?.list(activeConversationId) ?? [];
    setLocal(prev => (prev[activeConversationId] ? prev : { ...prev, [activeConversationId]: loaded }));
  }, [activeConversationId, local]);

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
    // `messages` and `conversations` re-create the reader when message threads change.
    [messageThreads, localThreads, messages, conversations],
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
    const conversationId = anchor.conversationId;
    const list = readLocal(conversationId);
    const current = list.find(t => t.id === threadId) ?? fresh();
    const next = upsertThread(list, change(current));
    if (!storeRef.current?.save(conversationId, next)) {
      notify.error('This side discussion could not be saved on this device', {
        description: 'Storage is full or unavailable. It stays visible until the app closes.',
      });
    }
    // Updated now so a second change in the same tick builds on this one.
    localRef.current = { ...localRef.current, [conversationId]: next };
    setLocal(prev => ({ ...prev, [conversationId]: next }));
  }, [updateThreads, readLocal]);

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
    if (live.purpose === 'summary') {
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
    };
    liveRef.current = live;
    setSideLive({ threadId, purpose, transcript: live.transcript });
    return live;
  }, [claimSideRun]);

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
    const instructions = conversationsRef.current.find(c => c.id === conversationId)?.systemPrompt?.trim() || null;
    try {
      const sessionId = await api.start(sideSessionKey(conversationId, threadId), instructions);
      if (live.settled) return true;
      live.sessionId = sessionId;
      setRuntimeInstalled(true);
      const message = composeSideQuestion(level.target, text, extras, { ancestors, notes, followups: true });
      await api.send(sessionId, message, live.runId, history);
    } catch (error) {
      const failure = toAgentError(error);
      enqueue(live, { type: 'local_failed', error: failure.message, code: failure.code, atMs: Date.now() }, true);
    }
    return true;
  }, [api, beginRun, enqueue, locationThreads, mainTurns, mutateThread, setRuntimeInstalled]);

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
          const sessionId = await api.start(key, instructions);
          if (live.settled) return;
          live.sessionId = sessionId;
          setRuntimeInstalled(true);
          await api.send(sessionId, composeSummaryRequest(thread, destination), live.runId, []);
        } catch (error) {
          const failure = toAgentError(error);
          enqueue(live, { type: 'local_failed', error: failure.message, code: failure.code, atMs: Date.now() }, true);
        }
      })();
    });
  }, [api, beginRun, enqueue, findThread, setRuntimeInstalled]);

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

  const approve = useCallback((stepId: string, approved: boolean) => {
    const live = liveRef.current;
    if (!live || live.settled || live.sessionId === null) return;
    enqueue(live, { type: 'local_approval', stepId, approved }, true);
    api.approve(live.sessionId, stepId, approved)
      .catch(error => notify.error('The decision did not reach the agent', { description: toAgentError(error).message }));
  }, [api, enqueue]);

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

  const closeFocus = useCallback(() => setSession(null), []);

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
    bringBack,
    stop,
    approve,
    sideLive,
  }), [openFocus, closeFocus, drillDown, navigate, jumpToThread, session, localThreads, locationThreads, findThread, ask, summarize, bringBack, stop, approve, sideLive]);

  return (
    <FocusContext.Provider value={value}>
      {children}
      {session && <FocusOverlay key={session.key} session={session} onClose={closeFocus} />}
      <SelectionAsk />
    </FocusContext.Provider>
  );
}

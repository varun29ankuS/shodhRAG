import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { notify } from '../../lib/notify';
import type { AgentEventEnvelope } from '../agent/events';
import { answerText, initialTranscript, isLive, reduceAll, toPersisted } from '../agent/reducer';
import type { TranscriptAction, TranscriptState } from '../agent/reducer';
import { toAgentError, useAgentSession } from '../agent/useAgentSession';
import { useChatSession } from '../ask/ChatSessionContext';
import { composeSideQuestion } from './contextBlock';
import type { FocusExtras, FocusTarget, FocusThread, ThreadAnchor, ThreadTurn } from './focusTypes';
import {
  appendTurn,
  createLocalThreadStore,
  sameTarget,
  sideSessionKey,
  threadHistory,
  threadsFromMetadata,
  upsertThread,
} from './threadStore';
import type { HistoryTurnLike, LocalThreadStore } from './threadStore';
import { FocusOverlay } from './FocusOverlay';

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
  /** Reopen this thread (from a "replies" chip). */
  threadId?: string | null;
  /** Element focus returns to when the pop-out closes. */
  trigger?: HTMLElement | null;
}

/** The pop-out as opened: the request resolved to a conversation and thread. */
export interface OpenFocus {
  target: FocusTarget;
  conversationId: string;
  parentMessageId: string | null;
  threadId: string;
  trigger: HTMLElement | null;
  /** Increases on every open, so reopening the same object remounts state. */
  seq: number;
}

/** The side answer being produced. */
interface SideLive {
  runId: string;
  threadId: string;
  anchor: ThreadAnchor;
  sessionId: string | null;
  transcript: TranscriptState;
  queue: TranscriptAction[];
  frame: number | null;
  settled: boolean;
  abortTimer: number | null;
}

export interface SideLiveView {
  threadId: string;
  transcript: TranscriptState;
}

export interface FocusContextValue {
  openFocus: (request: FocusRequest) => void;
  closeFocus: () => void;
  /** Threads with no parent message, kept on this device for a conversation. */
  localThreads: (conversationId: string) => FocusThread[];
  /** A thread by id, wherever it is kept. */
  findThread: (conversationId: string, parentMessageId: string | null, threadId: string) => FocusThread | null;
  /** Ask a side question. False when it could not start (another answer is running). */
  ask: (open: OpenFocus, question: string, extras: FocusExtras) => Promise<boolean>;
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
 * the Ask view; visuals show their focus affordance only inside it, so the
 * pop-out's own thread (and dense surfaces) never open nested pop-outs.
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
 */
export function FocusProvider({ children }: { children: React.ReactNode }) {
  const session = useChatSession();
  const { conversations, activeConversationId, messages, updateThreads, claimSideRun, releaseSideRun, setRuntimeInstalled } = session;

  const [open, setOpen] = useState<OpenFocus | null>(null);
  const [local, setLocal] = useState<Record<string, FocusThread[]>>({});
  const [sideLive, setSideLive] = useState<SideLiveView | null>(null);
  const seqRef = useRef(0);
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

  const findThread = useCallback((conversationId: string, parentMessageId: string | null, threadId: string): FocusThread | null => {
    const list = parentMessageId ? messageThreads(conversationId, parentMessageId) : localThreads(conversationId);
    return list.find(t => t.id === threadId) ?? null;
  }, [messageThreads, localThreads]);

  /** Change one thread wherever it is kept, creating it on first use. */
  const mutateThread = useCallback((anchor: ThreadAnchor, threadId: string, change: (thread: FocusThread) => FocusThread) => {
    const now = new Date().toISOString();
    const fresh = (): FocusThread => ({ id: threadId, anchor, turns: [], createdAt: now, updatedAt: now });
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
    const answer: ThreadTurn = {
      id: newId('turn'),
      role: 'assistant',
      content: answerText(transcript),
      timestamp: new Date().toISOString(),
      transcript: toPersisted(transcript) as unknown as Record<string, unknown>,
    };
    mutateThread(live.anchor, live.threadId, thread => appendTurn(thread, answer));
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
      setSideLive({ threadId: live.threadId, transcript: next });
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

  const ask = useCallback(async (target: OpenFocus, question: string, extras: FocusExtras): Promise<boolean> => {
    const text = question.trim();
    if (!text || liveRef.current) return false;
    const { conversationId, parentMessageId, threadId } = target;
    if (!claimSideRun({ conversationId, threadId, label: target.target.label })) return false;

    const anchor: ThreadAnchor = { conversationId, parentMessageId, target: target.target };
    const prior = findThread(conversationId, parentMessageId, threadId)?.turns ?? [];
    const selection = extras.selection?.trim() || undefined;
    const userTurn: ThreadTurn = {
      id: newId('turn'),
      role: 'user',
      content: text,
      timestamp: new Date().toISOString(),
      ...(selection ? { selection } : {}),
      ...(typeof extras.page === 'number' ? { page: extras.page } : {}),
    };
    mutateThread(anchor, threadId, thread => appendTurn({ ...thread, anchor }, userTurn));

    const runId = newId('run');
    const live: SideLive = {
      runId,
      threadId,
      anchor,
      sessionId: null,
      transcript: initialTranscript(runId, Date.now()),
      queue: [],
      frame: null,
      settled: false,
      abortTimer: null,
    };
    liveRef.current = live;
    setSideLive({ threadId, transcript: live.transcript });

    const history = threadHistory(
      mainTurns(conversationId, parentMessageId),
      prior.map(t => ({ role: t.role, content: t.content })),
    ).map(t => ({ role: t.role, content: t.content }));
    const instructions = conversationsRef.current.find(c => c.id === conversationId)?.systemPrompt?.trim() || null;
    try {
      const sessionId = await api.start(sideSessionKey(conversationId, threadId), instructions);
      if (live.settled) return true;
      live.sessionId = sessionId;
      setRuntimeInstalled(true);
      await api.send(sessionId, composeSideQuestion(target.target, text, extras), runId, history);
    } catch (error) {
      const failure = toAgentError(error);
      enqueue(live, { type: 'local_failed', error: failure.message, code: failure.code, atMs: Date.now() }, true);
    }
    return true;
  }, [api, claimSideRun, enqueue, findThread, mainTurns, mutateThread, setRuntimeInstalled]);

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

  const openFocus = useCallback((request: FocusRequest) => {
    const conversationId = request.conversationId ?? activeIdRef.current;
    if (!conversationId) return;
    let threadId = request.threadId ?? null;
    if (!threadId) {
      const list = request.parentMessageId ? messageThreads(conversationId, request.parentMessageId) : localThreads(conversationId);
      threadId = list.find(t => sameTarget(t.anchor.target, request.target))?.id ?? newId('thread');
    }
    seqRef.current += 1;
    setOpen({
      target: request.target,
      conversationId,
      parentMessageId: request.parentMessageId,
      threadId,
      trigger: request.trigger ?? null,
      seq: seqRef.current,
    });
  }, [messageThreads, localThreads]);

  const closeFocus = useCallback(() => setOpen(null), []);

  const value = useMemo<FocusContextValue>(() => ({
    openFocus,
    closeFocus,
    localThreads,
    findThread,
    ask,
    stop,
    approve,
    sideLive,
  }), [openFocus, closeFocus, localThreads, findThread, ask, stop, approve, sideLive]);

  // Resolved on every render (local or message threads changed), so new turns show at once.
  const openThread = open ? findThread(open.conversationId, open.parentMessageId, open.threadId) : null;

  return (
    <FocusContext.Provider value={value}>
      {children}
      {open && <FocusOverlay key={open.seq} open={open} thread={openThread} onClose={closeFocus} />}
    </FocusContext.Provider>
  );
}

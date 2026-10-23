import React, { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useConversations } from '../../hooks/useConversations';
import { ConversationsUnavailable } from './ConversationsUnavailable';
import type { ConversationMessage } from '../../hooks/useConversations';
import { notify } from '../../lib/notify';
import { normalizeViewTab } from '../../lib/viewTabs';
import type { ViewTab } from '../../lib/viewTabs';
import type { AgentEventEnvelope, NavigationTarget } from '../agent/events';
import { navigationFromEvent, publishTarget } from '../agent/navigation';
import { recordAnswerVisuals } from '../visuals/recording';
import {
  answerText,
  fromPersisted,
  initialTranscript,
  isLive,
  reduceAll,
  toCheckpoint,
  toPersisted,
} from '../agent/reducer';
import type { TranscriptAction, TranscriptState } from '../agent/reducer';
import { toAgentError, useAgentSession } from '../agent/useAgentSession';
import type { AnswerScope, HistoryTurn } from '../agent/useAgentSession';
import { answerScope } from '../workspaces/model';
import type { FocusThread } from '../focus/focusTypes';
import { metadataWithThreads, metadataWithoutThreads, threadsFromMetadata } from '../focus/threadStore';
import { metadataWithSummary, readSideSummary, summaryPrompt } from '../focus/summary';
import { replayedContent } from './chatHistory';
import { metadataWithAttachedContext, readAttachedContext } from './imageAttachments';
import type { AttachedContext } from './imageAttachments';
import type { SideSummaryRef } from '../focus/summary';
import type { ModelOverride } from '../modelPicker/modelTypes';
import type {
  ChatMessage,
  RawSearchResult,
  ResponseMetadata,
  RunRecord,
  RunStatus,
  RunStep,
  SendOptions,
} from './types';
import { onApprovalAnswered } from '../inbox/approvalBus';
import {
  appliesAtOnce,
  changeMessages,
  emptyView,
  loadConversation,
  publishMessage,
  saveStep,
  startsCheck,
  upsertById,
} from './chatPersistence';
import type { PersistMode, PersistView } from './chatPersistence';

/**
 * The answer being produced. Events are queued and folded into the
 * transcript once per animation frame, so streaming never re-renders (and
 * re-parses markdown) per token.
 */
interface LiveRun {
  runId: string;
  conversationId: string;
  sessionId: string | null;
  message: ChatMessage;
  queue: TranscriptAction[];
  frame: number | null;
  settled: boolean;
  abortTimer: number | null;
}

/** The visible chat and what of it is still to be saved (see ./chatPersistence). */
type ViewState = PersistView<ChatMessage, ConversationMessage>;

/** Earlier turns replayed into a fresh agent session. */
const HISTORY_LIMIT = 10;

/** How long an interrupt may take before the answer is closed locally. */
const INTERRUPT_TIMEOUT_MS = 5_000;

type ConversationsApi = ReturnType<typeof useConversations>;

/** One answer run with a fallback model: the override sent with `agent_start` and the model it replaces. */
export interface FallbackRun {
  override: ModelOverride;
  /** The failed model as runs name it (`provider/model`). */
  from: string;
}

export interface SendExtra {
  /** A summary brought back from a side discussion. */
  sideSummary?: SideSummaryRef;
  /** Run this answer with a fallback model. */
  fallback?: FallbackRun | null;
  /**
   * Text the user attached (read from images): sent beside the typed words, which stay
   * what is learned from and checked (`agent_send`'s `attachedContext`).
   */
  attachedContext?: AttachedContext[];
}

/** What the rest of the app needs to know about the running answer. */
export interface LiveRunInfo {
  conversationId: string;
  messageId: string;
  transcript: TranscriptState;
}

/** A `navigated` event: the agent opened another view. */
export interface AgentNavigation {
  view: ViewTab;
  focus: string | null;
  /** What to show inside the view, if the event said. */
  target: NavigationTarget | null;
  /** Increases with every navigation, so repeats are distinguishable. */
  seq: number;
}

export interface ChatSessionValue {
  conversations: ConversationsApi['conversations'];
  /** The saved conversations were read: `conversations` is no longer loading. */
  conversationsLoaded: ConversationsApi['loaded'];
  activeConversationId: ConversationsApi['activeConversationId'];
  activeConversation: ConversationsApi['activeConversation'];
  createConversation: ConversationsApi['createConversation'];
  switchConversation: ConversationsApi['switchConversation'];
  renameConversation: ConversationsApi['renameConversation'];
  deleteConversation: ConversationsApi['deleteConversation'];
  pinConversation: ConversationsApi['pinConversation'];
  updateConversationMeta: ConversationsApi['updateConversationMeta'];
  /** Move the chats of a deleted workspace to "No workspace" (in memory). */
  detachWorkspace: ConversationsApi['detachWorkspace'];

  /** Messages of the active conversation, including a live answer. */
  messages: ChatMessage[];
  /** True while an answer for the active conversation is running. */
  isStreaming: boolean;
  /** Conversation an answer is running for, if any (one at a time). */
  streamingConversationId: string | null;
  /** The running answer, for the activity tray and the dock. */
  liveRun: LiveRunInfo | null;
  /** Latest navigation requested by the agent. */
  navigation: AgentNavigation | null;
  /** Whether the agent runtime is installed (null until known). */
  runtimeInstalled: boolean | null;
  setRuntimeInstalled: (installed: boolean) => void;
  /**
   * Start the active conversation's agent session ahead of the question
   * (the user started typing one). Idempotent; never needed for correctness.
   */
  prewarm: () => void;

  /**
   * Post a message and run the agent on it. `extra.sideSummary` marks it as
   * a summary brought back from a side discussion. Returns whether the message
   * was accepted (false: empty, no conversation, or an answer is running);
   * decided at once, before the agent runs.
   */
  send: (text: string, options: SendOptions, extra?: SendExtra) => boolean;
  /**
   * Re-run the user prompt that produced `assistantMessageId`; with
   * `fallback`, that one answer runs with the fallback model.
   */
  retry: (assistantMessageId: string, options: SendOptions, fallback?: FallbackRun | null) => void;
  /** Redirect the running answer. */
  steer: (text: string) => void;
  /** Interrupt the running answer. */
  cancel: () => void;
  /** Answer a pending approval of the running answer (`forConversation`: allow what it offered for the session). */
  approve: (stepId: string, approved: boolean, forConversation?: boolean, reviewToken?: string) => void;
  /** Append a non-chat message (e.g. OCR or upload notices) to the active conversation. */
  appendMessage: (message: ChatMessage) => void;
  /**
   * Append a non-chat message to `conversationId`, visible or not (a notice of work
   * that started there before the person switched away).
   */
  appendMessageTo: (conversationId: string, message: ChatMessage) => void;
  updateMessage: (id: string, patch: Partial<ChatMessage>) => void;

  /** A side-thread question being answered; blocks main sends until it ends. */
  sideRun: SideRunInfo | null;
  /**
   * Reserve the single agent run for a side thread. False when an answer
   * (main or side) is already running.
   */
  claimSideRun: (info: SideRunInfo) => boolean;
  /** Give the run back; only the holder's `threadId` releases it. */
  releaseSideRun: (threadId: string) => void;
  /**
   * Change the side threads stored on a message, in the visible
   * conversation or in the background one it belongs to.
   */
  updateThreads: (conversationId: string, messageId: string, update: (threads: FocusThread[]) => FocusThread[]) => void;
  /** Change a conversation's side threads that have no parent message (stored on the conversation). */
  updateFocusThreads: ConversationsApi['updateFocusThreads'];
}

/** The side-thread answer that holds the run. */
export interface SideRunInfo {
  conversationId: string;
  threadId: string;
  /** What the question is about, e.g. "Revenue by quarter". */
  label: string;
}

const ChatSessionContext = createContext<ChatSessionValue | null>(null);

export function useChatSession(): ChatSessionValue {
  const value = useContext(ChatSessionContext);
  if (!value) {
    throw new Error('useChatSession must be used inside <ChatSessionProvider>');
  }
  return value;
}

function newId(prefix: string): string {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isRunStatus(value: unknown): value is RunStatus {
  return value === 'running' || value === 'done' || value === 'failed' || value === 'cancelled';
}

/** Legacy run record of answers produced before the agent harness. */
function readRun(value: unknown): RunRecord | undefined {
  if (!isRecord(value) || !isRunStatus(value.status) || typeof value.startedAt !== 'string') return undefined;
  const steps = Array.isArray(value.steps) ? (value.steps.filter(isRecord) as unknown as RunStep[]) : [];
  // A run persisted while "running" can only come from an interrupted session.
  const status: RunStatus = value.status === 'running' ? 'cancelled' : value.status;
  return {
    status,
    startedAt: value.startedAt,
    elapsedMs: typeof value.elapsedMs === 'number' ? value.elapsedMs : undefined,
    steps: steps.map(s => (s.status === 'running' ? { ...s, status: 'stopped' as const } : s)),
    error: typeof value.error === 'string' ? value.error : undefined,
  };
}

function fromStored(m: ConversationMessage): ChatMessage {
  const metadata = metadataWithAttachedContext(metadataWithSummary(metadataWithoutThreads(m.metadata), null), []);
  const threads = threadsFromMetadata(m.metadata);
  const sideSummary = m.role === 'user' ? readSideSummary(m.metadata) : null;
  const attachedContext = m.role === 'user' ? readAttachedContext(m.metadata) : [];
  return {
    id: m.id,
    role: m.role,
    content: m.content,
    timestamp: m.timestamp,
    artifacts: m.artifacts,
    searchResults: m.searchResults as RawSearchResult[] | undefined,
    metadata: metadata ? (metadata as ResponseMetadata) : undefined,
    run: readRun(m.run),
    transcript: fromPersisted(m.transcript) ?? undefined,
    threads: threads.length > 0 ? threads : undefined,
    sideSummary: sideSummary ?? undefined,
    attachedContext: attachedContext.length > 0 ? attachedContext : undefined,
  };
}

function toStored(m: ChatMessage): ConversationMessage {
  const stored: ConversationMessage = {
    id: m.id,
    role: m.role,
    content: m.content,
    timestamp: m.timestamp,
  };
  if (m.artifacts && m.artifacts.length > 0) stored.artifacts = m.artifacts;
  if (m.searchResults && m.searchResults.length > 0) stored.searchResults = m.searchResults;
  const metadata = metadataWithAttachedContext(
    metadataWithThreads(metadataWithSummary(m.metadata, m.sideSummary ?? null), m.threads ?? []),
    m.attachedContext ?? [],
  );
  if (metadata) stored.metadata = metadata;
  if (m.run) {
    const { activity: _activity, ...persistable } = m.run;
    stored.run = persistable as unknown as Record<string, unknown>;
  }
  if (m.transcript) stored.transcript = toPersisted(m.transcript) as unknown as Record<string, unknown>;
  return stored;
}

function isPersistable(m: ChatMessage): boolean {
  if (m.run?.status === 'running') return false;
  return !(m.transcript && isLive(m.transcript));
}

/**
 * Stored form of a running answer whose text is complete (its check against
 * the sources has started): kept if the app closes before the run ends.
 */
function toStoredCheckpoint(m: ChatMessage): ConversationMessage {
  return toStored(m.transcript ? { ...m, transcript: toCheckpoint(m.transcript) } : m);
}

/**
 * The answer's search limit from the send options and the conversation's workspace (whose
 * sources the backend limits search to, unless "search all my library" is on); null when
 * neither applies.
 */
function scopeOf(options: SendOptions | null, workspaceId: string | null): AnswerScope | null {
  const sourceIds = options?.sourceIds?.filter(id => id.trim().length > 0) ?? [];
  const sourceFiles = options?.sourceFiles?.filter(f => f.trim().length > 0) ?? [];
  const limit = sourceIds.length === 0 && sourceFiles.length === 0 ? null : { sourceIds, sourceFiles };
  return answerScope(limit, workspaceId, options?.searchAll === true);
}

function historyOf(messages: readonly ChatMessage[]): HistoryTurn[] {
  return messages
    .filter((m): m is ChatMessage & { role: 'user' | 'assistant' } => m.role === 'user' || m.role === 'assistant')
    // A failed answer's partial text is not an answer the model gave.
    .filter(m => m.content.trim().length > 0 && isPersistable(m) && m.transcript?.status !== 'error')
    .slice(-HISTORY_LIMIT)
    .map(m => ({ role: m.role, content: replayedContent(m) }));
}

export function ChatSessionProvider({ children }: { children: React.ReactNode }) {
  const conv = useConversations();
  const {
    activeConversationId,
    activeConversation,
    updateConversationMessages,
  } = conv;

  const [view, setView] = useState<ViewState>(() => emptyView<ChatMessage, ConversationMessage>());
  const [liveRun, setLiveRun] = useState<LiveRunInfo | null>(null);
  const [navigation, setNavigation] = useState<AgentNavigation | null>(null);
  const [runtimeInstalled, setRuntimeInstalled] = useState<boolean | null>(null);
  const [sideRun, setSideRun] = useState<SideRunInfo | null>(null);

  const liveRef = useRef<LiveRun | null>(null);
  // Read synchronously by send/retry/claim so two runs can never start in one tick.
  const sideRunRef = useRef<SideRunInfo | null>(null);
  const viewRef = useRef(view);
  viewRef.current = view;
  const activeConversationRef = useRef(activeConversation);
  activeConversationRef.current = activeConversation;
  // The latest `persist.seq` written; views at or below it need no save.
  const lastSavedSeqRef = useRef(0);
  const loadedConvIdRef = useRef<string | null>(null);
  const navSeqRef = useRef(0);
  const switchConversationRef = useRef(conv.switchConversation);
  switchConversationRef.current = conv.switchConversation;

  /**
   * Save a view's unsaved changes to its conversation. The data and the
   * "needs saving" mark come from the same view, so a view from an older
   * render writes only its own changes and leaves newer ones due.
   */
  const saveView = useCallback((v: ViewState) => {
    const step = saveStep(v, lastSavedSeqRef.current, isPersistable, toStored);
    if (!step) return;
    lastSavedSeqRef.current = step.seq;
    if (step.conversationId === null) return;
    const stored = step.messages;
    updateConversationMessages(step.conversationId, () => stored, { immediate: step.immediate });
  }, [updateConversationMessages]);

  // Load messages when the active conversation changes. A running answer for
  // that conversation is re-attached so it stays visible.
  // Layout effect: avoids painting one frame of the empty state on switch.
  useLayoutEffect(() => {
    if (!activeConversationId || activeConversationId === loadedConvIdRef.current) return;
    if (!activeConversation || activeConversation.id !== activeConversationId) return;
    // Changes not yet saved belong to the conversation being left.
    saveView(viewRef.current);
    loadedConvIdRef.current = activeConversationId;
    const loaded = activeConversation.messages.map(fromStored);
    const checkpoints: Record<string, ConversationMessage> = {};
    const live = liveRef.current;
    if (live && !live.settled && live.conversationId === activeConversationId) {
      // A checkpoint stored for it is the same message: show it live, and
      // keep saving the checkpoint until the run ends.
      const stored = activeConversation.messages.find(m => m.id === live.message.id);
      if (stored) checkpoints[live.message.id] = stored;
      const at = loaded.findIndex(m => m.id === live.message.id);
      if (at >= 0) loaded[at] = live.message;
      else loaded.push(live.message);
    }
    setView(v => loadConversation(v, activeConversationId, loaded, checkpoints));
  }, [activeConversationId, activeConversation, saveView]);

  // Persist after real mutations only (never on load), so switching
  // conversations does not bump `updatedAt` and reorder the sidebar.
  useEffect(() => {
    saveView(view);
  }, [view, saveView]);

  /**
   * Replace a message in the view if it belongs to the visible conversation.
   * `checkpoint`: the stored form to save while the message is still running.
   */
  const publish = useCallback((
    conversationId: string,
    message: ChatMessage,
    mode: PersistMode,
    checkpoint: ConversationMessage | null = null,
  ) => {
    if (viewRef.current.conversationId !== conversationId) return false;
    setView(v => publishMessage(v, conversationId, message, mode, checkpoint));
    return true;
  }, []);

  /** Write a finished message to its conversation, visible or not, without delay. */
  const commit = useCallback((conversationId: string, message: ChatMessage) => {
    if (publish(conversationId, message, 'immediate')) return;
    const stored = toStored(message);
    updateConversationMessages(conversationId, prev => upsertById(prev, stored), { immediate: true });
  }, [publish, updateConversationMessages]);

  /**
   * The answer's text is complete and its check has started: show it and
   * save it now, so it survives the app closing during the check.
   */
  const checkpoint = useCallback((conversationId: string, message: ChatMessage) => {
    const stored = toStoredCheckpoint(message);
    if (publish(conversationId, message, 'immediate', stored)) return;
    updateConversationMessages(conversationId, prev => upsertById(prev, stored), { immediate: true });
  }, [publish, updateConversationMessages]);

  /** Fold queued events into the live transcript; settle when the run ends. */
  const flush = useCallback((live: LiveRun) => {
    if (live.frame !== null) {
      cancelAnimationFrame(live.frame);
      live.frame = null;
    }
    if (live.settled || live.queue.length === 0) return;
    const actions = live.queue;
    live.queue = [];
    const prior = live.message.transcript ?? initialTranscript(live.runId, Date.now());
    const transcript = reduceAll(prior, actions);
    if (transcript === prior) return;
    const message: ChatMessage = { ...live.message, content: answerText(transcript), transcript };
    live.message = message;
    if (isLive(transcript)) {
      // Each check round (the first answer, then any revision) starts once
      // the text it checks is complete.
      if (startsCheck(actions, message.content)) {
        checkpoint(live.conversationId, message);
      } else {
        publish(live.conversationId, message, 'none');
      }
      setLiveRun({ conversationId: live.conversationId, messageId: message.id, transcript });
      return;
    }
    // Diagrams, charts, equations and tables in agent answers render inline
    // (MessageContentRenderer) and open in the focus pop-out; they are not
    // extracted into side-panel artifacts.
    live.settled = true;
    if (live.abortTimer !== null) {
      window.clearTimeout(live.abortTimer);
      live.abortTimer = null;
    }
    if (liveRef.current === live) liveRef.current = null;
    setLiveRun(null);
    commit(live.conversationId, message);
    // Its diagrams, charts, sketches, plots, simulations, equations and tables join the gallery.
    if (transcript.status === 'completed') {
      recordAnswerVisuals({ conversationId: live.conversationId, messageId: message.id, threadId: null, turnId: null }, message.content);
    }
  }, [checkpoint, commit, publish]);

  const enqueue = useCallback((live: LiveRun, action: TranscriptAction, immediate = false) => {
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
    if (event.type === 'navigated') {
      const nav = navigationFromEvent(event);
      const tab = normalizeViewTab(nav.view);
      if (tab) {
        navSeqRef.current += 1;
        setNavigation({ view: tab, focus: nav.focus, target: nav.target, seq: navSeqRef.current });
        if (nav.target?.kind === 'conversation') {
          // The run keeps streaming into its own conversation.
          switchConversationRef.current(nav.target.conversationId);
        } else if (nav.target) {
          publishTarget(nav.target);
        }
        window.dispatchEvent(new CustomEvent('switchTab', { detail: tab }));
      }
    }
    // Lifecycle changes and the start of the answer's check are applied at
    // once; streamed content waits for the frame.
    enqueue(live, event, appliesAtOnce(event.type));
  }, [enqueue]);

  const api = useAgentSession(onEnvelope);

  // Release the frame callback on unmount.
  useEffect(() => () => {
    const live = liveRef.current;
    if (live && live.frame !== null) cancelAnimationFrame(live.frame);
    if (live && live.abortTimer !== null) window.clearTimeout(live.abortTimer);
  }, []);

  // Learn whether the runtime is installed.
  useEffect(() => {
    let cancelled = false;
    api.runtimeStatus()
      .then(status => { if (!cancelled) setRuntimeInstalled(status.installed); })
      .catch(error => console.error('Agent runtime status:', toAgentError(error).message));
    return () => { cancelled = true; };
  }, [api]);

  // The agent runtime is not started with the app or when a conversation
  // opens: it holds hundreds of megabytes. `prewarm` starts the active
  // conversation's session once the user begins typing a question, so it is
  // ready (or nearly) when they send; `send` starts it anyway.
  const instructions = activeConversation?.systemPrompt?.trim() || null;
  const activeMode = activeConversation?.mode ?? 'research';
  const activeWorkspaceId = activeConversation?.workspaceId ?? null;
  const streaming = liveRun !== null;
  const prewarm = useCallback(() => {
    if (!activeConversationId || runtimeInstalled !== true || streaming) return;
    api.start(activeConversationId, instructions, null, null, { mode: activeMode, workspaceId: activeWorkspaceId }).catch(error => {
      const failure = toAgentError(error);
      if (failure.code === 'runtime_missing') setRuntimeInstalled(false);
      // Other failures (no model configured, no code folder, …) surface when the user asks.
    });
  }, [api, activeConversationId, instructions, activeMode, activeWorkspaceId, runtimeInstalled, streaming]);

  // `textOrigin`: `typed` when `prompt` is exactly what the user typed (learning may use it);
  // `composed` for prompts the app builds (a side-thread summary request).
  const runAgent = useCallback(async (
    conversationId: string,
    prompt: string,
    history: ChatMessage[],
    options: SendOptions | null,
    textOrigin: 'typed' | 'composed',
    fallback: FallbackRun | null = null,
    attachedContext: AttachedContext[] = [],
    priorRunIds: string[] = [],
  ) => {
    const runId = newId('run');
    const startedAtMs = Date.now();
    const transcript: TranscriptState = {
      ...initialTranscript(
        runId,
        startedAtMs,
        fallback ? { from: fallback.from, kind: fallback.override.failure ?? 'other', automatic: fallback.override.automatic } : null,
      ),
      ...(priorRunIds.length > 0 ? { priorRunIds } : {}),
    };
    const message: ChatMessage = {
      id: newId('msg'),
      role: 'assistant',
      content: '',
      timestamp: new Date(startedAtMs).toISOString(),
      transcript,
    };
    const live: LiveRun = {
      runId,
      conversationId,
      sessionId: null,
      message,
      queue: [],
      frame: null,
      settled: false,
      abortTimer: null,
    };
    liveRef.current = live;
    publish(conversationId, message, 'none');
    setLiveRun({ conversationId, messageId: message.id, transcript });

    const conversation = activeConversationRef.current?.id === conversationId ? activeConversationRef.current : null;
    const conversationInstructions = conversation?.systemPrompt?.trim() || null;
    try {
      const mode = { mode: conversation?.mode ?? 'research', workspaceId: conversation?.workspaceId ?? null } as const;
      const sessionId = await api.start(conversationId, conversationInstructions, null, fallback?.override ?? null, mode);
      if (live.settled) return;
      live.sessionId = sessionId;
      setRuntimeInstalled(true);
      const workspaceId = conversation?.workspaceId ?? null;
      await api.send(sessionId, prompt, runId, historyOf(history), {
        scope: scopeOf(options, workspaceId),
        textOrigin,
        attachedContext,
        priorRunIds,
      });
    } catch (error) {
      const failure = toAgentError(error);
      if (failure.code === 'runtime_missing') setRuntimeInstalled(false);
      enqueue(live, { type: 'local_failed', error: failure.message, code: failure.code, atMs: Date.now() }, true);
    }
  }, [api, enqueue, publish]);

  const send = useCallback((text: string, options: SendOptions, extra?: SendExtra): boolean => {
    const content = text.trim();
    const conversationId = viewRef.current.conversationId;
    if (!content || !conversationId || liveRef.current || sideRunRef.current) return false;

    const history = viewRef.current.messages;
    const sideSummary = extra?.sideSummary;
    const attachedContext = (extra?.attachedContext ?? []).filter(a => a.text.trim().length > 0);
    const userMessage: ChatMessage = {
      id: newId('msg'),
      role: 'user',
      content,
      timestamp: new Date().toISOString(),
      ...(sideSummary ? { sideSummary } : {}),
      ...(attachedContext.length > 0 ? { attachedContext } : {}),
    };
    const prompt = sideSummary ? summaryPrompt(sideSummary, content) : content;
    publish(conversationId, userMessage, 'debounced');

    // runAgent reports its own failures in the answer; acceptance is decided above.
    void runAgent(conversationId, prompt, history, options, sideSummary ? 'composed' : 'typed', extra?.fallback ?? null, attachedContext);
    return true;
  }, [publish, runAgent]);

  const retry = useCallback((assistantMessageId: string, options: SendOptions, fallback: FallbackRun | null = null) => {
    const { conversationId, messages } = viewRef.current;
    if (!conversationId || liveRef.current || sideRunRef.current) return;
    const index = messages.findIndex(m => m.id === assistantMessageId);
    if (index < 0) return;
    let userIndex = -1;
    for (let i = index - 1; i >= 0; i--) {
      if (messages[i].role === 'user') {
        userIndex = i;
        break;
      }
    }
    if (userIndex < 0) return;
    const asked = messages[userIndex];
    const prompt = asked.sideSummary ? summaryPrompt(asked.sideSummary, asked.content) : asked.content;

    if (index === messages.length - 1) {
      // Latest answer: replace it in place and re-run the same prompt. The replaced
      // tries' run ids stay with the answer, so what they saved (a request to remember,
      // which the retry finds kept) is still shown under the question.
      const replaced = messages[index].transcript;
      const priorRunIds = replaced ? [...(replaced.priorRunIds ?? []), replaced.runId] : [];
      setView(v => changeMessages(v, conversationId, ms => ms.filter(m => m.id !== assistantMessageId), 'debounced'));
      void runAgent(conversationId, prompt, messages.slice(0, userIndex), options, asked.sideSummary ? 'composed' : 'typed', fallback, asked.attachedContext ?? [], priorRunIds);
    } else {
      send(asked.content, options, {
        ...(asked.sideSummary ? { sideSummary: asked.sideSummary } : {}),
        ...(asked.attachedContext ? { attachedContext: asked.attachedContext } : {}),
        fallback,
      });
    }
  }, [runAgent, send]);

  const steer = useCallback((text: string) => {
    const message = text.trim();
    const live = liveRef.current;
    if (!message || !live || live.settled || live.sessionId === null) return;
    const sessionId = live.sessionId;
    enqueue(live, { type: 'local_steer', id: newId('steer'), text: message }, true);
    api.steer(sessionId, message)
      .then(runId => {
        if (runId === live.runId) return;
        // The answer finished before the steer arrived; the backend started
        // a new run for it. Show it as a new turn.
        const conversationId = live.conversationId;
        const adopted: LiveRun = {
          runId,
          conversationId,
          sessionId,
          message: {
            id: newId('msg'),
            role: 'assistant',
            content: '',
            timestamp: new Date().toISOString(),
            transcript: { ...initialTranscript(runId, Date.now()), status: 'running', sessionId },
          },
          queue: [],
          frame: null,
          settled: false,
          abortTimer: null,
        };
        if (liveRef.current && !liveRef.current.settled) return;
        liveRef.current = adopted;
        publish(conversationId, { id: newId('msg'), role: 'user', content: message, timestamp: new Date().toISOString() }, 'debounced');
        publish(conversationId, adopted.message, 'none');
      })
      .catch(error => notify.error('Could not steer the answer', { description: toAgentError(error).message }));
  }, [api, enqueue, publish]);

  const cancel = useCallback(() => {
    const live = liveRef.current;
    if (!live || live.settled) return;
    if (live.sessionId === null) {
      enqueue(live, { type: 'local_interrupted', atMs: Date.now() }, true);
      return;
    }
    if (live.message.transcript?.interrupting) return;
    enqueue(live, { type: 'local_interrupt_requested' }, true);
    live.abortTimer = window.setTimeout(() => {
      live.abortTimer = null;
      enqueue(live, { type: 'local_interrupted', atMs: Date.now() }, true);
    }, INTERRUPT_TIMEOUT_MS);
    api.abort(live.sessionId).catch(error => {
      const failure = toAgentError(error);
      console.error('Interrupt failed:', failure.message);
      enqueue(live, { type: 'local_interrupted', atMs: Date.now() }, true);
    });
  }, [api, enqueue]);

  const approve = useCallback((stepId: string, approved: boolean, forConversation = false, reviewToken?: string) => {
    const live = liveRef.current;
    if (!live || live.settled || live.sessionId === null) return;
    enqueue(live, { type: 'local_approval', stepId, approved }, true);
    api.approve(live.sessionId, stepId, approved, approved && forConversation, approved ? reviewToken : undefined)
      .catch(error => notify.error('The decision did not reach the agent', { description: toAgentError(error).message }));
  }, [api, enqueue]);

  // Answered in the Inbox: show the decision here too.
  useEffect(() => onApprovalAnswered(({ sessionId, stepId, approved }) => {
    const live = liveRef.current;
    if (!live || live.settled || live.sessionId !== sessionId) return;
    enqueue(live, { type: 'local_approval', stepId, approved }, true);
  }), [enqueue]);

  const appendMessage = useCallback((message: ChatMessage) => {
    const conversationId = viewRef.current.conversationId;
    if (!conversationId) return;
    publish(conversationId, message, 'debounced');
  }, [publish]);

  const appendMessageTo = useCallback((conversationId: string, message: ChatMessage) => {
    if (publish(conversationId, message, 'debounced')) return;
    const stored = toStored(message);
    updateConversationMessages(conversationId, prev => upsertById(prev, stored), { immediate: true });
  }, [publish, updateConversationMessages]);

  const updateMessage = useCallback((id: string, patch: Partial<ChatMessage>) => {
    setView(v => (v.conversationId === null
      ? v
      : changeMessages(v, v.conversationId, ms => ms.map(m => (m.id === id ? { ...m, ...patch } : m)), 'debounced')));
  }, []);

  const claimSideRun = useCallback((info: SideRunInfo) => {
    if (liveRef.current || sideRunRef.current) return false;
    sideRunRef.current = info;
    setSideRun(info);
    return true;
  }, []);

  const releaseSideRun = useCallback((threadId: string) => {
    if (sideRunRef.current?.threadId !== threadId) return;
    sideRunRef.current = null;
    setSideRun(null);
  }, []);

  const updateThreads = useCallback((conversationId: string, messageId: string, update: (threads: FocusThread[]) => FocusThread[]) => {
    const apply = (threads: FocusThread[] | undefined) => {
      const next = update(threads ?? []);
      return next.length > 0 ? next : undefined;
    };
    if (viewRef.current.conversationId === conversationId) {
      setView(v => changeMessages(
        v,
        conversationId,
        ms => ms.map(m => (m.id === messageId ? { ...m, threads: apply(m.threads) } : m)),
        'debounced',
      ));
      return;
    }
    // Background conversation: edit the stored message, keeping its other metadata.
    updateConversationMessages(conversationId, prev => prev.map(m => (m.id !== messageId
      ? m
      : { ...m, metadata: metadataWithThreads(m.metadata, apply(threadsFromMetadata(m.metadata)) ?? []) })));
  }, [updateConversationMessages]);

  const streamingConversationId = liveRun?.conversationId ?? null;

  const value = useMemo<ChatSessionValue>(() => ({
    conversations: conv.conversations,
    conversationsLoaded: conv.loaded,
    activeConversationId: conv.activeConversationId,
    activeConversation: conv.activeConversation,
    createConversation: conv.createConversation,
    switchConversation: conv.switchConversation,
    renameConversation: conv.renameConversation,
    deleteConversation: conv.deleteConversation,
    pinConversation: conv.pinConversation,
    updateConversationMeta: conv.updateConversationMeta,
    detachWorkspace: conv.detachWorkspace,
    messages: view.conversationId === conv.activeConversationId ? view.messages : [],
    isStreaming: streamingConversationId !== null && streamingConversationId === conv.activeConversationId,
    streamingConversationId,
    liveRun,
    navigation,
    runtimeInstalled,
    setRuntimeInstalled,
    prewarm,
    send,
    retry,
    steer,
    cancel,
    approve,
    appendMessage,
    appendMessageTo,
    updateMessage,
    sideRun,
    claimSideRun,
    releaseSideRun,
    updateThreads,
    updateFocusThreads: conv.updateFocusThreads,
  }), [conv, view, streamingConversationId, liveRun, navigation, runtimeInstalled, prewarm, send, retry, steer, cancel, approve, appendMessage, appendMessageTo, updateMessage, sideRun, claimSideRun, releaseSideRun, updateThreads]);

  return (
    <ChatSessionContext.Provider value={value}>
      {children}
      {conv.loadFailure && !conv.loadFailureSeen && (
        <ConversationsUnavailable notice={conv.loadFailure} onContinue={conv.acknowledgeLoadFailure} />
      )}
    </ChatSessionContext.Provider>
  );
}

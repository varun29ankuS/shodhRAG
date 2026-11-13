import React, { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useConversations } from '../../hooks/useConversations';
import { ConversationsUnavailable } from './ConversationsUnavailable';
import type { ConversationMessage } from '../../hooks/useConversations';
import { notify } from '../../lib/notify';
import { normalizeViewTab, VIEW_TAB_LABELS } from '../../lib/viewTabs';
import type { ViewTab } from '../../lib/viewTabs';
import type { AgentEventEnvelope, NavigationTarget } from '../agent/events';
import { navigationFromEvent, publishTarget } from '../agent/navigation';
import { recordAnswerVisuals } from '../visuals/recording';
import {
  answerText,
  initialTranscript,
  isLive,
  reduceAll,
} from '../agent/reducer';
import type { TranscriptAction, TranscriptState } from '../agent/reducer';
import { toAgentError, useAgentSession } from '../agent/useAgentSession';
import { useRuntimeSetup } from '../setup/RuntimeSetupContext';
import type { HistoryTurn } from '../agent/useAgentSession';
import type { FocusThread } from '../focus/focusTypes';
import { metadataWithThreads, threadsFromMetadata } from '../focus/threadStore';
import { summaryPrompt } from '../focus/summary';
import { replayedContent } from './chatHistory';
import { isConnectorConversation, readSeed, seedGoesWithNextQuestion } from '../connectors/conversations';
import type { AttachedContext } from './imageAttachments';
import {
  fromStored,
  isPersistable,
  questionIndexBefore,
  questionMessage,
  retryOptions,
  scopeOf,
  startedAnswer,
  toStored,
} from './messageStore';
import type { SideSummaryRef } from '../focus/summary';
import { connectorFocus, requestConnectorPage } from '../connectors/navigation';
import {
  arrive,
  dropHeld,
  flush as flushMoves,
  goThere,
  heldNavigationText,
  initialCoalescer,
  initialGate,
  offerNavigation,
  typingActivity,
  viewOpened,
} from '../../lib/navigationGate';
import type { CoalescerState, Counted, GateState } from '../../lib/navigationGate';
import { announce } from '../../lib/announcer';
import { creationsApi, errorText } from '../artifacts/api';
import { startsBuilding } from '../artifacts/model';
import type { ModelOverride } from '../modelPicker/modelTypes';
import type { ChatMessage, SendOptions } from './types';
import { creatorConfirmed, resolveAgentMode, suggestionIgnored } from '../agent/agentModes';
import { useDefaultAgentMode } from '../../hooks/useDefaultAgentMode';
import { onApprovalAnswered } from '../inbox/approvalBus';
import {
  appliesAtOnce,
  changeMessages,
  changedSince,
  emptyView,
  loadConversation,
  publishMessage,
  saveStep,
  upsertById,
  withLiveRun,
} from './chatPersistence';
import type { PersistMode, PersistView } from './chatPersistence';
import { originOf, wireOrigin } from './messageOrigin';
import type { MessageOrigin } from './messageOrigin';

/**
 * The answer being produced. Events are queued and folded into the
 * transcript once per animation frame, so streaming never re-renders (and
 * re-parses markdown) per token.
 *
 * Its question and answer are saved by the agent session from the moment `agent_send`
 * takes them with their turn (`savedByBackend`): the window only shows them, and reloads
 * the conversation when the backend says it changed. A request that ends before that
 * (the session did not start, the creation folder could not be made, the user stopped
 * it first) is saved by the window, as before: the question and the failed answer.
 */
interface LiveRun {
  runId: string;
  conversationId: string;
  sessionId: string | null;
  message: ChatMessage;
  /** The question it answers (null for a retry of the latest answer, which asks the one before). */
  question: ChatMessage | null;
  /** The answer a retry replaces. */
  replaces: string | null;
  /** `agent_send` was called with the turn: the agent session saves it. */
  savedByBackend: boolean;
  queue: TranscriptAction[];
  frame: number | null;
  settled: boolean;
  abortTimer: number | null;
  /** A Creator answer's creation: the Artifacts tab opens on it when building starts. */
  creationId?: string | null;
  /** The Artifacts tab was opened for this answer. */
  creationShown?: boolean;
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
  /** Display names of the failed model and of the fallback, for the line under the answer. */
  fromName?: string;
  toName?: string;
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
  /**
   * The code review pane wrote the message, not the user (a check or a fix it sends):
   * it is shown and answered like any other, but never learned from or taken for the
   * user's own words, and every edit or write of its answer asks (`messageOrigin.ts`).
   */
  fromReview?: boolean;
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
  /**
   * Open the conversation dock beside the view for this visit: a Creator build the
   * user just started opened the Artifacts tab, and they steer it from the chat.
   */
  openDock?: boolean;
}

/**
 * A move the assistant asked for, before it is followed (`lib/navigationGate`): a view
 * (null for the System status panel, which opens over the current view), what to show
 * in it, and the item's name for the chip.
 */
export interface HeldNavigation {
  view: ViewTab | null;
  focus: string | null;
  target: NavigationTarget | null;
  label: string | null;
  openDock?: boolean;
}

export interface ChatSessionValue {
  conversations: ConversationsApi['conversations'];
  /** The saved conversations were read: `conversations` is no longer loading. */
  conversationsLoaded: ConversationsApi['loaded'];
  activeConversationId: ConversationsApi['activeConversationId'];
  activeConversation: ConversationsApi['activeConversation'];
  createConversation: ConversationsApi['createConversation'];
  /** "Continue in Ask": a new chat carrying a connector conversation as context. */
  createSeededConversation: ConversationsApi['createSeededConversation'];
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
  /** A move held because the user was typing, with how many moves it stands for; the shell offers "Go there". */
  heldNavigation: Counted<HeldNavigation> | null;
  /** Follow the held navigation (the user clicked "Go there"). */
  goToHeldNavigation: () => void;
  /** Drop the held navigation (the user dismissed the chip). */
  dismissHeldNavigation: () => void;
  /** The user opened `view` themselves (a move held for it is spent). */
  noteViewOpened: (view: ViewTab) => void;
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
    saveMessages,
  } = conv;

  const [view, setView] = useState<ViewState>(() => emptyView<ChatMessage, ConversationMessage>());
  const [liveRun, setLiveRun] = useState<LiveRunInfo | null>(null);
  const [navigation, setNavigation] = useState<AgentNavigation | null>(null);
  // A navigation held while the user types ("The assistant opened X · Go there").
  const [heldNavigation, setHeldNavigation] = useState<Counted<HeldNavigation> | null>(null);
  const gateRef = useRef<GateState<HeldNavigation>>(initialGate());
  const coalescerRef = useRef<CoalescerState<HeldNavigation>>(initialCoalescer());
  const flushTimerRef = useRef<number | null>(null);
  // Shared with first-run setup and the setup card (one status, one install).
  const { installed: runtimeInstalled, setInstalled: setRuntimeInstalled } = useRuntimeSetup();
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
  // Messages changed while their answer was still running (a side thread opened on it):
  // written once it has settled.
  const deferredRef = useRef<Set<string>>(new Set());
  const loadedConvIdRef = useRef<string | null>(null);
  const navSeqRef = useRef(0);
  const switchConversationRef = useRef(conv.switchConversation);
  switchConversationRef.current = conv.switchConversation;

  /**
   * Save a view's unsaved changes to its conversation: only the messages this window
   * changed (a notice, an edit, a side thread), never its whole list, which would write
   * this window's copy over turns the agent session saved. The data and the "needs
   * saving" mark come from the same view, so a view from an older render writes only its
   * own changes and leaves newer ones due.
   */
  const saveView = useCallback((v: ViewState) => {
    const step = saveStep(v, lastSavedSeqRef.current, isPersistable, toStored);
    if (!step) return;
    const changed = changedSince(v.changed, lastSavedSeqRef.current);
    for (const id of deferredRef.current) changed.add(id);
    lastSavedSeqRef.current = step.seq;
    if (step.conversationId === null) return;
    const stored = step.messages.filter(m => changed.has(m.id));
    // A changed message still running is written once it has settled.
    const written = new Set(stored.map(m => m.id));
    deferredRef.current = new Set([...changed].filter(id => !written.has(id) && v.messages.some(m => m.id === id)));
    saveMessages(step.conversationId, stored, [], { immediate: step.immediate });
  }, [saveMessages]);

  // Each stored message restored once: a reload keeps the unchanged ones' objects
  // (`unchangedKept`), so only changed messages render (and parse markdown) again.
  const restoredRef = useRef(new WeakMap<ConversationMessage, ChatMessage>());
  const restore = useCallback((m: ConversationMessage): ChatMessage => {
    const cached = restoredRef.current.get(m);
    if (cached) return cached;
    const restored = fromStored(m);
    restoredRef.current.set(m, restored);
    return restored;
  }, []);

  /** What to show for `conversation` as stored: with the answer running in it, if any. */
  const shownMessages = useCallback((conversationId: string, stored: readonly ConversationMessage[]): ChatMessage[] => {
    const live = liveRef.current;
    const running = live && !live.settled && live.conversationId === conversationId ? live : null;
    return withLiveRun(stored.map(restore), running
      ? { message: running.message, question: running.question, replaces: running.replaces }
      : null);
  }, [restore]);

  // Load messages when the active conversation changes. A running answer for
  // that conversation is re-attached so it stays visible.
  // Layout effect: avoids painting one frame of the empty state on switch.
  useLayoutEffect(() => {
    if (!activeConversationId || activeConversationId === loadedConvIdRef.current) return;
    if (!activeConversation || activeConversation.id !== activeConversationId) return;
    // Changes not yet saved belong to the conversation being left.
    saveView(viewRef.current);
    loadedConvIdRef.current = activeConversationId;
    const loaded = shownMessages(activeConversationId, activeConversation.messages);
    setView(v => loadConversation(v, activeConversationId, loaded, {}));
  }, [activeConversationId, activeConversation, saveView, shownMessages]);

  // The visible conversation changed in the store (an answer the agent session saved, a
  // question from another device, an edit in another window): show it as stored, with
  // the answer still running in it and this window's changes not yet written.
  const reloaded = conv.reloaded;
  useEffect(() => {
    if (!reloaded || reloaded.id !== viewRef.current.conversationId) return;
    // Changes not yet saved are handed to the store first, so they stay shown.
    saveView(viewRef.current);
    const conversation = conv.getConversation(reloaded.id);
    if (!conversation) return;
    const loaded = shownMessages(reloaded.id, conversation.messages);
    setView(v => {
      if (v.conversationId !== reloaded.id) return v;
      // A change made since (a notice published in the same tick) stays, to be saved.
      const due = changedSince(v.changed, lastSavedSeqRef.current);
      const shown = v.messages.filter(m => due.has(m.id)).reduce((list, m) => upsertById(list, m), loaded);
      return loadConversation(v, reloaded.id, shown, {});
    });
  }, [reloaded, conv.getConversation, saveView, shownMessages]);

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

  /**
   * The run ended. One the agent session took is saved by it (shown here; a side thread
   * opened on it while it ran is written now). One that ended before (it never started,
   * or was stopped first) is written by the window, visible or not, without delay: its
   * question, the failed answer, and without the answer a retry replaced.
   */
  const settleRun = useCallback((live: LiveRun, message: ChatMessage) => {
    const { conversationId } = live;
    if (live.savedByBackend) {
      publish(conversationId, message, deferredRef.current.has(message.id) ? 'immediate' : 'none');
      return;
    }
    const shown = live.question ? [live.question, message] : [message];
    for (const m of shown) publish(conversationId, m, 'none');
    saveMessages(conversationId, shown.map(toStored), live.replaces ? [live.replaces] : [], { immediate: true });
  }, [publish, saveMessages]);

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
      // Display only: the agent session saves the answer as it is checked (each check
      // round starts once the text it checks is complete) and when it ends.
      publish(live.conversationId, message, 'none');
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
    settleRun(live, message);
    // Its diagrams, charts, sketches, plots, simulations, equations and tables join the gallery.
    if (transcript.status === 'completed') {
      recordAnswerVisuals({ conversationId: live.conversationId, messageId: message.id, threadId: null, turnId: null }, message.content);
    }
  }, [publish, settleRun]);

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

  // What the user types, for the navigation gate: an input event in a text field.
  useEffect(() => {
    const onInput = (event: Event) => typingActivity.note(event.target as HTMLElement | null, Date.now());
    document.addEventListener('input', onInput, true);
    return () => document.removeEventListener('input', onInput, true);
  }, []);

  /**
   * Go where the assistant asked: the view, the item inside it (scrolled to and
   * highlighted by the view that owns it), a connector's page, another
   * conversation, or the System status panel over the current view. Only ever
   * called through the gate (`deliver`, `goToHeldNavigation`).
   */
  const followNavigation = useCallback((nav: HeldNavigation) => {
    if (nav.view !== null) {
      navSeqRef.current += 1;
      setNavigation({ view: nav.view, focus: nav.focus, target: nav.target, seq: navSeqRef.current, openDock: nav.openDock });
    }
    if (nav.target?.kind === 'conversation') {
      // The run keeps streaming into its own conversation.
      switchConversationRef.current(nav.target.conversationId);
    } else if (nav.target) {
      publishTarget(nav.target);
    }
    // A connector named in the focus opens its page (a GitHub action highlights its repository).
    const page = nav.view === 'connectors' ? connectorFocus(nav.focus) : null;
    if (page) requestConnectorPage(page);
    if (nav.view !== null) window.dispatchEvent(new CustomEvent('switchTab', { detail: nav.view }));
    const where = nav.view === null ? null : VIEW_TAB_LABELS[nav.view];
    announce(
      where
        ? `Shodh opened ${where}${nav.label ? `: ${nav.label}` : ''}`
        : `Shodh opened ${nav.label ?? 'the system status'}`,
    );
  }, []);

  /**
   * The gate (lib/navigationGate): a move the coalescer let through is followed, or
   * held while the user types, so nothing moves under their hands. A held move
   * publishes nothing: no view, item or conversation changes until "Go there". The
   * chip is announced once, when it appears; later moves update it silently.
   */
  const deliver = useCallback((move: Counted<HeldNavigation>) => {
    const focused = typeof document === 'undefined' ? null : (document.activeElement as HTMLElement | null);
    const result = offerNavigation(gateRef.current, move, typingActivity.isTyping(focused, Date.now()));
    gateRef.current = result.state;
    setHeldNavigation(result.state.held);
    if (result.appeared && result.state.held) {
      const { nav, count } = result.state.held;
      announce(`${heldNavigationText(nav.view === null ? null : VIEW_TAB_LABELS[nav.view], nav.label, count)}. Go there is in the title bar.`);
    }
    if (result.follow) followNavigation(result.follow);
  }, [followNavigation]);

  /**
   * Every move of the assistant comes here first: the coalescer folds a burst into
   * one move to its last target (lib/navigationGate), then hands it to the gate.
   */
  const offer = useCallback((nav: HeldNavigation) => {
    const now = Date.now();
    const result = arrive(coalescerRef.current, nav, nav.view, now);
    coalescerRef.current = result.state;
    if (result.emit) deliver(result.emit);
    const due = result.state.dueAt;
    if (due === null) return;
    if (flushTimerRef.current !== null) window.clearTimeout(flushTimerRef.current);
    flushTimerRef.current = window.setTimeout(() => {
      flushTimerRef.current = null;
      const flushed = flushMoves(coalescerRef.current, Date.now());
      coalescerRef.current = flushed.state;
      if (flushed.emit) deliver(flushed.emit);
    }, Math.max(0, due - now));
  }, [deliver]);

  useEffect(() => () => {
    if (flushTimerRef.current !== null) window.clearTimeout(flushTimerRef.current);
  }, []);

  const goToHeldNavigation = useCallback(() => {
    const result = goThere(gateRef.current);
    gateRef.current = result.state;
    setHeldNavigation(null);
    if (result.follow) followNavigation(result.follow);
  }, [followNavigation]);

  const dismissHeldNavigation = useCallback(() => {
    gateRef.current = dropHeld(gateRef.current);
    setHeldNavigation(null);
  }, []);

  /** The user opened `view` themselves: a move held for it is spent. */
  const noteViewOpened = useCallback((view: ViewTab) => {
    const next = viewOpened(gateRef.current, view);
    if (next === gateRef.current) return;
    gateRef.current = next;
    setHeldNavigation(null);
  }, []);

  // A held move belongs to the conversation it came from: another one drops it.
  useEffect(() => {
    gateRef.current = dropHeld(gateRef.current);
    setHeldNavigation(null);
  }, [activeConversationId]);

  const onEnvelope = useCallback((envelope: AgentEventEnvelope) => {
    const live = liveRef.current;
    if (!live || live.settled) return;
    const { event } = envelope;
    if (event.runId !== live.runId) return;
    if (live.sessionId !== null && envelope.sessionId !== live.sessionId) return;
    if (event.type === 'navigated') {
      const nav = navigationFromEvent(event);
      const tab = normalizeViewTab(nav.view);
      if (nav.target?.kind === 'status') {
        // The System status panel opens over whatever view is shown.
        offer({ view: null, focus: null, target: nav.target, label: nav.label });
      } else if (tab) {
        offer({ view: tab, focus: nav.focus, target: nav.target, label: nav.label });
      }
    }
    // A Creator answer started building: the Artifacts tab opens on its creation
    // (preview pane, files filling in, progress), with the chat alongside in the dock. The
    // pane opens without the preview window (pane null): the user opens it, and nothing
    // takes the focus from the chat.
    if (event.type === 'step_started' && live.creationId && !live.creationShown && startsBuilding(event.tool)) {
      live.creationShown = true;
      const target: NavigationTarget = { kind: 'artifact', artifactId: live.creationId, version: null, pane: null };
      offer({ view: 'artifacts', focus: null, target, label: null, openDock: true });
    }
    // Lifecycle changes and the start of the answer's check are applied at
    // once; streamed content waits for the frame.
    enqueue(live, event, appliesAtOnce(event.type));
  }, [enqueue, offer]);

  const api = useAgentSession(onEnvelope);

  // Release the frame callback on unmount.
  useEffect(() => () => {
    const live = liveRef.current;
    if (live && live.frame !== null) cancelAnimationFrame(live.frame);
    if (live && live.abortTimer !== null) window.clearTimeout(live.abortTimer);
  }, []);

  // The agent runtime is not started with the app or when a conversation
  // opens: it holds hundreds of megabytes. `prewarm` starts the active
  // conversation's session once the user begins typing a question, so it is
  // ready (or nearly) when they send; `send` starts it anyway.
  const instructions = activeConversation?.systemPrompt?.trim() || null;
  const activeMode = activeConversation?.mode ?? 'research';
  const activeWorkspaceId = activeConversation?.workspaceId ?? null;
  // The mode the conversation answers in: its own, else (a new conversation) the
  // default in Settings, else Ask. Null while the default is being read: nothing
  // starts on a guess.
  const defaultAgentMode = useDefaultAgentMode();
  const defaultAgentModeRef = useRef(defaultAgentMode);
  defaultAgentModeRef.current = defaultAgentMode;
  const activeHasMessages = (activeConversation?.messages.length ?? 0) > 0;
  const activeAgentMode = resolveAgentMode(
    activeConversation?.agentMode,
    activeHasMessages,
    defaultAgentMode,
    creatorConfirmed(activeConversationId),
  );
  const streaming = liveRun !== null;
  // A connector's conversation is read-only: no agent session for it.
  const activeReadOnly = isConnectorConversation(activeConversation);
  const prewarm = useCallback(() => {
    if (!activeConversationId || runtimeInstalled !== true || streaming || activeReadOnly || !activeAgentMode) return;
    // A Creator conversation's folder is made with its first request: nothing to start before it.
    if (activeAgentMode === 'creator' && !activeHasMessages) return;
    api.start(activeConversationId, instructions, null, null, { mode: activeMode, workspaceId: activeWorkspaceId, agentMode: activeAgentMode }).catch(error => {
      const failure = toAgentError(error);
      if (failure.code === 'runtime_missing') setRuntimeInstalled(false);
      // Other failures (no model configured, no code folder, …) surface when the user asks.
    });
  }, [api, activeConversationId, instructions, activeMode, activeWorkspaceId, activeAgentMode, activeHasMessages, runtimeInstalled, streaming, activeReadOnly]);

  // `origin`: who wrote `prompt` (`messageOrigin.ts`): only `typed` text is learned from;
  // `composed` for prompts the app builds; `review` for a fix or check from the code review.
  const runAgent = useCallback(async (
    conversationId: string,
    prompt: string,
    history: ChatMessage[],
    options: SendOptions | null,
    origin: MessageOrigin,
    fallback: FallbackRun | null = null,
    attachedContext: AttachedContext[] = [],
    priorRunIds: string[] = [],
    question: ChatMessage | null = null,
    replaces: string | null = null,
  ) => {
    const runId = newId('run');
    // Saved from its start (`startedAnswer`) by the agent session when it takes the
    // question: an app closed before the answer ends shows it as interrupted, with Retry.
    const started = startedAnswer(
      newId('msg'),
      runId,
      Date.now(),
      fallback
        ? {
            from: fallback.from,
            kind: fallback.override.failure ?? 'other',
            automatic: fallback.override.automatic,
            fromName: fallback.fromName ?? null,
            toName: fallback.toName ?? null,
          }
        : null,
      priorRunIds,
    );
    const { message } = started;
    const transcript = message.transcript as TranscriptState;
    const conversationNow = activeConversationRef.current?.id === conversationId ? activeConversationRef.current : null;
    // The mode this answer runs in, sent explicitly (Ask when nothing better is known)
    // and kept on the answer; a conversation without a mode of its own keeps this one
    // from now on, so a later change of the default never changes it.
    const agentMode = resolveAgentMode(conversationNow?.agentMode, history.length > 0, defaultAgentModeRef.current, creatorConfirmed(conversationId)) ?? 'ask';
    transcript.agentMode = agentMode;
    if (conversationNow && !conversationNow.agentMode) conv.updateConversationMeta(conversationId, { agentMode });
    // The answer as the agent session saves it when it takes the question (with its mode).
    const startedStored = toStored(message);
    const live: LiveRun = {
      runId,
      conversationId,
      sessionId: null,
      message,
      question,
      replaces,
      savedByBackend: false,
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
      const mode = {
        mode: conversation?.mode ?? 'research',
        workspaceId: conversation?.workspaceId ?? null,
        agentMode,
      } as const;
      // Creator mode builds in the conversation's own folder, made fresh (and named)
      // from its first request; later requests keep it.
      if (agentMode === 'creator') {
        const creation = await creationsApi.prepare(conversationId, prompt).catch((error: unknown) => {
          throw new Error(errorText(error));
        });
        if (live.settled) return;
        live.creationId = creation.id;
      }
      const sessionId = await api.start(conversationId, conversationInstructions, null, fallback?.override ?? null, mode);
      if (live.settled) return;
      live.sessionId = sessionId;
      setRuntimeInstalled(true);
      const workspaceId = conversation?.workspaceId ?? null;
      // A mode suggestion the user went on without is not made again in this conversation.
      const answers = history.flatMap(m => (m.transcript ? [{ agentMode: m.transcript.agentMode, steps: Object.values(m.transcript.steps) }] : []));
      // From here the agent session saves the question and the answer, also when the
      // send fails: this window only shows them.
      live.savedByBackend = true;
      await api.send(sessionId, prompt, runId, historyOf(history), {
        scope: scopeOf(options, workspaceId),
        ...wireOrigin(origin),
        attachedContext,
        priorRunIds,
        suggestionIgnored: suggestionIgnored(answers, agentMode),
        turn: {
          conversationId,
          question: question ? toStored(question) : null,
          answer: startedStored,
          replaces,
        },
      });
    } catch (error) {
      const failure = toAgentError(error);
      if (failure.code === 'runtime_missing') setRuntimeInstalled(false);
      // A model that failed moments ago is not called again: its failure card shows at once.
      enqueue(live, {
        type: 'local_failed',
        error: failure.message,
        code: failure.code,
        atMs: Date.now(),
        model: failure.cooling?.model ?? null,
        providerError: failure.cooling?.providerError ?? null,
      }, true);
    }
  }, [api, enqueue, publish, setRuntimeInstalled, conv.updateConversationMeta]);

  const send = useCallback((text: string, options: SendOptions, extra?: SendExtra): boolean => {
    const content = text.trim();
    const conversationId = viewRef.current.conversationId;
    if (!content || !conversationId || liveRef.current || sideRunRef.current) return false;
    // The draft is sent: an empty composer no longer holds the assistant's moves, and a
    // move held for the last answer is spent.
    typingActivity.reset();
    gateRef.current = dropHeld(gateRef.current);
    setHeldNavigation(null);

    const history = viewRef.current.messages;
    const conversation = activeConversationRef.current?.id === conversationId ? activeConversationRef.current : null;
    // A connector's conversation is read-only: questions go to Slack or Telegram, or to a
    // chat continued from it.
    if (isConnectorConversation(conversation ?? { id: conversationId })) return false;
    const sideSummary = extra?.sideSummary;
    // A chat continued from a connector conversation carries it with its first question,
    // quoted (the user's typed words stay what is learned from and checked).
    const seed = readSeed(conversation?.seed);
    const carried = seed && seedGoesWithNextQuestion(seed, history)
      ? [{ origin: 'connector' as const, text: seed.text }]
      : [];
    const attachedContext = [...carried, ...(extra?.attachedContext ?? [])].filter(a => a.text.trim().length > 0);
    const userMessage = questionMessage(newId('msg'), content, new Date().toISOString(), options, {
      sideSummary,
      attachedContext,
      fromReview: extra?.fromReview === true,
    });
    const prompt = sideSummary ? summaryPrompt(sideSummary, content) : content;
    // Shown now; saved with its answer by the agent session when it takes it (or by this
    // window if the request fails before that).
    publish(conversationId, userMessage, 'none');
    conv.previewTitle(conversationId, content);

    // runAgent reports its own failures in the answer; acceptance is decided above.
    void runAgent(conversationId, prompt, history, options, originOf({ sideSummary, fromReview: extra?.fromReview }), extra?.fallback ?? null, attachedContext, [], userMessage);
    return true;
  }, [publish, runAgent, conv.previewTitle]);

  const retry = useCallback((assistantMessageId: string, options: SendOptions, fallback: FallbackRun | null = null) => {
    const { conversationId, messages } = viewRef.current;
    if (!conversationId || liveRef.current || sideRunRef.current) return;
    if (isConnectorConversation(activeConversationRef.current?.id === conversationId ? activeConversationRef.current : { id: conversationId })) return;
    const index = messages.findIndex(m => m.id === assistantMessageId);
    if (index < 0) return;
    const userIndex = questionIndexBefore(messages, index);
    if (userIndex < 0) return;
    const asked = messages[userIndex];
    const prompt = asked.sideSummary ? summaryPrompt(asked.sideSummary, asked.content) : asked.content;
    // A question about a file is asked about that file again, and only that file.
    const askedOptions = retryOptions(options, asked);

    if (index === messages.length - 1) {
      // Latest answer: replace it in place and re-run the same prompt. The replaced
      // tries' run ids stay with the answer, so what they saved (a request to remember,
      // which the retry finds kept) is still shown under the question.
      const replaced = messages[index].transcript;
      const priorRunIds = replaced ? [...(replaced.priorRunIds ?? []), replaced.runId] : [];
      // Hidden now; removed from the store when the new try is saved.
      setView(v => changeMessages(v, conversationId, ms => ms.filter(m => m.id !== assistantMessageId), 'none'));
      void runAgent(conversationId, prompt, messages.slice(0, userIndex), askedOptions, originOf(asked), fallback, asked.attachedContext ?? [], priorRunIds, null, assistantMessageId);
    } else {
      send(asked.content, askedOptions, {
        ...(asked.sideSummary ? { sideSummary: asked.sideSummary } : {}),
        ...(asked.attachedContext ? { attachedContext: asked.attachedContext } : {}),
        ...(asked.fromReview ? { fromReview: true } : {}),
        fallback,
      });
    }
  }, [runAgent, send]);

  const steer = useCallback((text: string) => {
    const message = text.trim();
    const live = liveRef.current;
    if (!message || !live || live.settled || live.sessionId === null) return;
    const sessionId = live.sessionId;
    const steerId = newId('steer');
    // The ids of the turn the steer becomes if the answer ended before it arrived; the
    // agent session saves that turn under them.
    const questionId = newId('msg');
    const answerId = newId('msg');
    enqueue(live, { type: 'local_steer', id: steerId, text: message }, true);
    api.steer(sessionId, message, steerId, { questionId, answerId })
      .then(runId => {
        if (runId === live.runId) return;
        // The answer finished before the steer arrived; the backend started
        // a new run for it. Show it as a new turn.
        const conversationId = live.conversationId;
        const question: ChatMessage = { id: questionId, role: 'user', content: message, timestamp: new Date().toISOString() };
        const adopted: LiveRun = {
          runId,
          conversationId,
          sessionId,
          message: {
            id: answerId,
            role: 'assistant',
            content: '',
            timestamp: new Date().toISOString(),
            transcript: { ...initialTranscript(runId, Date.now()), status: 'running', sessionId },
          },
          question,
          replaces: null,
          savedByBackend: true,
          queue: [],
          frame: null,
          settled: false,
          abortTimer: null,
        };
        if (liveRef.current && !liveRef.current.settled) return;
        liveRef.current = adopted;
        publish(conversationId, question, 'none');
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
    saveMessages(conversationId, [toStored(message)], [], { immediate: true });
  }, [publish, saveMessages]);

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
    const stored = conv.getConversation(conversationId)?.messages.find(m => m.id === messageId);
    if (!stored) return;
    saveMessages(conversationId, [{
      ...stored,
      metadata: metadataWithThreads(stored.metadata, apply(threadsFromMetadata(stored.metadata)) ?? []),
    }]);
  }, [conv.getConversation, saveMessages]);

  const streamingConversationId = liveRun?.conversationId ?? null;

  const value = useMemo<ChatSessionValue>(() => ({
    conversations: conv.conversations,
    conversationsLoaded: conv.loaded,
    activeConversationId: conv.activeConversationId,
    activeConversation: conv.activeConversation,
    createConversation: conv.createConversation,
    createSeededConversation: conv.createSeededConversation,
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
    heldNavigation,
    goToHeldNavigation,
    dismissHeldNavigation,
    noteViewOpened,
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
  }), [conv, view, streamingConversationId, liveRun, navigation, heldNavigation, goToHeldNavigation, dismissHeldNavigation, noteViewOpened, runtimeInstalled, prewarm, send, retry, steer, cancel, approve, appendMessage, appendMessageTo, updateMessage, sideRun, claimSideRun, releaseSideRun, updateThreads]);

  return (
    <ChatSessionContext.Provider value={value}>
      {children}
      {conv.loadFailure && !conv.loadFailureSeen && (
        <ConversationsUnavailable notice={conv.loadFailure} onContinue={conv.acknowledgeLoadFailure} />
      )}
    </ChatSessionContext.Provider>
  );
}

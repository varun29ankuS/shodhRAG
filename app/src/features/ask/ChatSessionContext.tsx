import React, { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { useConversations } from '../../hooks/useConversations';
import type { ConversationMessage } from '../../hooks/useConversations';
import { useActivityTracker } from '../../hooks/useActivityTracker';
import { extractArtifacts } from '../../utils/artifactExtractor';
import { buildSearchStep, describeArguments } from './run';
import type {
  ChatMessage,
  RawSearchResult,
  ResponseMetadata,
  RunRecord,
  RunStatus,
  RunStep,
  SendOptions,
} from './types';

/** Shape of `shodh_rag::chat::AssistantResponse` as returned by `unified_chat`. */
interface AssistantResponsePayload {
  content?: string;
  artifacts?: any[];
  suggestions?: string[];
  search_results?: RawSearchResult[] | null;
  metadata?: ResponseMetadata;
}

/** Request currently streaming. Mutated only outside React state updaters. */
interface LiveRequest {
  requestId: string;
  conversationId: string;
  startedAtMs: number;
  message: ChatMessage;
  pendingDelta: string;
  frame: number | null;
  stepSeq: number;
  settled: boolean;
}

interface ViewState {
  /** Conversation the messages belong to (persistence target). */
  conversationId: string | null;
  messages: ChatMessage[];
}

const HISTORY_LIMIT = 10;

type ConversationsApi = ReturnType<typeof useConversations>;

export interface ChatSessionValue {
  conversations: ConversationsApi['conversations'];
  activeConversationId: ConversationsApi['activeConversationId'];
  activeConversation: ConversationsApi['activeConversation'];
  createConversation: ConversationsApi['createConversation'];
  switchConversation: ConversationsApi['switchConversation'];
  renameConversation: ConversationsApi['renameConversation'];
  deleteConversation: ConversationsApi['deleteConversation'];
  pinConversation: ConversationsApi['pinConversation'];
  updateConversationMeta: ConversationsApi['updateConversationMeta'];

  /** Messages of the active conversation, including a live streaming answer. */
  messages: ChatMessage[];
  /** True while a request for the active conversation is in flight. */
  isStreaming: boolean;
  /** Conversation a request is in flight for, if any (only one at a time). */
  streamingConversationId: string | null;

  send: (text: string, options: SendOptions) => Promise<void>;
  /** Re-run the user prompt that produced `assistantMessageId`. */
  retry: (assistantMessageId: string, options: SendOptions) => void;
  /** Stop listening to the in-flight request and mark its answer as stopped. */
  cancel: () => void;
  /** Append a non-chat message (e.g. OCR or upload notices) to the active conversation. */
  appendMessage: (message: ChatMessage) => void;
  updateMessage: (id: string, patch: Partial<ChatMessage>) => void;
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

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  try {
    return JSON.stringify(error);
  } catch {
    return 'Unknown error';
  }
}

function isRunStatus(value: unknown): value is RunStatus {
  return value === 'running' || value === 'done' || value === 'failed' || value === 'cancelled';
}

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
  return {
    id: m.id,
    role: m.role,
    content: m.content,
    timestamp: m.timestamp,
    artifacts: m.artifacts,
    searchResults: m.searchResults as RawSearchResult[] | undefined,
    metadata: isRecord(m.metadata) ? (m.metadata as ResponseMetadata) : undefined,
    run: readRun(m.run),
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
  if (m.metadata) stored.metadata = m.metadata as Record<string, unknown>;
  if (m.run) {
    const { activity: _activity, ...persistable } = m.run;
    stored.run = persistable as unknown as Record<string, unknown>;
  }
  return stored;
}

function isPersistable(m: ChatMessage): boolean {
  return m.run?.status !== 'running';
}

export function ChatSessionProvider({ children }: { children: React.ReactNode }) {
  const conv = useConversations();
  const {
    activeConversationId,
    activeConversation,
    updateConversationMessages,
    updateConversationMeta,
  } = conv;
  const { trackActivity } = useActivityTracker();

  const [view, setView] = useState<ViewState>({ conversationId: null, messages: [] });
  const [streamingConversationId, setStreamingConversationId] = useState<string | null>(null);

  const liveRef = useRef<LiveRequest | null>(null);
  const viewRef = useRef(view);
  viewRef.current = view;
  const activeConversationRef = useRef(activeConversation);
  activeConversationRef.current = activeConversation;
  const dirtyRef = useRef(false);
  const loadedConvIdRef = useRef<string | null>(null);

  // Load messages when the active conversation changes. A request still
  // streaming for that conversation is re-attached so its answer stays visible.
  // Layout effect: avoids painting one frame of the empty state on switch.
  useLayoutEffect(() => {
    if (!activeConversationId || activeConversationId === loadedConvIdRef.current) return;
    if (!activeConversation || activeConversation.id !== activeConversationId) return;
    loadedConvIdRef.current = activeConversationId;
    const loaded = activeConversation.messages.map(fromStored);
    const live = liveRef.current;
    if (live && !live.settled && live.conversationId === activeConversationId) {
      loaded.push(live.message);
    }
    dirtyRef.current = false;
    setView({ conversationId: activeConversationId, messages: loaded });
  }, [activeConversationId, activeConversation]);

  // Persist after real mutations only (never on load), so switching
  // conversations does not bump `updatedAt` and reorder the sidebar.
  useEffect(() => {
    if (!dirtyRef.current || !view.conversationId) return;
    dirtyRef.current = false;
    const stored = view.messages.filter(isPersistable).map(toStored);
    updateConversationMessages(view.conversationId, () => stored);
  }, [view, updateConversationMessages]);

  /** Replace a message in the view if it belongs to the visible conversation. */
  const publish = useCallback((conversationId: string, message: ChatMessage, persist: boolean) => {
    if (viewRef.current.conversationId !== conversationId) return false;
    if (persist) dirtyRef.current = true;
    setView(v => {
      if (v.conversationId !== conversationId) return v;
      const exists = v.messages.some(m => m.id === message.id);
      return {
        ...v,
        messages: exists ? v.messages.map(m => (m.id === message.id ? message : m)) : [...v.messages, message],
      };
    });
    return true;
  }, []);

  /** Write a finished message to its conversation, visible or not. */
  const commit = useCallback((conversationId: string, message: ChatMessage) => {
    if (publish(conversationId, message, true)) return;
    const stored = toStored(message);
    updateConversationMessages(conversationId, prev => {
      const exists = prev.some(m => m.id === stored.id);
      return exists ? prev.map(m => (m.id === stored.id ? stored : m)) : [...prev, stored];
    });
  }, [publish, updateConversationMessages]);

  const updateLive = useCallback((mutate: (message: ChatMessage, live: LiveRequest) => ChatMessage) => {
    const live = liveRef.current;
    if (!live || live.settled) return;
    live.message = mutate(live.message, live);
    publish(live.conversationId, live.message, false);
  }, [publish]);

  const flushDelta = useCallback(() => {
    const live = liveRef.current;
    if (!live) return;
    live.frame = null;
    if (live.settled || !live.pendingDelta) return;
    const delta = live.pendingDelta;
    live.pendingDelta = '';
    updateLive(m => ({
      ...m,
      content: m.content + delta,
      run: m.run ? { ...m.run, activity: 'Writing the answer' } : m.run,
    }));
  }, [updateLive]);

  const updateRun = useCallback((mutate: (run: RunRecord, live: LiveRequest) => RunRecord) => {
    updateLive((m, live) => (m.run ? { ...m, run: mutate(m.run, live) } : m));
  }, [updateLive]);

  const startStep = useCallback((kind: RunStep['kind'], title: string, detail: string | undefined, activity: string) => {
    updateRun((run, live) => {
      live.stepSeq += 1;
      const step: RunStep = { id: `step-${live.stepSeq}`, kind, title, detail, status: kind === 'thinking' ? 'done' : 'running' };
      return { ...run, activity, steps: [...run.steps, step] };
    });
  }, [updateRun]);

  const completeTool = useCallback((toolName: string, success: boolean, durationMs: number | undefined) => {
    updateRun(run => {
      const steps = [...run.steps];
      for (let i = steps.length - 1; i >= 0; i--) {
        if (steps[i].kind === 'tool' && steps[i].title === toolName && steps[i].status === 'running') {
          steps[i] = { ...steps[i], status: success ? 'done' : 'failed', durationMs };
          break;
        }
      }
      return { ...run, steps, activity: 'Working' };
    });
  }, [updateRun]);

  // Streaming listeners, registered once. Events carry `requestId`; anything
  // not belonging to the live request (e.g. a cancelled one) is ignored.
  const handlersRef = useRef({ flushDelta, startStep, completeTool, updateRun });
  handlersRef.current = { flushDelta, startStep, completeTool, updateRun };

  useEffect(() => {
    let disposed = false;
    const unlisteners: UnlistenFn[] = [];

    const forLive = (payload: unknown): Record<string, unknown> | null => {
      const live = liveRef.current;
      if (!live || live.settled || !isRecord(payload)) return null;
      return payload.requestId === live.requestId ? payload : null;
    };

    const register = (event: string, handler: (payload: Record<string, unknown>) => void) => {
      listen<unknown>(event, e => {
        const payload = forLive(e.payload);
        if (payload) handler(payload);
      })
        .then(unlisten => {
          if (disposed) unlisten();
          else unlisteners.push(unlisten);
        })
        .catch(err => console.error(`Failed to listen for ${event}:`, err));
    };

    register('chat_token', p => {
      const live = liveRef.current;
      if (!live || typeof p.delta !== 'string' || p.delta.length === 0) return;
      live.pendingDelta += p.delta;
      if (live.frame === null) {
        live.frame = requestAnimationFrame(() => handlersRef.current.flushDelta());
      }
    });

    const onToolStart = (name: unknown, args: unknown) => {
      if (typeof name !== 'string' || !name) return;
      handlersRef.current.startStep('tool', name, describeArguments(args), `Running ${name}`);
    };
    const onToolComplete = (name: unknown, success: unknown, duration: unknown) => {
      if (typeof name !== 'string' || !name) return;
      handlersRef.current.completeTool(
        name,
        success !== false,
        typeof duration === 'number' ? duration : undefined,
      );
    };
    const onThinking = (message: unknown) => {
      if (typeof message !== 'string' || !message.trim()) return;
      const text = message.trim().replace(/\.{3}$|…$/, '');
      const steps = liveRef.current?.message.run?.steps ?? [];
      const last = steps[steps.length - 1];
      if (last && last.kind === 'thinking' && last.title === text) {
        handlersRef.current.updateRun(run => ({ ...run, activity: text }));
      } else {
        handlersRef.current.startStep('thinking', text, undefined, text);
      }
    };

    register('tool_call_start', p => onToolStart(p.tool_name, p.arguments));
    register('tool_call_complete', p => onToolComplete(p.tool_name, p.success, p.duration_ms));
    register('tool_execution', p => {
      if (p.stage === 'executing') onToolStart(p.tool, p.arguments);
      else if (p.stage === 'completed') onToolComplete(p.tool, p.success, p.duration_ms);
      else if (p.stage === 'thinking') onThinking(p.message);
    });
    register('agent_thinking', p => onThinking(p.message));
    register('agent_creation_progress', p => {
      if (typeof p.message !== 'string' || !p.message.trim()) return;
      const text = p.message.trim().replace(/\.{3}$|…$/, '');
      handlersRef.current.updateRun(run => ({ ...run, activity: text }));
    });

    return () => {
      disposed = true;
      unlisteners.forEach(unlisten => unlisten());
      const live = liveRef.current;
      if (live && live.frame !== null) {
        cancelAnimationFrame(live.frame);
        live.frame = null;
      }
    };
  }, []);

  /** Settle the live request: stop accepting events and free the slot. */
  const settleLive = useCallback((live: LiveRequest) => {
    live.settled = true;
    if (live.frame !== null) {
      cancelAnimationFrame(live.frame);
      live.frame = null;
    }
    if (liveRef.current === live) liveRef.current = null;
    setStreamingConversationId(null);
  }, []);

  const runRequest = useCallback(async (
    conversationId: string,
    prompt: string,
    history: ChatMessage[],
    options: SendOptions,
  ) => {
    const requestId = newId('req');
    const startedAtMs = Date.now();
    const message: ChatMessage = {
      id: newId('msg'),
      role: 'assistant',
      content: '',
      timestamp: new Date(startedAtMs).toISOString(),
      run: { status: 'running', startedAt: new Date(startedAtMs).toISOString(), steps: [], activity: 'Working' },
    };
    const live: LiveRequest = {
      requestId,
      conversationId,
      startedAtMs,
      message,
      pendingDelta: '',
      frame: null,
      stepSeq: 0,
      settled: false,
    };
    liveRef.current = live;
    setStreamingConversationId(conversationId);
    publish(conversationId, message, false);

    const conversationHistory = history
      .filter(m => (m.role === 'user' || m.role === 'assistant') && m.content.trim().length > 0 && m.run?.status !== 'running')
      .slice(-HISTORY_LIMIT)
      .map(m => ({ role: m.role, content: m.content }));

    const systemPrompt = activeConversationRef.current?.id === conversationId
      ? activeConversationRef.current.systemPrompt ?? null
      : null;

    try {
      const response = await invoke<AssistantResponsePayload>('unified_chat', {
        message: prompt,
        context: {
          agent_id: null,
          conversation_history: conversationHistory,
          space_id: options.spaceId,
          conversation_id: null,
          custom_system_prompt: systemPrompt,
        },
        requestId,
      });
      if (live.settled) return; // cancelled while waiting

      const content = typeof response?.content === 'string' ? response.content : live.message.content;
      const searchResults = Array.isArray(response?.search_results) ? response.search_results : [];
      const metadata = isRecord(response?.metadata) ? response.metadata : undefined;
      const artifacts = [
        ...(Array.isArray(response?.artifacts) ? response.artifacts : []),
        ...extractArtifacts(content),
      ];
      const searchStep = buildSearchStep(metadata, searchResults.length);
      const priorRun = live.message.run;
      const steps = (priorRun?.steps ?? []).map(s => (s.status === 'running' ? { ...s, status: 'done' as const } : s));
      const finalMessage: ChatMessage = {
        ...live.message,
        content,
        artifacts: artifacts.length > 0 ? artifacts : undefined,
        searchResults: searchResults.length > 0 ? searchResults : undefined,
        metadata,
        run: {
          status: 'done',
          startedAt: priorRun?.startedAt ?? message.timestamp,
          elapsedMs: Date.now() - startedAtMs,
          steps: searchStep ? [searchStep, ...steps] : steps,
        },
      };
      settleLive(live);
      commit(conversationId, finalMessage);
    } catch (error) {
      if (live.settled) return;
      const pending = live.pendingDelta;
      const priorRun = live.message.run;
      const finalMessage: ChatMessage = {
        ...live.message,
        content: live.message.content + pending,
        run: {
          status: 'failed',
          startedAt: priorRun?.startedAt ?? message.timestamp,
          elapsedMs: Date.now() - startedAtMs,
          steps: (priorRun?.steps ?? []).map(s => (s.status === 'running' ? { ...s, status: 'stopped' as const } : s)),
          error: errorMessage(error),
        },
      };
      settleLive(live);
      commit(conversationId, finalMessage);
    }
  }, [commit, publish, settleLive]);

  const send = useCallback(async (text: string, options: SendOptions) => {
    const prompt = text.trim();
    const conversationId = viewRef.current.conversationId;
    if (!prompt || !conversationId || liveRef.current) return;

    const history = viewRef.current.messages;
    const userMessage: ChatMessage = {
      id: newId('msg'),
      role: 'user',
      content: prompt,
      timestamp: new Date().toISOString(),
    };
    publish(conversationId, userMessage, true);

    const active = activeConversationRef.current;
    if (active && active.id === conversationId && !active.spaceId && options.spaceId && options.spaceName) {
      updateConversationMeta(conversationId, { spaceId: options.spaceId, spaceName: options.spaceName });
    }
    void trackActivity({ activityType: 'search', data: `Chat: "${prompt}"`, project: 'shodh' });

    await runRequest(conversationId, prompt, history, options);
  }, [publish, runRequest, trackActivity, updateConversationMeta]);

  const retry = useCallback((assistantMessageId: string, options: SendOptions) => {
    const { conversationId, messages } = viewRef.current;
    if (!conversationId || liveRef.current) return;
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
    const prompt = messages[userIndex].content;

    if (index === messages.length - 1) {
      // Latest answer: replace it in place and re-run the same prompt.
      dirtyRef.current = true;
      setView(v => (v.conversationId === conversationId
        ? { ...v, messages: v.messages.filter(m => m.id !== assistantMessageId) }
        : v));
      void runRequest(conversationId, prompt, messages.slice(0, userIndex), options);
    } else {
      void send(prompt, options);
    }
  }, [runRequest, send]);

  const cancel = useCallback(() => {
    const live = liveRef.current;
    if (!live || live.settled) return;
    const priorRun = live.message.run;
    const finalMessage: ChatMessage = {
      ...live.message,
      content: live.message.content + live.pendingDelta,
      run: {
        status: 'cancelled',
        startedAt: priorRun?.startedAt ?? live.message.timestamp,
        elapsedMs: Date.now() - live.startedAtMs,
        steps: (priorRun?.steps ?? []).map(s => (s.status === 'running' ? { ...s, status: 'stopped' as const } : s)),
      },
    };
    settleLive(live);
    commit(live.conversationId, finalMessage);
  }, [commit, settleLive]);

  const appendMessage = useCallback((message: ChatMessage) => {
    const conversationId = viewRef.current.conversationId;
    if (!conversationId) return;
    publish(conversationId, message, true);
  }, [publish]);

  const updateMessage = useCallback((id: string, patch: Partial<ChatMessage>) => {
    dirtyRef.current = true;
    setView(v => ({ ...v, messages: v.messages.map(m => (m.id === id ? { ...m, ...patch } : m)) }));
  }, []);

  const value = useMemo<ChatSessionValue>(() => ({
    conversations: conv.conversations,
    activeConversationId: conv.activeConversationId,
    activeConversation: conv.activeConversation,
    createConversation: conv.createConversation,
    switchConversation: conv.switchConversation,
    renameConversation: conv.renameConversation,
    deleteConversation: conv.deleteConversation,
    pinConversation: conv.pinConversation,
    updateConversationMeta: conv.updateConversationMeta,
    messages: view.conversationId === conv.activeConversationId ? view.messages : [],
    isStreaming: streamingConversationId !== null && streamingConversationId === conv.activeConversationId,
    streamingConversationId,
    send,
    retry,
    cancel,
    appendMessage,
    updateMessage,
  }), [conv, view, streamingConversationId, send, retry, cancel, appendMessage, updateMessage]);

  return <ChatSessionContext.Provider value={value}>{children}</ChatSessionContext.Provider>;
}

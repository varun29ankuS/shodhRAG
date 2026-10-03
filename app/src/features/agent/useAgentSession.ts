/**
 * Thin client for the agent session commands (`agent_session_commands.rs`)
 * and the `agent_event` stream.
 */
import { useEffect, useMemo, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import type { AgentEventEnvelope } from './events';
import type { FailureCode } from './reducer';

export const AGENT_EVENT = 'agent_event';
export const RUNTIME_PROGRESS_EVENT = 'agent_runtime_progress';

/** `AgentCommandError` from the backend. */
export interface AgentError {
  code: FailureCode;
  message: string;
}

const CODES: readonly FailureCode[] = [
  'runtime_missing',
  'runtime_invalid',
  'model_config',
  'busy',
  'invalid_request',
  'session_closed',
  'runtime_error',
];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Normalise anything a command rejected with. */
export function toAgentError(error: unknown): AgentError {
  if (isRecord(error) && typeof error.message === 'string') {
    const code = CODES.includes(error.code as FailureCode) ? (error.code as FailureCode) : 'runtime_error';
    return { code, message: error.message };
  }
  if (error instanceof Error) return { code: 'runtime_error', message: error.message };
  if (typeof error === 'string') return { code: 'runtime_error', message: error };
  return { code: 'runtime_error', message: 'The agent did not respond.' };
}

/** What one answer may search; empty lists mean everything indexed. */
export interface AnswerScope {
  sourceIds: string[];
  sourceFiles: string[];
}

export interface HistoryTurn {
  role: 'user' | 'assistant';
  content: string;
}

export interface RuntimeStatus {
  installed: boolean;
  path: string;
  version: string;
}

export interface RuntimeInstall {
  path: string;
  version: string;
  sha256: string;
}

export interface RuntimeProgress {
  downloaded: number;
  total: number | null;
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw toAgentError(error);
  }
}

export const agentApi = {
  /** Start or reuse the conversation's session. Rejects with `AgentError`. */
  start: (conversationId: string, instructions: string | null) =>
    call<string>('agent_start', { conversationId, profileId: null, instructions }),
  /** `scope` limits what the answer may search (selected sources, or files for "Ask about this file"). */
  send: (sessionId: string, text: string, requestId: string, history: HistoryTurn[], scope: AnswerScope | null = null) =>
    call<string>('agent_send', { sessionId, text, requestId, history, scope }),
  steer: (sessionId: string, text: string) => call<string>('agent_steer', { sessionId, text }),
  abort: (sessionId: string) => call<void>('agent_abort', { sessionId }),
  approve: (sessionId: string, stepId: string, approved: boolean) =>
    call<void>('agent_approve', { sessionId, stepId, approved }),
  runtimeStatus: () => call<RuntimeStatus>('agent_runtime_status'),
  installRuntime: () => call<RuntimeInstall>('agent_install_runtime'),
};

function isEnvelope(value: unknown): value is AgentEventEnvelope {
  return (
    isRecord(value) &&
    typeof value.sessionId === 'string' &&
    isRecord(value.event) &&
    typeof value.event.type === 'string' &&
    typeof value.event.runId === 'string'
  );
}

/** Subscribe to a Tauri event for the component's lifetime. */
function useTauriEvent<T>(event: string, guard: (value: unknown) => value is T, handler: (payload: T) => void) {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    listen<unknown>(event, e => {
      if (guard(e.payload)) handlerRef.current(e.payload);
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(err => console.error(`Failed to listen for ${event}:`, err));
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [event, guard]);
}

function isProgress(value: unknown): value is RuntimeProgress {
  return isRecord(value) && typeof value.downloaded === 'number';
}

/**
 * The agent session client. `onEvent` receives every `agent_event`; it is
 * registered once at mount, so events emitted while a command is still
 * resolving are never missed.
 */
export function useAgentSession(onEvent: (envelope: AgentEventEnvelope) => void) {
  useTauriEvent(AGENT_EVENT, isEnvelope, onEvent);
  return useMemo(() => agentApi, []);
}

/** Runtime download progress while `agentApi.installRuntime` runs. */
export function useRuntimeProgress(onProgress: (progress: RuntimeProgress) => void) {
  useTauriEvent(RUNTIME_PROGRESS_EVENT, isProgress, onProgress);
}

/**
 * The `agent_event` stream: every event of every agent run. Only the main window's
 * chat session (and the panes it hosts) subscribe to it; the commands and types are in
 * `agentApi.ts`.
 */
import { useMemo } from 'react';
import type { AgentEventEnvelope } from './events';
import { agentApi, useTauriEvent } from './agentApi';

export const AGENT_EVENT = 'agent_event';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isEnvelope(value: unknown): value is AgentEventEnvelope {
  return (
    isRecord(value) &&
    typeof value.sessionId === 'string' &&
    isRecord(value.event) &&
    typeof value.event.type === 'string' &&
    typeof value.event.runId === 'string'
  );
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

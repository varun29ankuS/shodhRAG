/**
 * AgentEvent contract — mirror of `crates/shodh-rag/src/harness/events.rs`.
 *
 * Serialised by serde with a snake_case `type` discriminator and camelCase
 * fields. Optional values are always present and `null` when absent.
 * A Rust contract test asserts that every variant tag and key below exists,
 * so keep the `key: Type` spelling when editing.
 */

export type RiskTier = "read" | "write" | "destructive";

export type RunStatus = "completed" | "aborted" | "error";

export type PlanStatus = "pending" | "in_progress" | "done";

export interface PlanItem {
  id: string;
  text: string;
  status: PlanStatus;
}

export interface RunStartedEvent {
  type: "run_started";
  runId: string;
  sessionId: string;
  model: string;
  atMs: number;
}

export interface TextDeltaEvent {
  type: "text_delta";
  runId: string;
  messageId: string;
  delta: string;
}

export interface ThinkingEvent {
  type: "thinking";
  runId: string;
  messageId: string;
  delta: string;
}

export interface StepStartedEvent {
  type: "step_started";
  runId: string;
  stepId: string;
  parentStepId: string | null;
  tool: string;
  label: string;
  args: unknown;
  tier: RiskTier;
  atMs: number;
}

export interface StepProgressEvent {
  type: "step_progress";
  runId: string;
  stepId: string;
  text: string;
}

export interface StepFinishedEvent {
  type: "step_finished";
  runId: string;
  stepId: string;
  ok: boolean;
  summary: string;
  detail: unknown | null;
  durationMs: number;
}

export interface ApprovalRequestedEvent {
  type: "approval_requested";
  runId: string;
  stepId: string;
  tool: string;
  label: string;
  tier: RiskTier;
  preview: unknown;
}

export interface PlanUpdatedEvent {
  type: "plan_updated";
  runId: string;
  items: PlanItem[];
}

export interface NavigatedEvent {
  type: "navigated";
  runId: string;
  view: string;
  focus: string | null;
}

export interface UsageEvent {
  type: "usage";
  runId: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  costUsd: number;
}

export interface RunFinishedEvent {
  type: "run_finished";
  runId: string;
  status: RunStatus;
  durationMs: number;
  error: string | null;
}

export type AgentEvent =
  | RunStartedEvent
  | TextDeltaEvent
  | ThinkingEvent
  | StepStartedEvent
  | StepProgressEvent
  | StepFinishedEvent
  | ApprovalRequestedEvent
  | PlanUpdatedEvent
  | NavigatedEvent
  | UsageEvent
  | RunFinishedEvent;

/** Payload of the Tauri `agent_event` event. */
export interface AgentEventEnvelope {
  sessionId: string;
  event: AgentEvent;
}

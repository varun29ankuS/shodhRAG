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
  /** Data-handling warning for the selected model, shown for the whole run. */
  warning: string | null;
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

/** Where a `navigated` event points inside a view (`kind` discriminator). */
export interface DocumentTarget {
  kind: "document";
  path: string;
  /** 1-based page, when known. */
  page: number | null;
  /** Text to highlight in the document. */
  passage: string | null;
}

export interface CalendarTarget {
  kind: "calendar";
  /** YYYY-MM-DD. */
  date: string | null;
  taskId: string | null;
  eventId: string | null;
}

export interface ConversationTarget {
  kind: "conversation";
  conversationId: string;
}

export interface AuditTarget {
  kind: "audit";
  /** Audit event types, e.g. "tool_call"; empty means all. */
  types: string[];
  tool: string | null;
  /** RFC 3339 bounds. */
  from: string | null;
  to: string | null;
  text: string | null;
}

export interface SourceTarget {
  kind: "source";
  sourceId: string;
}

/** A generated visual (gallery), opened in the focus pop-out. */
export interface VisualTarget {
  kind: "visual";
  visualId: string;
  /** Version to show; the latest when null. */
  version: number | null;
}

export type NavigationTarget =
  | DocumentTarget
  | CalendarTarget
  | ConversationTarget
  | AuditTarget
  | SourceTarget
  | VisualTarget;

export interface NavigatedEvent {
  type: "navigated";
  runId: string;
  view: string;
  focus: string | null;
  /** What to show inside the view; null when the event only switches views. */
  target: NavigationTarget | null;
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

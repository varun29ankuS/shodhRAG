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

/** Whether retrieved passages cover an information need. */
export type CoverageState = "covered" | "missing";

export interface PlanItem {
  id: string;
  text: string;
  status: PlanStatus;
  /** An information need of the question, checked against the passages. False in older events. */
  need: boolean;
  /** For a need, once checked; null otherwise. */
  coverage: CoverageState | null;
  /** For a covered need: the passages that cover it, best first. */
  evidence: number[];
}

/** How one claim of an answer relates to its sources. */
export type ClaimOutcome =
  | "supported"
  | "weak"
  | "unsupported"
  | "uncited_factual"
  | "invalid_citation"
  | "unchecked";

/** How support was scored: the local models plus number checks, or word overlap only. */
export type ScoringMethod = "entailment" | "cross_encoder" | "lexical";

export type ClaimKind = "sentence" | "list_item" | "table_row";

export interface ClaimCheck {
  /** Text block (assistant message) the claim is in. */
  messageId: string;
  /** The claim as plain text. */
  text: string;
  /** Exact text of the message after which the claim's flag goes. */
  anchor: string;
  kind: ClaimKind;
  outcome: ClaimOutcome;
  cited: number[];
  /** Cited numbers that no passage of the answer has. */
  invalid: number[];
  /** Best support score of the cited passages, 0..1. */
  support: number | null;
  /** With the entailment model: the highest probability that a cited passage contradicts the claim. */
  contradiction: number | null;
  /** Numbers the claim states that its cited passages do not contain. */
  missingNumbers: string[];
  /** For a flagged claim: the passage that comes closest to supporting it. */
  closest: number | null;
  closestScore: number | null;
}

export interface GroundingSummary {
  checked: number;
  supported: number;
  weak: number;
  unsupported: number;
  uncited: number;
  invalid: number;
  unchecked: number;
  /** (supported + weak / 2) / (checked - unchecked); null when nothing checkable was claimed. */
  score: number | null;
}

export interface NeedCheck {
  id: string;
  text: string;
  state: CoverageState;
  passages: number[];
}

export interface GroundingReport {
  /** 0 for the first answer, then one per follow-up turn. */
  round: number;
  /** The last check of the run: the answer's grounding. */
  isFinal: boolean;
  method: ScoringMethod;
  summary: GroundingSummary;
  claims: ClaimCheck[];
  needs: NeedCheck[];
  /** Text blocks the claims come from. */
  messageIds: string[];
  /** Text blocks replaced by a revised answer (kept as an earlier draft). */
  supersededMessageIds: string[];
}

export type RevisionReason = "repair" | "coverage" | "repair_and_coverage";

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

/** A workspace's page, at a tab (the overview when null). */
export interface WorkspaceTarget {
  kind: "workspace";
  workspaceId: string;
  /** "overview", "sources", "chats", "memory", "visuals" or "results". */
  tab: string | null;
}

export type NavigationTarget =
  | DocumentTarget
  | CalendarTarget
  | ConversationTarget
  | AuditTarget
  | SourceTarget
  | VisualTarget
  | WorkspaceTarget;

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

/** The model finished a round of answering; the answer is being checked against its sources. */
export interface GroundingStartedEvent {
  type: "grounding_started";
  runId: string;
  round: number;
}

/** The grounding check of the answer so far; `report.isFinal` on the last one. */
export interface GroundingEvent {
  type: "grounding";
  runId: string;
  report: GroundingReport;
}

/** The app asked the model for one more turn to fix flagged claims or search for missing parts. */
export interface RevisionStartedEvent {
  type: "revision_started";
  runId: string;
  round: number;
  reason: RevisionReason;
  flagged: number;
  missingNeeds: string[];
}

export interface RunFinishedEvent {
  type: "run_finished";
  runId: string;
  status: RunStatus;
  durationMs: number;
  error: string | null;
  /** What kind of provider failure `error` is (read with `readProviderError`). */
  providerError: unknown;
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
  | GroundingStartedEvent
  | GroundingEvent
  | RevisionStartedEvent
  | RunFinishedEvent;

/** Payload of the Tauri `agent_event` event. */
export interface AgentEventEnvelope {
  sessionId: string;
  event: AgentEvent;
}

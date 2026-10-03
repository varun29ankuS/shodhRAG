/**
 * Mirror of the Rust audit types (`shodh_rag::audit`), as serialised by the
 * `audit_*` Tauri commands. Row/stat fields are camelCase; payload keys are
 * snake_case because payloads are stored verbatim.
 */

export type AuditEventType =
  | 'question'
  | 'tool_call'
  | 'approval'
  | 'retrieval'
  | 'answer'
  | 'source_change'
  | 'settings_change'
  | 'runtime_install'
  | 'retention_checkpoint';

export const EVENT_TYPES: readonly { id: AuditEventType; label: string }[] = [
  { id: 'question', label: 'Questions' },
  { id: 'answer', label: 'Answers' },
  { id: 'tool_call', label: 'Tool calls' },
  { id: 'retrieval', label: 'Retrievals' },
  { id: 'approval', label: 'Approvals' },
  { id: 'source_change', label: 'Sources' },
  { id: 'settings_change', label: 'Settings' },
  { id: 'runtime_install', label: 'Runtime' },
  { id: 'retention_checkpoint', label: 'Retention' },
];

export function eventLabel(type: string): string {
  switch (type) {
    case 'question':
      return 'Question';
    case 'answer':
      return 'Answer';
    case 'tool_call':
      return 'Tool call';
    case 'retrieval':
      return 'Retrieval';
    case 'approval':
      return 'Approval';
    case 'source_change':
      return 'Source';
    case 'settings_change':
      return 'Settings';
    case 'runtime_install':
      return 'Runtime';
    case 'retention_checkpoint':
      return 'Retention';
    default:
      return type;
  }
}

export interface AuditRow {
  id: number;
  ts: string;
  principal: string;
  conversationId: string | null;
  profileId: string | null;
  runId: string | null;
  eventType: AuditEventType | string;
  payload: Record<string, unknown>;
  prevHash: string;
  hash: string;
}

export interface AuditPage {
  rows: AuditRow[];
  total: number;
}

/** Filters sent to `audit_query` / `audit_export` (camelCase, ISO times). */
export interface AuditQuery {
  types?: AuditEventType[];
  from?: string;
  to?: string;
  conversationId?: string;
  text?: string;
  limit?: number;
  offset?: number;
}

export interface VerifyReport {
  ok: boolean;
  checked: number;
  firstBadId: number | null;
  reason: string | null;
}

export interface MonthStats {
  questions: number;
  toolCalls: number;
  approvals: number;
  cloudInputTokens: number;
  cloudOutputTokens: number;
  cloudCostUsd: number;
}

export interface AuditStats {
  total: number;
  byType: Record<string, number>;
  oldest: string | null;
  newest: string | null;
  month: MonthStats;
  retentionDays: number;
  encrypted: boolean;
}

export const MIN_RETENTION_DAYS = 1;
export const MAX_RETENTION_DAYS = 3650;

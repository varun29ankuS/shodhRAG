import { invoke } from '@tauri-apps/api/core';

/** A coding tool Recall reads (`shodh_rag::recall::Tool`). */
export type RecallToolId = 'claude_code' | 'codex';

/** Who a turn is from (`shodh_rag::recall::Role`). */
export type RecallRole = 'user' | 'assistant' | 'tool_call' | 'tool_output';

/** The user's choices (`shodh_rag::recall::settings::RecallSettings`). */
export interface RecallSettings {
  enabled: boolean;
  tools: RecallToolId[];
  /** Project key → included (true) or excluded (false); a project not listed is not read. */
  projects: Record<string, boolean>;
  /** The user's own sensitive patterns (regular expressions). */
  patterns: string[];
  idleMinutes: number;
}

export interface RecallToolView {
  id: RecallToolId;
  label: string;
  folder: string | null;
  /** Transcript files there (counted by name, none read). */
  files: number;
  chosen: boolean;
}

export interface RecallProjectView {
  key: string;
  name: string;
  dir: string;
  /** null: not decided yet, so not read. */
  included: boolean | null;
  sessions: number;
  indexedSessions: number;
  tools: RecallToolId[];
  failed: number;
  lastActive: string | null;
}

export interface RecallRun {
  sourceId: string;
  done: number;
  total: number;
  current: string | null;
  paused: boolean;
}

export interface RecallLastSync {
  at: string;
  indexed: number;
  removed: number;
  failed: number;
  waiting: number;
  stopped: 'cancelled' | 'paused' | null;
}

/** `recall_commands::RecallStatus`. */
export interface RecallStatus {
  settings: RecallSettings;
  tools: RecallToolView[];
  projects: RecallProjectView[];
  indexedSessions: number;
  indexedTurns: number;
  secretsRedacted: number;
  unreadableLines: number;
  running: RecallRun | null;
  lastSync: RecallLastSync | null;
  error: string | null;
  searchModels: boolean;
}

export interface RecallReceipt {
  id: string;
  fileId: number;
  turn: number;
  role: RecallRole;
  toolName: string | null;
  at: string | null;
  heading: string;
  snippet: string;
}

export interface RecallSessionHit {
  tool: RecallToolId;
  sessionId: string;
  title: string;
  project: string;
  projectDir: string;
  branch: string | null;
  started: string | null;
  ended: string | null;
  turns: number;
  score: number;
  exact: string[];
  receipts: RecallReceipt[];
}

export interface RecallSearchAnswer {
  sessions: RecallSessionHit[];
  searched: number;
  tookMs: number;
}

export interface RecallSessionSummary {
  tool: RecallToolId;
  sessionId: string;
  title: string;
  project: string;
  projectDir: string;
  branch: string | null;
  started: string | null;
  ended: string | null;
  turns: number;
  threads: number;
  first: string | null;
  entities: string[];
}

export interface RecallScopeAnswer {
  sessions: RecallSessionSummary[];
  more: boolean;
}

export interface RecallOpenedTurn {
  turn: number;
  role: RecallRole;
  toolName: string | null;
  at: string | null;
  heading: string;
  text: string;
  leftOut: number;
  keptBack: string | null;
  line: number;
  target: boolean;
}

export interface RecallOpened {
  tool: RecallToolId;
  sessionId: string;
  title: string;
  project: string;
  projectDir: string;
  branch: string | null;
  fileId: number;
  path: string;
  turn: number;
  turnsTotal: number;
  turns: RecallOpenedTurn[];
  unreadableLines: number;
}

export interface RecallSearchRequest {
  query: string;
  tools?: RecallToolId[];
  projects?: string[];
  period?: string;
  since?: string;
  until?: string;
  entity?: string;
  limit?: number;
}

export interface RecallScopeRequest {
  period?: string;
  since?: string;
  until?: string;
  tools?: RecallToolId[];
  projects?: string[];
}

export const recallApi = {
  status: () => invoke<RecallStatus>('recall_status'),
  saveSettings: (settings: RecallSettings) => invoke<RecallStatus>('recall_save_settings', { settings }),
  syncNow: () => invoke<void>('recall_sync_now'),
  removeSession: (tool: RecallToolId, sessionId: string) =>
    invoke<RecallStatus>('recall_remove_session', { tool, sessionId }),
  search: (request: RecallSearchRequest) => invoke<RecallSearchAnswer>('recall_search', { request }),
  scope: (request: RecallScopeRequest) => invoke<RecallScopeAnswer>('recall_scope', { request }),
  open: (receipt: string, around?: number) => invoke<RecallOpened>('recall_open', { receipt, around }),
};

/** Emitted when Recall's state or progress changes (`recall_commands::RECALL_EVENT`). */
export const RECALL_EVENT = 'recall-status';

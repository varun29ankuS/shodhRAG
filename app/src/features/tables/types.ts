/**
 * Shapes of the evidence table commands (`app/src-tauri/src/table_commands.rs` and
 * `shodh_rag::research::tables`), as they serialise: camelCase fields, snake_case enum
 * values. `CellValue` is discriminated by `kind`; with `strict: false`, narrow it by that
 * string, never by truthiness.
 */

export type ColumnKind = 'text' | 'number' | 'yes_no' | 'choice';

export type CellStatus = 'empty' | 'queued' | 'verified' | 'unverified' | 'not_found' | 'edited' | 'failed';

export type CellValue =
  | { kind: 'text'; text: string }
  /** `lexeme` as printed; `decimal` without thousands separators, a Unicode minus as `-`. */
  | { kind: 'number'; lexeme: string; decimal: string; unit: string | null }
  | { kind: 'yes_no'; yes: boolean }
  | { kind: 'choice'; option: string };

/** A column as the user defines it (`ColumnSpec`). */
export interface ColumnSpec {
  question: string;
  kind: ColumnKind;
  /** 2–12 options for a `choice` column; none for the other kinds. */
  options: string[];
  instructions: string;
}

export interface EvidenceTable {
  id: string;
  workspaceId: string;
  name: string;
  createdAt: string;
  updatedAt: string;
}

/** A column (`Column`, its spec flattened). `revision` grows each time the spec's key changes. */
export interface Column extends ColumnSpec {
  id: string;
  tableId: string;
  position: number;
  key: string;
  revision: number;
  createdAt: string;
  updatedAt: string;
}

/** A row: a paper by its library identity (`path_key`: lower-cased on Windows). */
export interface TableRow {
  id: string;
  tableId: string;
  position: number;
  documentId: string;
  addedAt: string;
}

/** A row with the paper's title (`RowView`). */
export interface RowView extends TableRow {
  title: string;
}

export interface Cell {
  tableId: string;
  rowId: string;
  columnId: string;
  status: CellStatus;
  value: CellValue | null;
  quote: string | null;
  chunkId: string | null;
  page: number | null;
  /** The passage's boxes as the index stores them: a JSON array of `{page,x0,y0,x1,y1}`. */
  bboxes: string | null;
  model: string | null;
  columnRevision: number | null;
  paperHash: string | null;
  filledAt: string | null;
  editedAt: string | null;
  /** A reason code, `refill_failed:<code>` on a kept result, or `<code>: <detail>`. */
  error: string | null;
}

/** `table_get`. */
export interface TableView {
  table: EvidenceTable;
  columns: Column[];
  rows: RowView[];
  cells: Cell[];
}

export type JobState = 'queued' | 'running' | 'paused' | 'done' | 'cancelled' | 'failed';

export type PauseReason = 'daily_cap' | 'quota' | 'auth' | 'unreachable' | 'failing' | 'busy' | 'model_unavailable';

/** A fill job (`JobView`). Times are UTC Unix milliseconds. */
export interface JobView {
  id: string;
  tableId: string;
  state: JobState;
  plannedCalls: number;
  doneCalls: number;
  pauseReason: PauseReason | null;
  /**
   * What paused or stopped the job: a code, `search_failed: <detail>`, `fill_error: <detail>`,
   * for `quota` `model_quota:daily|short|unknown` (which limit it was), or for `model_unavailable` one of `refused_reasoning`, `local_only`, `no_key`, `no_model`.
   */
  lastError: string | null;
  /** When a paused job goes on by itself; null while it waits for the user. */
  nextAttemptAt: number | null;
  /** The running run's last heartbeat. */
  heartbeatAt: number | null;
  cancelRequested: boolean;
  createdAt: string;
}

/** `fill_status`. */
export interface FillStatus {
  job: JobView | null;
  papersDone: number;
  papersTotal: number;
  /** Table calls made today (UTC day), by every table. */
  callsToday: number;
  cap: number;
  /** The next 00:00 UTC (Unix milliseconds). */
  dayResetsAt: number;
  /**
   * Who runs a `running` job: this app, another run of Shodh under a live lease, or nobody
   * (the scheduler takes it up at its next check). Null when the job is not running.
   */
  run: 'here' | 'elsewhere' | 'stopped' | null;
  /** For `elsewhere`: when that run's lease ends at the latest, after which the fill goes on here. */
  leaseExpiresAt: number | null;
}

/** `fill_estimate`. */
export interface EstimateView {
  papers: number;
  columns: number;
  cells: number;
  calls: number;
  cached: number;
  callsToday: number;
  cap: number;
  dayResetsAt: number;
}

export type FillStartState = 'started' | 'already_running' | 'held_elsewhere' | 'at_cap';

/** `fill_start`. */
export interface FillStart {
  job: JobView;
  start: FillStartState;
  /** For `held_elsewhere`: when the earlier run's lease ends at the latest. */
  leaseExpiresAt: number | null;
  dayResetsAt: number;
}

/** `fill_cancel`. */
export interface FillCancel {
  job: JobView | null;
  /** Asked and not done yet: the job runs (it stops after its current call) or another run holds it. */
  pending: boolean;
  leaseExpiresAt: number | null;
}

/** What a fill by hand did (`HandFill`). */
export interface HandFill {
  cells: Cell[];
  calls: number;
  cacheHits: number;
  /** `daily_cap` when the day's cap stopped it before a call. */
  halted: string | null;
  /** The model's error code when a call failed (`model_quota`, `model_auth`, …). */
  modelError: string | null;
  /** What kept the whole paper from being filled (`paper_not_indexed`, …). */
  paperError: string | null;
}

/** `table_fill_paper` (the capture is kept in the database; its JSON is exported whole). */
export interface PaperCapture {
  capturedAt: string;
  tableId: string;
  rowId: string;
  documentId: string;
  model: string;
  reasoningRule: string;
  calls: unknown[];
  result: HandFill | null;
  error: string | null;
  captureId: string | null;
}

/** `tables_settings_get` / `tables_settings_set`. */
export interface TablesSettings {
  dailyCallCap: number;
  minDailyCallCap: number;
  maxDailyCallCap: number;
  /** `table_fill_paper` (keeping a fill's model calls) is allowed. */
  diagnostics: boolean;
}

/** A command error: `{ code, message }` with the code kept as sent. */
export interface TableFailure {
  code: string;
  message: string;
}

/** Emitted with `{ tableId }` after anything in a table changed and after every run of its fill. */
export const TABLE_CHANGED_EVENT = 'table_changed';

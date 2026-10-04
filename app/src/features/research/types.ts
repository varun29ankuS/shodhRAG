/**
 * Shapes of the research commands (snippets and Result statements) as the
 * backend returns them (camelCase JSON). Pure module (types and constants
 * only) so the logic around it is unit-tested with Node.
 *
 * Coordinates:
 * - `SnippetRect` is in PDF points from the top-left corner of the page's
 *   view box (pdf.js `page.view`), y growing downwards — the research
 *   ontology's `snippetRect`.
 * - `ResultRegion` is in PDF points with the origin at the bottom-left of the
 *   page (the indexer's convention, the same as citation `regions`).
 */

/** A rectangle on a page in PDF points, top-left origin. */
export interface SnippetRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** What a snippet shows, chosen by the user (or `passage` by default). */
export type SnippetKind = 'figure' | 'table' | 'equation' | 'passage';

export const SNIPPET_KINDS: readonly SnippetKind[] = ['figure', 'table', 'equation', 'passage'];

/** A saved page region (`Snippet` in Rust). */
export interface Snippet {
  /** Stable id (`snippet:<uuid>`); edits keep it. */
  id: string;
  /** Id of the current statement version. */
  statementId: string;
  filePath: string;
  fileName: string;
  /** 1-based page. */
  page: number;
  rect: SnippetRect;
  text: string;
  title: string;
  note: string;
  tags: string[];
  kind: SnippetKind;
  /** Whether the image is stored. Agent-made snippets get it on first display. */
  hasImage: boolean;
  /** LaTeX transcribed by a vision model, when the user asked for it. */
  latex: string | null;
  /** Model that transcribed `latex`. */
  latexModel: string | null;
  /** `global` or `workspace:<source id>`. */
  scope: string;
  createdAt: string;
  updatedAt: string;
}

/** Input of `snippets_create`. `imagePng` is base64 PNG without a data: prefix. */
export interface NewSnippetInput {
  filePath: string;
  page: number;
  rect: SnippetRect;
  text: string;
  imagePng: string | null;
  title?: string | null;
  note?: string | null;
  tags?: string[];
  kind?: SnippetKind;
  workspace?: string | null;
}

/** Input of `snippets_list`. */
export interface SnippetListQuery {
  /** Only snippets of this file. */
  filePath?: string | null;
  /** Words or meaning to search for (semantic + keyword). */
  text?: string | null;
  /** A workspace's snippets plus global ones; omitted means every snippet. */
  workspace?: string | null;
  limit?: number | null;
}

/** Input of `snippets_update`; omitted fields are kept. */
export interface SnippetPatch {
  title?: string | null;
  note?: string | null;
  tags?: string[] | null;
  kind?: SnippetKind | null;
}

/** Whether a vision-capable model can transcribe equations (`vision_capability`). */
export interface VisionCapability {
  available: boolean;
  /** The model that would be used, when known. */
  model: string | null;
  /** Why it is unavailable (shown on the disabled button). */
  reason: string | null;
}

/** Rows of the parser's table blocks overlapping a snippet (`snippets_table`). */
export interface SnippetTable {
  caption: string | null;
  header: string[];
  rows: string[][];
  page: number;
}

/** A region on a page, bottom-left origin (as citation `regions`). */
export interface ResultRegion {
  page: number;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

export type ResultExtractor = 'rule' | 'llm' | 'user';

/** One Result statement. */
export interface ResultRecord {
  id: string;
  /** Labels as printed in the paper. */
  method: string;
  dataset: string;
  metric: string;
  /** Canonical entity ids (`method:…`, `dataset:…`, `metric:…`), used to compare across papers. */
  methodId: string;
  datasetId: string;
  metricId: string;
  /** Canonical decimal (`95.3`); `valueText` is the cell exactly as printed. */
  value: string;
  valueText: string;
  unit: string | null;
  setting: string | null;
  filePath: string;
  fileName: string;
  page: number;
  /** The value's cell. */
  region: ResultRegion | null;
  tableCaption: string | null;
  extractor: ResultExtractor;
  confidence: number;
  /** `accepted` results are used; `review` ones wait for the user and are never used silently. */
  status: 'accepted' | 'review';
}

/** Why a table produced no results, per table. */
export interface SkippedTable {
  page: number;
  caption: string | null;
  reason: string;
}

/** What one extraction run did (`results_extract`), also stored per paper. */
export interface ExtractionReport {
  filePath: string;
  fileName: string;
  tables: number;
  added: number;
  unchanged: number;
  review: number;
  /** Values that conflicted with a result the user accepted (kept as is). */
  conflicts: number;
  skipped: SkippedTable[];
  /** Whether a language model was asked to interpret headers, and which. */
  model: string | null;
  extractedAt: string;
}

/** `results_list` for one paper. */
export interface PaperResults {
  results: ResultRecord[];
  review: ResultRecord[];
  report: ExtractionReport | null;
}

/** Filters of `results_query` (labels or canonical ids; case-insensitive). */
export interface ResultFilter {
  method?: string | null;
  dataset?: string | null;
  metric?: string | null;
  /** Only these papers (file paths). */
  papers?: string[] | null;
  workspace?: string | null;
}

/** One cell of a comparison: a value reported by one paper. */
export interface ComparisonCell {
  resultId: string;
  value: string;
  valueText: string;
  unit: string | null;
  filePath: string;
  fileName: string;
  page: number;
  region: ResultRegion | null;
  setting: string | null;
}

export interface ComparisonRow {
  method: string;
  methodId: string;
  /** Cells by column key (`ComparisonColumn.key`); several when papers disagree. */
  cells: Record<string, ComparisonCell[]>;
}

export interface ComparisonColumn {
  key: string;
  dataset: string;
  metric: string;
}

/** `results_query`: a method × (dataset, metric) table with coverage notes. */
export interface Comparison {
  columns: ComparisonColumn[];
  rows: ComparisonRow[];
  /** Papers that contributed cells. */
  papers: { filePath: string; fileName: string }[];
  /** Plain-language coverage notes, e.g. "3 papers report recall@10 on SIFT1M". */
  notes: string[];
  /** Results waiting for review that match the filter (excluded from the table). */
  pendingReview: number;
  truncated: boolean;
}

/** Values to choose from in the comparison builder (`results_facets`). */
export interface ResultFacets {
  methods: FacetValue[];
  datasets: FacetValue[];
  metrics: FacetValue[];
  papers: { filePath: string; fileName: string; results: number }[];
}

export interface FacetValue {
  id: string;
  label: string;
  count: number;
}

/** Tauri command names (kept in one place; the coverage manifest lists each). */
export const RESEARCH_COMMANDS = {
  snippetsCreate: 'snippets_create',
  snippetsList: 'snippets_list',
  snippetsGet: 'snippets_get',
  snippetsImage: 'snippets_image',
  snippetsSetImage: 'snippets_set_image',
  snippetsUpdate: 'snippets_update',
  snippetsDelete: 'snippets_delete',
  snippetsTable: 'snippets_table',
  snippetsTranscribe: 'snippets_transcribe_latex',
  visionCapability: 'vision_capability',
  resultsExtract: 'results_extract',
  resultsList: 'results_list',
  resultsReview: 'results_review',
  resultsQuery: 'results_query',
  resultsFacets: 'results_facets',
} as const;

/**
 * Command arguments (Tauri maps camelCase argument names to snake_case Rust
 * parameters):
 * - snippets_create({ input: NewSnippetInput }) → Snippet
 * - snippets_list({ query: SnippetListQuery }) → Snippet[]
 * - snippets_get({ id }) → Snippet
 * - snippets_image({ id }) → string | null (base64 PNG)
 * - snippets_set_image({ id, imagePng }) → Snippet
 * - snippets_update({ id, patch: SnippetPatch }) → Snippet
 * - snippets_delete({ id }) → void
 * - snippets_table({ id }) → SnippetTable | null
 * - snippets_transcribe_latex({ id }) → Snippet (latex + latexModel set)
 * - vision_capability() → VisionCapability
 * - results_extract({ filePath, workspace, useModel }) → ExtractionReport
 * - results_list({ filePath }) → PaperResults
 * - results_review({ id, accept }) → void
 * - results_query({ filter: ResultFilter }) → Comparison
 * - results_facets({ workspace }) → ResultFacets
 *
 * Errors are `{ code, message }` objects like the visual commands.
 * The backend emits `research-changed` ({ kind: 'snippet' | 'result', filePath })
 * after every write, including the agent's.
 */
export const RESEARCH_CHANGED_EVENT = 'research-changed';

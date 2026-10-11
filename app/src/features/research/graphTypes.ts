/**
 * Shapes of the citation graph commands (`paper_graph_*`, `paper_get`,
 * `papers_find`, `paper_concept`). Field names follow the backend's camelCase
 * serialization.
 */

import type { ResultRegion, ResultRecord } from './types.ts';

export interface PaperNode {
  /** `paper:arxiv:…`, `paper:doi:…`, `paper:openalex:…` or `paper:title:…`. */
  id: string;
  title: string | null;
  year: number | null;
  doi: string | null;
  arxivId: string | null;
  openalexId: string | null;
  /** The author list as printed. */
  authors: string | null;
  authorIds: string[];
  venue: string | null;
  inLibrary: boolean;
  /** The PDF, for a library paper. */
  filePath: string | null;
  citedByCount: number | null;
  statementId: string | null;
}

export interface CitationEvidence {
  text: string;
  page: number | null;
  regions: ResultRegion[];
}

export interface LinkedPaper {
  paper: PaperNode;
  evidence: CitationEvidence | null;
  citingFile: string | null;
}

export interface ConceptNode {
  id: string;
  label: string;
}

export interface PaperHit {
  paper: PaperNode;
  count: number;
}

export interface PaperDetail {
  paper: PaperNode;
  authors: { id: string; name: string }[];
  links: { doi: string | null; arxiv: string | null; openalex: string | null };
  citesInLibrary: LinkedPaper[];
  citesElsewhere: LinkedPaper[];
  citedByInLibrary: LinkedPaper[];
  related: PaperHit[];
  methods: ConceptNode[];
  datasets: ConceptNode[];
  proposes: ConceptNode[];
  results: ResultRecord[];
  snippets: unknown[];
}

export interface ConceptView {
  concept: ConceptNode;
  kind: 'method' | 'dataset';
  papers: PaperNode[];
  proposedIn: PaperNode | null;
}

export interface ViewNode {
  id: string;
  label: string;
  year: number | null;
  inLibrary: boolean;
  filePath: string | null;
  /** Library papers citing it. */
  libraryCiters: number;
  citedByCount: number | null;
  methods: string[];
}

export interface GraphViewData {
  nodes: ViewNode[];
  /** `[citing, cited]` node ids. */
  edges: [string, string][];
  methods: ConceptNode[];
  omitted: number;
}

export interface GraphSize {
  papers: number;
  libraryPapers: number;
  cites: number;
  authors: number;
  methods: number;
  datasets: number;
}

export interface BuildReport {
  files: number;
  parsed: number;
  reusedScans: number;
  failed: { filePath: string; reason: string }[];
  references: number;
  identifiableReferences: number;
  unidentifiedReferences: number;
  rejectedBlocks: Record<string, number>;
  libraryResolved: number;
  referencesResolved: number;
  referencesLinkedToLibrary: number;
  online: boolean;
  lookups: { requests: number; cacheHits: number; notFound: number; errors: number } | null;
  halted: string | null;
  added: number;
  updated: number;
  unchanged: number;
  removed: number;
  refused: number;
  size: GraphSize;
  builtAt: string;
}

export interface GraphStatus {
  report: BuildReport | null;
  size: GraphSize;
  onlineAllowed: boolean;
  onlineBlockedReason: string | null;
  building: boolean;
}

export type BuildProgress =
  | { stage: 'scanning'; done: number; total: number; file: string }
  | { stage: 'resolving'; done: number; total: number }
  | { stage: 'writing'; done: number; total: number };

/** What the Library's graph area shows. */
export type GraphPage =
  | { kind: 'graph' }
  | { kind: 'paper'; id: string }
  | { kind: 'method'; id: string }
  | { kind: 'dataset'; id: string };

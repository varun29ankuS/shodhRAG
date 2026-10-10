import type { PageSpan, RawSearchResult, SearchHit } from './types';
import { parseRegions } from './viewer/regionGeometry.ts';
import { citedNumbersIn } from '../agent/grounding.ts';

const SPAN_PATTERN = /^\s*(\d+)(?:\s*(?:[-–—]|to)\s*(\d+))?\s*$/i;

function positiveInt(value: number): number | null {
  return Number.isInteger(value) && value > 0 ? value : null;
}

/**
 * Parse a page reference. The backend sends `Option<String>` ("4", "4-5");
 * older persisted data may hold a plain number. Anything else yields null.
 */
export function parsePageSpan(value: unknown): PageSpan | null {
  if (typeof value === 'number') {
    const page = positiveInt(value);
    return page === null ? null : { start: page, end: page };
  }
  if (typeof value !== 'string') return null;
  const match = SPAN_PATTERN.exec(value);
  if (!match) return null;
  const start = positiveInt(Number(match[1]));
  if (start === null) return null;
  const end = match[2] !== undefined ? positiveInt(Number(match[2])) : start;
  if (end === null || end < start) return { start, end: start };
  return { start, end };
}

/**
 * Parse a line range. The backend sends `Option<String>` ("10-20"); older
 * persisted data may hold a `[start, end]` tuple. Anything else yields null.
 */
export function parseLineRange(value: unknown): [number, number] | null {
  if (Array.isArray(value)) {
    if (value.length !== 2) return null;
    const start = typeof value[0] === 'number' ? positiveInt(value[0]) : null;
    const end = typeof value[1] === 'number' ? positiveInt(value[1]) : null;
    if (start === null || end === null || end < start) return null;
    return [start, end];
  }
  const span = parsePageSpan(value);
  return span ? [span.start, span.end] : null;
}

export function fileNameOf(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

export function fileExtensionOf(path: string): string {
  const name = fileNameOf(path);
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
}

/** Name without extension, used for compact chips. */
export function fileStemOf(path: string): string {
  const name = fileNameOf(path);
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(0, dot) : name;
}

export function isWebUrl(path: string): boolean {
  return /^https?:\/\//i.test(path);
}

/**
 * A viewer target for a document the agent asked to show: the file at
 * `page`, with `passage` highlighted when given. Not a citation, so it has
 * no number.
 */
export function documentHit(path: string, page: number | null, passage: string | null): SearchHit {
  const name = path.split(/[\\/]/).filter(Boolean).pop() ?? path;
  const text = passage ?? '';
  return {
    number: 0,
    sourceFile: path,
    fileName: name,
    title: name,
    text,
    snippet: text.slice(0, 200),
    score: 0,
    page: page !== null && Number.isInteger(page) && page > 0 ? { start: page, end: page } : null,
    lineRange: null,
    url: isWebUrl(path) ? path : null,
  };
}

export type AppRecordKind = 'task' | 'event' | 'calendar' | 'note';

/** In-app records are indexed under pseudo-sources (`calendar://task/<id>`, `note://<id>`), not files. */
export function appRecordKind(path: string): AppRecordKind | null {
  const match = /^(calendar|note):\/\/([^/]*)/i.exec(path);
  if (!match) return null;
  if (match[1].toLowerCase() === 'note') return 'note';
  const sub = match[2].toLowerCase();
  return sub === 'task' || sub === 'event' ? sub : 'calendar';
}

const RECORD_NOUN: Record<AppRecordKind, string> = { task: 'Task', event: 'Event', calendar: 'Calendar', note: 'Note' };

/** "Task: File GST return"; never the record's id. */
export function recordLabel(kind: AppRecordKind, title: string | null | undefined): string {
  const t = title?.trim();
  return t ? `${RECORD_NOUN[kind]}: ${t}` : `${RECORD_NOUN[kind]} (untitled)`;
}

/** Compact chip label: the file name without extension, or the record label. */
/** "arxiv.org" for a web address (no "www."), or the input when it is not a URL. */
export function webHost(url: string): string {
  try {
    return new URL(url).host.replace(/^www\./, '');
  } catch {
    return url;
  }
}

export function sourceLabel(hit: Pick<SearchHit, 'sourceFile' | 'fileName'>): string {
  // Web sources: the page title the search returned, else the site.
  if (isWebUrl(hit.sourceFile)) return hit.fileName?.trim() || webHost(hit.sourceFile);
  const record = appRecordKind(hit.sourceFile);
  if (!record) return fileStemOf(hit.sourceFile);
  // Answers saved before records carried titles hold the bare id as the name.
  return hit.fileName && hit.fileName !== fileNameOf(hit.sourceFile) ? hit.fileName : recordLabel(record, null);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Normalise raw search results. Index i becomes citation number i + 1. */
export function toSearchHits(raw: readonly unknown[] | undefined): SearchHit[] {
  if (!raw) return [];
  const hits: SearchHit[] = [];
  raw.forEach((entry, index) => {
    if (!isRecord(entry)) return;
    const r = entry as RawSearchResult;
    const sourceFile = typeof r.sourceFile === 'string' ? r.sourceFile : '';
    if (!sourceFile) return;
    const citation = isRecord(r.citation) ? r.citation : null;
    const record = appRecordKind(sourceFile);
    const citationTitle = citation && typeof citation.title === 'string' ? citation.title : null;
    const fileName = record ? recordLabel(record, citationTitle) : fileNameOf(sourceFile);
    const text = typeof r.text === 'string' ? r.text : '';
    hits.push({
      number: index + 1,
      sourceFile,
      fileName,
      title: (citation && typeof citation.title === 'string' && citation.title.trim()) || fileName,
      text,
      snippet: typeof r.snippet === 'string' ? r.snippet : text.slice(0, 200),
      score: typeof r.score === 'number' ? r.score : 0,
      // The citation's label carries ranges ("4-5"); `pageNumber` only the first page.
      page: parsePageSpan(citation?.pageNumbers ?? null) ?? parsePageSpan(r.pageNumber ?? null),
      lineRange: parseLineRange(r.lineRange ?? null),
      url: citation && typeof citation.url === 'string' && citation.url ? citation.url : null,
      section: typeof r.metadata?.section_path === 'string' && r.metadata.section_path.trim() ? r.metadata.section_path.trim() : null,
      regions: parseRegions(r.metadata?.bboxes ?? null),
    });
  });
  return hits;
}

/** "p. 4", "pp. 4–5", "lines 10–20", or null when the location is unknown. */
export function formatLocation(hit: Pick<SearchHit, 'page' | 'lineRange'>): string | null {
  if (hit.page) {
    return hit.page.start === hit.page.end ? `p. ${hit.page.start}` : `pp. ${hit.page.start}–${hit.page.end}`;
  }
  if (hit.lineRange) {
    const [a, b] = hit.lineRange;
    return a === b ? `line ${a}` : `lines ${a}–${b}`;
  }
  return null;
}

/**
 * Citation numbers referenced in an answer, in the grammar the transcript
 * and the grounding check share (`[3]`, `[1, 4]`, `[2-4]`, `[Document 2]`,
 * `【5†…】`); code is ignored.
 */
export function citedNumbers(content: string): Set<number> {
  return citedNumbersIn(content);
}

export interface SourceGroup {
  sourceFile: string;
  fileName: string;
  /** Representative hit opened by the chip (first cited, else first). */
  primary: SearchHit;
  hits: SearchHit[];
  cited: boolean;
}

/** Unique files from the search results; files the answer cites come first. */
export function groupSources(hits: readonly SearchHit[], cited: ReadonlySet<number>): SourceGroup[] {
  const groups = new Map<string, SourceGroup>();
  for (const hit of hits) {
    const isCited = cited.has(hit.number);
    const existing = groups.get(hit.sourceFile);
    if (existing) {
      existing.hits.push(hit);
      if (isCited && !existing.cited) {
        existing.cited = true;
        existing.primary = hit;
      }
    } else {
      groups.set(hit.sourceFile, {
        sourceFile: hit.sourceFile,
        fileName: hit.fileName,
        primary: hit,
        hits: [hit],
        cited: isCited,
      });
    }
  }
  const all = [...groups.values()];
  return [...all.filter(g => g.cited), ...all.filter(g => !g.cited)];
}

/** Arguments for the `jump_to_source` command (integers only, or omitted). */
export function jumpToSourceArgs(hit: SearchHit): {
  filePath: string;
  lineNumber: number | null;
  pageNumber: number | null;
  searchText: string | null;
} {
  return {
    filePath: hit.sourceFile,
    lineNumber: hit.lineRange ? hit.lineRange[0] : null,
    pageNumber: hit.page ? hit.page.start : null,
    searchText: hit.snippet || null,
  };
}

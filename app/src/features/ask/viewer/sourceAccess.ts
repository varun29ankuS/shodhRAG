import { invoke } from '@tauri-apps/api/core';
import type { SheetData } from './tableMatch';

/** Mirrors `ViewerKind` in src-tauri/src/source_viewer_commands.rs. */
export type ViewerKind = 'pdf' | 'image' | 'table' | 'text' | 'unsupported';

/** Mirrors `SourceFileInfo`. */
export interface SourceFileInfo {
  path: string;
  fileName: string;
  folder: string | null;
  extension: string;
  sizeBytes: number;
  /** Last modification (ms since the epoch); null when the file system has none. */
  modifiedMs: number | null;
  kind: ViewerKind;
  mimeType: string | null;
}

/** Mirrors `shodh_rag::processing::pdf_info::PdfInfo`. */
export interface PdfInfo {
  title: string | null;
  pageCount: number;
  firstPageWidth: number | null;
  firstPageHeight: number | null;
}

/** Mirrors `SourceText`. */
export interface SourceText {
  text: string;
  truncated: boolean;
  totalChars: number;
}

/** Mirrors `SourceSheet`. */
export interface SourceSheet extends SheetData {
  totalRows: number;
  truncated: boolean;
}

export type SourceErrorKind =
  | 'notAFile'
  | 'indexUnavailable'
  | 'notIndexed'
  | 'notFound'
  | 'tooLarge'
  | 'unsupported'
  | 'readFailed'
  | 'unknown';

/** Error raised by the source viewer commands, normalised for display. */
export class SourceAccessError extends Error {
  readonly kind: SourceErrorKind;

  constructor(kind: SourceErrorKind, message: string) {
    super(message);
    this.name = 'SourceAccessError';
    this.kind = kind;
  }
}

const KNOWN_KINDS: readonly SourceErrorKind[] = [
  'notAFile',
  'indexUnavailable',
  'notIndexed',
  'notFound',
  'tooLarge',
  'unsupported',
  'readFailed',
];

/** Turn whatever an invoke rejected with into a `SourceAccessError`. */
export function toSourceError(error: unknown): SourceAccessError {
  if (error instanceof SourceAccessError) return error;
  if (typeof error === 'object' && error !== null) {
    const record = error as Record<string, unknown>;
    const kind = typeof record.kind === 'string' && (KNOWN_KINDS as readonly string[]).includes(record.kind)
      ? (record.kind as SourceErrorKind)
      : 'unknown';
    const message = typeof record.message === 'string' && record.message ? record.message : 'The file could not be read.';
    return new SourceAccessError(kind, message);
  }
  if (typeof error === 'string' && error) return new SourceAccessError('unknown', error);
  return new SourceAccessError('unknown', 'The file could not be read.');
}

async function call<T>(command: string, filePath: string): Promise<T> {
  try {
    return await invoke<T>(command, { filePath });
  } catch (error) {
    throw toSourceError(error);
  }
}

export function getSourceFileInfo(filePath: string): Promise<SourceFileInfo> {
  return call<SourceFileInfo>('get_source_file_info', filePath);
}

/** Title, page count and first page size of an indexed PDF, without opening it in pdf.js. */
export function getPdfInfo(filePath: string): Promise<PdfInfo> {
  return call<PdfInfo>('get_pdf_info', filePath);
}

/** Raw bytes of an indexed PDF or image (binary IPC payload). */
export async function readSourceBytes(filePath: string): Promise<Uint8Array> {
  const buffer = await call<ArrayBuffer>('read_source_bytes', filePath);
  return new Uint8Array(buffer);
}

export function readSourceText(filePath: string): Promise<SourceText> {
  return call<SourceText>('read_source_text', filePath);
}

export function readSourceTable(filePath: string): Promise<SourceSheet[]> {
  return call<SourceSheet[]>('read_source_table', filePath);
}

/** True when the user prefers reduced motion (instant scrolling). */
export function prefersReducedMotion(): boolean {
  return typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

export function scrollBehavior(): ScrollBehavior {
  return prefersReducedMotion() ? 'auto' : 'smooth';
}

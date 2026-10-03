/**
 * Remembered per-file state (where the reader was in a document, PDF
 * metadata) kept for the session in memory and persisted to Web Storage.
 * Storage may be missing, full, disabled or corrupt: every access is guarded
 * and the in-memory copy keeps working regardless.
 *
 * Pure module (storage is injected) so it is unit-tested directly with Node
 * (`app/tests/viewState.test.ts`).
 */

/** The subset of `Storage` used here. */
export interface KeyValueStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** Returns the storage to use, or null when none is available (may throw). */
export type StorageProvider = () => KeyValueStorage | null;

const FORMAT_VERSION = 1;

export interface PersistentStoreOptions<T> {
  /** Web Storage key holding the whole map. */
  storageKey: string;
  storage: StorageProvider;
  /** Turns a stored value back into a `T`, or null when it is not valid. */
  validate: (raw: unknown) => T | null;
  /** Entries kept (most recently written win). */
  maxEntries: number;
}

function safeStorage(provider: StorageProvider): KeyValueStorage | null {
  try {
    return provider();
  } catch {
    return null;
  }
}

/** Parse a stored map; anything malformed yields the valid entries only. */
export function parseStoredEntries<T>(text: string | null, validate: (raw: unknown) => T | null): Array<[string, T]> {
  if (!text) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return [];
  }
  if (typeof parsed !== 'object' || parsed === null) return [];
  const record = parsed as { v?: unknown; entries?: unknown };
  if (record.v !== FORMAT_VERSION || !Array.isArray(record.entries)) return [];
  const out: Array<[string, T]> = [];
  for (const item of record.entries) {
    if (!Array.isArray(item) || item.length !== 2 || typeof item[0] !== 'string' || !item[0]) continue;
    const value = validate(item[1]);
    if (value !== null) out.push([item[0], value]);
  }
  return out;
}

export function serializeEntries<T>(entries: Iterable<[string, T]>): string {
  return JSON.stringify({ v: FORMAT_VERSION, entries: [...entries] });
}

/**
 * A string-keyed map that remembers values for the session and mirrors them
 * to storage, keeping the `maxEntries` most recently written.
 */
export class PersistentStore<T> {
  private readonly options: PersistentStoreOptions<T>;
  /** Recency order: first = least recently written. */
  private readonly memory = new Map<string, T>();
  private loaded = false;

  constructor(options: PersistentStoreOptions<T>) {
    this.options = options;
  }

  get(key: string): T | null {
    this.load();
    return this.memory.get(key) ?? null;
  }

  /** Remember `value` for the session and persist it (best effort). */
  set(key: string, value: T): void {
    this.load();
    this.memory.delete(key);
    this.memory.set(key, value);
    while (this.memory.size > this.options.maxEntries) {
      const oldest = this.memory.keys().next().value as string;
      this.memory.delete(oldest);
    }
    this.persist();
  }

  private load(): void {
    if (this.loaded) return;
    this.loaded = true;
    const storage = safeStorage(this.options.storage);
    if (!storage) return;
    let text: string | null = null;
    try {
      text = storage.getItem(this.options.storageKey);
    } catch {
      return;
    }
    const entries = parseStoredEntries(text, this.options.validate);
    for (const [key, value] of entries.slice(-this.options.maxEntries)) {
      this.memory.delete(key);
      this.memory.set(key, value);
    }
  }

  private persist(): void {
    const storage = safeStorage(this.options.storage);
    if (!storage) return;
    try {
      storage.setItem(this.options.storageKey, serializeEntries(this.memory));
    } catch {
      // Quota exceeded or storage disabled: the session copy still works.
    }
  }
}

function finite(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

/** Zoom of a PDF: follow the panel width, or a fixed scale. */
export type PdfZoom = { mode: 'fit' } | { mode: 'manual'; scale: number };

/** Where the reader was in a PDF. */
export interface PdfViewState {
  /** 1-based page at the top of the viewport. */
  page: number;
  /** How far into that page the viewport's top edge is (0 = page top). */
  offset: number;
  zoom: PdfZoom;
}

export const MIN_PDF_SCALE = 0.25;
export const MAX_PDF_SCALE = 5;

export function parsePdfViewState(raw: unknown): PdfViewState | null {
  if (typeof raw !== 'object' || raw === null) return null;
  const r = raw as Record<string, unknown>;
  if (!finite(r.page) || !Number.isInteger(r.page) || r.page < 1) return null;
  if (!finite(r.offset)) return null;
  const zoomRaw = r.zoom as Record<string, unknown> | null | undefined;
  let zoom: PdfZoom;
  if (zoomRaw && zoomRaw.mode === 'fit') {
    zoom = { mode: 'fit' };
  } else if (zoomRaw && zoomRaw.mode === 'manual' && finite(zoomRaw.scale)) {
    zoom = { mode: 'manual', scale: Math.min(MAX_PDF_SCALE, Math.max(MIN_PDF_SCALE, zoomRaw.scale)) };
  } else {
    return null;
  }
  return { page: r.page, offset: Math.min(1, Math.max(0, r.offset)), zoom };
}

/** Where the reader was in a scrolling text document. */
export interface TextViewState {
  /** scrollTop / (scrollHeight - clientHeight), 0..1. */
  ratio: number;
}

export function parseTextViewState(raw: unknown): TextViewState | null {
  if (typeof raw !== 'object' || raw === null) return null;
  const ratio = (raw as Record<string, unknown>).ratio;
  if (!finite(ratio)) return null;
  return { ratio: Math.min(1, Math.max(0, ratio)) };
}

/** What the browser shows for a PDF in the file list and its skeleton. */
export interface PdfMeta {
  /** File size the metadata was read from (detects a changed file). */
  size: number;
  /** Document info Title, when present and plausible. */
  title: string | null;
  pages: number;
  /** First page size in PDF points (skeleton aspect ratio). */
  width: number;
  height: number;
}

export function parsePdfMeta(raw: unknown): PdfMeta | null {
  if (typeof raw !== 'object' || raw === null) return null;
  const r = raw as Record<string, unknown>;
  if (!finite(r.size) || r.size < 0) return null;
  if (!finite(r.pages) || !Number.isInteger(r.pages) || r.pages < 1) return null;
  if (!finite(r.width) || !finite(r.height) || r.width <= 0 || r.height <= 0) return null;
  if (r.title !== null && typeof r.title !== 'string') return null;
  return { size: r.size, title: r.title === null ? null : cleanPdfTitle(r.title), pages: r.pages, width: r.width, height: r.height };
}

/** Producer prefixes some tools stamp onto the Title field. */
const TITLE_PREFIXES = /^(microsoft (word|powerpoint|excel) - |untitled - )/i;

/**
 * The document info Title when it reads as a title, else null (the browser
 * then shows the file name). Rejects empty values, placeholders, and values
 * that are just a file name or path, which tools often write instead.
 */
export function cleanPdfTitle(raw: unknown): string | null {
  if (typeof raw !== 'string') return null;
  // Control characters (incl. stray NULs from UTF-16 decoding) out, spaces collapsed.
  const title = raw.replace(/[\u0000-\u001f\u007f]/g, ' ').replace(/\s+/g, ' ').trim().replace(TITLE_PREFIXES, '').trim();
  if (title.length < 3) return null;
  if (/^(untitled|title|document|no title|slide \d+|page \d+|abstract|\W+)$/i.test(title)) return null;
  if (/[\\/]/.test(title) && /\.[a-z0-9]{2,5}$/i.test(title)) return null;
  if (/^[^\s]+\.(pdf|docx?|dvi|tex|ps|eps|pptx?|rtf|txt|odt|indd|qxd|xml|html?)$/i.test(title)) return null;
  return title.length > 300 ? `${title.slice(0, 299)}…` : title;
}

/** Remembered size of a resizable panel, clamped; `fallback` when unset or invalid. */
export function readNumberPreference(storage: StorageProvider, key: string, fallback: number, min: number, max: number): number {
  const store = safeStorage(storage);
  if (!store) return fallback;
  let text: string | null = null;
  try {
    text = store.getItem(key);
  } catch {
    return fallback;
  }
  if (text === null || text.trim() === '') return fallback;
  const value = Number(text);
  if (!Number.isFinite(value)) return fallback;
  return Math.min(max, Math.max(min, value));
}

export function writeNumberPreference(storage: StorageProvider, key: string, value: number): void {
  const store = safeStorage(storage);
  if (!store || !Number.isFinite(value)) return;
  try {
    store.setItem(key, String(Math.round(value)));
  } catch {
    // Storage unavailable: the value lasts for this session only.
  }
}

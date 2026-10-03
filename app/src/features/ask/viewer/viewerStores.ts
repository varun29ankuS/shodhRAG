/**
 * App-wide remembered viewer state, persisted to localStorage: where the
 * reader was in each PDF and text document, and the PDF metadata the file
 * browser shows (title, page count, first-page size for skeletons).
 */
import { pathKey } from '../../library/fileTree';
import {
  parsePdfMeta,
  parsePdfViewState,
  parseTextViewState,
  PersistentStore,
  type PdfMeta,
  type PdfViewState,
  type StorageProvider,
  type TextViewState,
} from './viewState';

export const browserStorage: StorageProvider = () => (typeof window === 'undefined' ? null : window.localStorage);

/** Keyed by `pathKey(path)`. */
export const pdfViewStates = new PersistentStore<PdfViewState>({
  storageKey: 'shodh.viewer.pdf.v1',
  storage: browserStorage,
  validate: parsePdfViewState,
  maxEntries: 300,
});

/** Keyed by `pathKey(path)`. */
export const textViewStates = new PersistentStore<TextViewState>({
  storageKey: 'shodh.viewer.text.v1',
  storage: browserStorage,
  validate: parseTextViewState,
  maxEntries: 300,
});

const pdfMetaStore = new PersistentStore<PdfMeta>({
  storageKey: 'shodh.library.pdfMeta.v1',
  storage: browserStorage,
  validate: parsePdfMeta,
  maxEntries: 2000,
});

const metaListeners = new Set<() => void>();

/** Remembered metadata of a PDF, whatever size it was read at. */
export function getPdfMeta(path: string): PdfMeta | null {
  return pdfMetaStore.get(pathKey(path));
}

/** Remembered metadata of a PDF, only if read from a file of `size` bytes. */
export function getPdfMetaForSize(path: string, size: number): PdfMeta | null {
  const meta = getPdfMeta(path);
  return meta && meta.size === size ? meta : null;
}

export function rememberPdfMeta(path: string, meta: PdfMeta): void {
  const previous = getPdfMeta(path);
  if (
    previous &&
    previous.size === meta.size &&
    previous.title === meta.title &&
    previous.pages === meta.pages &&
    previous.width === meta.width &&
    previous.height === meta.height
  ) {
    return;
  }
  pdfMetaStore.set(pathKey(path), meta);
  for (const listener of [...metaListeners]) listener();
}

/** Called whenever any remembered PDF metadata changes. */
export function subscribePdfMeta(listener: () => void): () => void {
  metaListeners.add(listener);
  return () => {
    metaListeners.delete(listener);
  };
}

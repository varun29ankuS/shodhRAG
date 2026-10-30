/**
 * App-wide requests around snippets and source boxes, so low-level views
 * (the PDF viewer, Library tables, toasts) need no providers: open a
 * snippet (focus pop-out, or a dialog when no conversation is open), show a
 * place in a paper with its box outlined, and insert context into the Ask
 * composer. `SnippetHost` and `AskView` listen.
 */

import type { ResultRegion, Snippet, SnippetRect } from './types';
import type { FileRef } from '../ask/fileQuestion';

export const OPEN_SNIPPET_EVENT = 'shodh:open-snippet';
export const SHOW_SOURCE_EVENT = 'shodh:show-source-box';
export const COMPOSER_INSERT_EVENT = 'shodh:composer-insert';

/** A place in a paper to show with its box outlined. */
export interface SourceBoxRequest {
  filePath: string;
  fileName: string;
  page: number;
  /** Indexer boxes (bottom-left origin). */
  regions?: ResultRegion[] | null;
  /** Snippet rectangles (top-left origin of the view box). */
  rects?: { page: number; rect: SnippetRect }[] | null;
  /** What the box is (for the dialog title), e.g. "HNSW · recall@10". */
  label?: string | null;
  /**
   * Set by a view that shows the file itself (the Library's open file), so
   * the app-wide dialog does not open as well.
   */
  claimed?: boolean;
}

export function openSnippet(snippet: Snippet): void {
  window.dispatchEvent(new CustomEvent<Snippet>(OPEN_SNIPPET_EVENT, { detail: snippet }));
}

export function showSourceBox(request: SourceBoxRequest): void {
  window.dispatchEvent(new CustomEvent<SourceBoxRequest>(SHOW_SOURCE_EVENT, { detail: request }));
}

/**
 * Text for the composer, and the file the next question is about when the
 * text asks about one: the file goes with the question as its search limit,
 * never in its text (`features/ask/fileQuestion`).
 */
export interface ComposerInsert {
  text: string;
  file?: FileRef;
}

/** Inserts not yet taken by a composer (Ask may mount after the request). */
let pendingInserts: ComposerInsert[] = [];

/**
 * Append `text` to the Ask composer's draft and show Ask. Kept until the
 * composer takes it, so it is not lost while Ask mounts.
 */
export function insertIntoComposer(text: string, file?: FileRef): void {
  const insert: ComposerInsert = file ? { text, file } : { text };
  pendingInserts.push(insert);
  window.dispatchEvent(new CustomEvent<ComposerInsert>(COMPOSER_INSERT_EVENT, { detail: insert }));
  window.dispatchEvent(new CustomEvent('switchTab', { detail: 'ask' }));
}

/** The pending inserts, oldest first, now owned by the caller. */
export function takePendingInserts(): ComposerInsert[] {
  const taken = pendingInserts;
  pendingInserts = [];
  return taken;
}

export function onWindowEvent<T>(name: string, handler: (detail: T) => void): () => void {
  const listener = (event: Event) => handler((event as CustomEvent<T>).detail);
  window.addEventListener(name, listener);
  return () => window.removeEventListener(name, listener);
}

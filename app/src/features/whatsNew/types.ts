/**
 * What's new as the backend sends it (`app/src-tauri/src/whats_new.rs`, camelCase).
 * Types only, so the pure model and Node's strip-types tests import it freely.
 */

export type FeedKind = 'file' | 'paper';

/** One row: a file new in a watched folder. */
export interface FeedItem {
  id: string;
  kind: FeedKind;
  /** The file's index identity. */
  sourceRef: string;
  /** The folder watch that found it. */
  jobId: string;
  /** The file name. */
  title: string;
  /** The folder's name. */
  detail: string | null;
  /** When it was found (UTC, RFC 3339). */
  foundAt: string;
  postedRunId: string | null;
  seenAt: string | null;
  dismissedAt: string | null;
  link: unknown;
  /** `{ sourceId, path, folder, more? }` for a file. */
  data: Record<string, unknown>;
}

/** A watched folder and whether its new files show in What's new. */
export interface FolderWatchView {
  sourceId: string;
  folder: string;
  on: boolean;
  /** Stopped after failing again and again; Resume turns it on again. */
  held: boolean;
}

export interface WhatsNewView {
  items: FeedItem[];
  folders: FolderWatchView[];
}

/**
 * What's new as the backend sends it (`app/src-tauri/src/whats_new.rs`, camelCase).
 * Types only, so the pure model and Node's strip-types tests import it freely.
 */

export type FeedKind = 'file' | 'paper';

/** One row: a file new in a watched folder, or a paper a paper alert found. */
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
  /**
   * A file: `{ sourceId, path, folder }`. A paper: `{ doi, arxivId, year, authors,
   * venue, abstract, pdfUrl, landingUrl, sources, alertId, words, addedPath? }`.
   */
  data: Record<string, unknown>;
}

/** A watched folder and whether its new files show in What's new. */
export interface FolderWatchView {
  sourceId: string;
  folder: string;
  on: boolean;
  /** Stopped after failing again and again; Resume turns it on again. */
  held: boolean;
  /** Files found beyond the listed ones, by local day (`yyyy-mm-dd`). */
  moreByDay: Record<string, number>;
}

export type AlertState = 'checking' | 'waiting_retry' | 'offline' | 'active' | 'off' | 'held' | 'blocked';

/** A paper alert (`app/src-tauri/src/paper_alerts.rs`). */
export interface AlertView {
  id: string;
  /** The exact words sent. */
  words: string;
  consentText: string;
  state: AlertState;
  lastCheckedAt: string | null;
  lastSummary: string | null;
  nextCheckAt: string | null;
  /** APIs that did not answer in the current check. */
  failedApis: string[];
  retryAt: string | null;
  blockedReason: string | null;
}

/** A topic suggested from the library: the exact words an alert on it sends. */
export interface TopicSuggestion {
  words: string;
  from: 'titles' | 'method';
  papers: number;
}

export interface WhatsNewView {
  items: FeedItem[];
  folders: FolderWatchView[];
  alerts: AlertView[];
  /** Why paper alerts send nothing now (Local-only mode, web access off). */
  blockedReason: string | null;
}

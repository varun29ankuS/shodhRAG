/**
 * Types for the Ask view and the chat session.
 *
 * - `TranscriptState` (features/agent/reducer) is the record of an answer
 *   produced by an agent session, built from `agent_event`s.
 * - `ResponseMetadata`, `RawSearchResult` and `RunRecord` describe answers
 *   stored by the earlier chat pipeline; they are kept so existing
 *   conversations still render.
 */

import type { TranscriptState } from '../agent/reducer';
import type { FocusThread } from '../focus/focusTypes';
import type { SideSummaryRef } from '../focus/summary';

export type ChatRole = 'user' | 'assistant' | 'system';

export interface ResponseMetadata {
  model?: string | null;
  inputTokens?: number | null;
  outputTokens?: number | null;
  durationMs?: number | null;
  intent?: string;
  routerTokens?: number;
  routerLatencyMs?: number;
  searchQueriesUsed?: string[];
  rerankLatencyMs?: number;
}

export interface RawCitation {
  title?: string;
  snippet?: string;
  score?: number;
  url?: string | null;
  authors?: string[];
  source?: string;
  year?: string;
  pageNumbers?: string | null;
}

/** Search result exactly as the backend serializes it. */
export interface RawSearchResult {
  text?: string;
  score?: number;
  citation?: RawCitation | null;
  sourceFile?: string;
  /** Rust: `Option<String>` such as "4" or "4-5". Older data may hold a number. */
  pageNumber?: string | number | null;
  /** Rust: `Option<String>` such as "10-20". Older data may hold a tuple. */
  lineRange?: string | [number, number] | null;
  snippet?: string;
  metadata?: Record<string, string>;
}

export interface PageSpan {
  start: number;
  end: number;
}

/** A search result normalised at the frontend boundary. */
export interface SearchHit {
  /** 1-based position; matches the `[N]` markers the model writes. */
  number: number;
  sourceFile: string;
  fileName: string;
  title: string;
  text: string;
  snippet: string;
  score: number;
  page: PageSpan | null;
  lineRange: [number, number] | null;
  url: string | null;
}

export type RunStatus = 'running' | 'done' | 'failed' | 'cancelled';

export type RunStepKind = 'search' | 'tool' | 'thinking';

export interface RunStep {
  id: string;
  kind: RunStepKind;
  /** Short title, e.g. "Searched", a tool name, "Thinking". */
  title: string;
  /** Real detail from the event: queries, tool arguments, model message. */
  detail?: string;
  /** 'stopped': the request was cancelled before this step reported completion. */
  status: 'running' | 'done' | 'failed' | 'stopped';
  /** Duration reported by the backend for this step, when it reports one. */
  durationMs?: number;
  /** Small right-aligned annotation, e.g. "12 passages". */
  meta?: string;
}

export interface RunRecord {
  status: RunStatus;
  /** ISO timestamp when the request was sent. */
  startedAt: string;
  /** Wall-clock time from send until the response (or cancel/failure). */
  elapsedMs?: number;
  steps: RunStep[];
  /** Latest human-readable activity while running (from real events only). */
  activity?: string;
  error?: string;
}

export interface ChatMessage {
  id: string;
  role: ChatRole;
  content: string;
  timestamp: string;
  searchResults?: RawSearchResult[];
  artifacts?: any[];
  metadata?: ResponseMetadata;
  /** Legacy (pre-agent) run record of older answers. */
  run?: RunRecord;
  /** Agent transcript of the answer: steps, task list, usage, passages. */
  transcript?: TranscriptState;
  /** Base64 image attached to OCR notices. Not persisted. */
  image?: string;
  /**
   * Side threads ("Ask about this") anchored to this message. Persisted in
   * the message's opaque `metadata` under `focusThreads`.
   */
  threads?: FocusThread[];
  /**
   * A user message posted from a side discussion ("Add to main
   * conversation"): rendered as a card that reopens the discussion.
   * Persisted in `metadata` under `focusSummary`.
   */
  sideSummary?: SideSummaryRef;
}

/** Options that scope a request; supplied by the caller at send time. */
export interface SendOptions {
  spaceId: string | null;
  spaceName: string | null;
  /** Sources the answer may search; empty or absent means every source. */
  sourceIds?: string[];
  /** Files the answer is about ("Ask about this file"); limits search to them. */
  sourceFiles?: string[];
}

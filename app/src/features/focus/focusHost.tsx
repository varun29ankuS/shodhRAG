/**
 * The focus pop-out's interface, apart from the provider that runs it (`FocusContext.tsx`):
 * the types, the React contexts and the hooks that read them. Components inside an answer
 * (visuals, tables, the transcript) read focus through these hooks only, so they render
 * wherever the answer does: in Ask under the provider, and in the floating chat over the
 * embedded preview (`features/dock`), where no provider runs and every hook gives null,
 * exactly as in the in-page chat, which has no answer anchor.
 */
import React, { createContext, useContext, useMemo } from 'react';
import { notify } from '../../lib/notify';
import type { ClaimCheck } from '../agent/events';
import type { CorrectionOutcome } from '../agent/grounding';
import type { TranscriptState } from '../agent/reducer';
import type { ContinuedAnswer } from './carriedPassages';
import type { FocusExtras, FocusTarget, FocusThread } from './focusTypes';
import type { StackState } from './focusStack';
import type { SummaryDestination } from './summary';
import type { RefineResult } from '../visuals/refine';
import type { VisualRecord, VisualRef } from '../visuals/model';

/** What to show in the pop-out. */
export interface FocusRequest {
  target: FocusTarget;
  /** Conversation the side thread belongs to; the active one when omitted. */
  conversationId?: string | null;
  /** Message the object came from; null for objects outside the conversation. */
  parentMessageId: string | null;
  /** Reopen this thread (from a "replies" chip); nested threads open with their outer levels. */
  threadId?: string | null;
  /** Element focus returns to when the pop-out closes. */
  trigger?: HTMLElement | null;
  /** The gallery visual shown (version switcher, refine, export). */
  visual?: VisualRef | null;
  /** A prepared question the new level sends once (a "go deeper" mark). */
  question?: string | null;
}

/** An object opened inside the current level (a visual or a selection in a side answer). */
export interface DrillRequest {
  target: FocusTarget;
  /** Thread of the level it was found in. */
  parentThreadId: string;
  /** Answer it was found in; null when found in the level's object itself. */
  parentTurnId: string | null;
  /** A prepared question the new level sends once (a "go deeper" mark). */
  question?: string | null;
}

/** "opened", "depth" when the depth cap is reached, "closed" when no pop-out is open. */
export type DrillResult = 'opened' | 'depth' | 'closed';

/** One level of the pop-out: the object, resolved to a conversation and thread. */
export interface OpenFocus {
  target: FocusTarget;
  conversationId: string;
  parentMessageId: string | null;
  threadId: string;
  /** Thread this level was opened from (null at the first level). */
  parentThreadId: string | null;
  /** Answer of the parent thread it was opened from. */
  parentTurnId: string | null;
  trigger: HTMLElement | null;
  /** Unique per level, so a reopened object remounts its state. */
  seq: number;
  /** The gallery visual this level shows, when it was opened from the gallery. */
  visual: VisualRef | null;
  /** A prepared question this level sends once, unless its thread already asked it. */
  question: string | null;
}

/** The pop-out as opened: its levels. `key` stays while levels change. */
export interface FocusSession {
  key: number;
  stack: StackState<OpenFocus>;
}

export type FocusNavigation = { type: 'back' } | { type: 'forward' } | { type: 'jump'; index: number };

export type SummaryResult =
  | { ok: true; text: string }
  | { ok: false; reason: 'busy' | 'stopped' | 'failed'; message?: string };

export type SidePurpose = 'answer' | 'summary' | 'refine' | 'repair';

/** What a refine run produced. */
export type RefineOutcome =
  | RefineResult
  | { ok: false; reason: 'busy' | 'stopped' | 'failed'; message: string };

/** Where a diagram that did not draw was found: a main answer, or an answer in a side thread. */
export type DiagramPlace =
  | { kind: 'answer'; conversationId: string; messageId: string }
  | { kind: 'side'; parentThreadId: string; parentTurnId: string };

/** A request for the model to rework a diagram: fix one that did not draw, adjust one, or write it as a list. */
export interface DiagramRepairRequest {
  place: DiagramPlace;
  /** Identifies the diagram (a hash of its source as written); one run per key at a time. */
  key: string;
  /** The diagram the request is about. */
  source: string;
  /** The message sent (source, the reader's note, the answer's context and the reply format asked for). */
  message: string;
  /** What the run is doing, as the app names a running answer (e.g. "adjusting a diagram"). */
  label: string;
}

/** The model's reply, or why there is none. `busy`: another answer holds the agent, nothing was sent. */
export type DiagramRepairOutcome =
  | { ok: true; reply: string }
  | { ok: false; reason: 'busy' | 'stopped' | 'failed'; message: string };


export interface SideLiveView {
  threadId: string;
  purpose: SidePurpose;
  transcript: TranscriptState;
}

export interface FocusContextValue {
  openFocus: (request: FocusRequest) => void;
  closeFocus: () => void;
  /** Open an object found inside the current level as a new level. */
  drillDown: (request: DrillRequest) => DrillResult;
  /** Move between the pop-out's levels. */
  navigate: (move: FocusNavigation) => void;
  /** Show a thread of the open pop-out's answer, with its outer levels (exploration map). */
  jumpToThread: (threadId: string) => void;
  /** The open pop-out's levels, or null. */
  session: FocusSession | null;
  /**
   * Threads with no parent message (about a task, or a document the agent
   * opened), with their nested threads, stored on the conversation.
   */
  localThreads: (conversationId: string) => FocusThread[];
  /** Every thread kept with a message (or on the conversation when `parentMessageId` is null). */
  locationThreads: (conversationId: string, parentMessageId: string | null) => FocusThread[];
  /** A thread by id, wherever it is kept. */
  findThread: (conversationId: string, parentMessageId: string | null, threadId: string) => FocusThread | null;
  /** Ask a side question. False when it could not start (another answer is running). */
  ask: (open: OpenFocus, question: string, extras: FocusExtras) => Promise<boolean>;
  /** Ask the agent for a summary of a level's discussion (not added to the thread). */
  summarize: (open: OpenFocus, destination: SummaryDestination) => Promise<SummaryResult>;
  /** Post a summary of a nested level into its parent discussion and go up to it. */
  bringBack: (open: OpenFocus, text: string) => boolean;
  /**
   * Ask the agent for a revised version of a gallery visual (not added to the
   * discussion). Resolves to the revised source or why there is none.
   */
  refine: (open: OpenFocus, instruction: string) => Promise<RefineOutcome>;
  /** Show another version of the gallery visual as the pop-out's only level. */
  showVisualVersion: (record: VisualRecord) => void;
  /**
   * Ask the model, in a session of its own, for a corrected version of a
   * diagram that did not draw. Never enters the conversation or a thread;
   * resolves `busy` without sending anything when another answer is running.
   */
  repairDiagram: (request: DiagramRepairRequest) => Promise<DiagramRepairOutcome>;
  /** Whether the answer at `place` is the newest one where it is shown (read on call, no re-render). */
  isLatestAnswer: (place: DiagramPlace) => boolean;
  /**
   * The stored answer at `place` (its text and the passages of its run),
   * which a diagram rework carries instead of searching again; null when it
   * is not stored or no longer open.
   */
  answerContext: (place: DiagramPlace) => ContinuedAnswer | null;
  /**
   * "Cite N instead" on a miscited claim of a stored side answer: records the
   * correction in the audit log, then rewrites that answer's citation and
   * saves the thread (audit first: nothing changes without the record).
   */
  citeInstead: (open: OpenFocus, turnId: string, check: ClaimCheck) => Promise<CorrectionOutcome>;
  /** Interrupt the side answer. */
  stop: () => void;
  /** Answer a pending approval of the side answer (`forConversation`: allow what it offered for the session). */
  approve: (stepId: string, approved: boolean, forConversation?: boolean, reviewToken?: string) => void;
  /** The side answer being produced, for rendering. */
  sideLive: SideLiveView | null;
}

export const FocusContext = createContext<FocusContextValue | null>(null);

/** The pop-out API, or null outside the provider. */
export function useFocus(): FocusContextValue | null {
  return useContext(FocusContext);
}

/**
 * Where visuals inside an answer come from. Provided around each answer in
 * the Ask view; visuals show their focus affordance only inside it (or
 * inside a side answer, see `FocusDrillProvider`).
 */
export interface FocusAnchorValue {
  conversationId: string;
  messageId: string;
}

const FocusAnchorContext = createContext<FocusAnchorValue | null>(null);

/**
 * Anchors visuals to an answer. Always rendered (toggle `enabled`), so an
 * answer that finishes streaming keeps its subtree instead of remounting.
 */
export function FocusAnchorProvider({
  conversationId,
  messageId,
  enabled,
  children,
}: { conversationId: string | null; messageId: string; enabled: boolean; children: React.ReactNode }) {
  const value = useMemo(
    () => (enabled && conversationId ? { conversationId, messageId } : null),
    [enabled, conversationId, messageId],
  );
  return <FocusAnchorContext.Provider value={value}>{children}</FocusAnchorContext.Provider>;
}

export function useFocusAnchor(): FocusAnchorValue | null {
  return useContext(FocusAnchorContext);
}

/** A finished side answer, so visuals inside it open as a nested level. */
export interface FocusDrillValue {
  parentThreadId: string;
  parentTurnId: string;
}

const FocusDrillContext = createContext<FocusDrillValue | null>(null);

export function FocusDrillProvider({ value, children }: { value: FocusDrillValue | null; children: React.ReactNode }) {
  return <FocusDrillContext.Provider value={value}>{children}</FocusDrillContext.Provider>;
}

export function useFocusDrill(): FocusDrillValue | null {
  return useContext(FocusDrillContext);
}

/** The message shown when the depth cap is reached. */
export function notifyDepthLimit(): void {
  notify.info('This is as deep as the pop-out goes', {
    description: 'Ask about it here, or go up a level (Alt+←) and open it from there.',
  });
}

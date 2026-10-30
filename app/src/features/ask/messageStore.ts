/**
 * The chat provider's pure parts (`ChatSessionContext`): how a message is stored and
 * restored, how a question and a new answer are built, what a retry asks with, and the
 * search limit sent with a question. No React and no Tauri (type imports only besides
 * `.ts` modules), so it is unit-tested under Node (`app/tests/messageStore.test.ts`).
 */

import type { ConversationMessage } from '../../hooks/useConversations';
import type { AnswerScope } from '../agent/useAgentSession';
import { fromPersisted, initialTranscript, isLive, toCheckpoint, toPersisted } from '../agent/reducer.ts';
import type { TranscriptState } from '../agent/reducer';
import { metadataWithThreads, metadataWithoutThreads, threadsFromMetadata } from '../focus/threadStore.ts';
import { metadataWithSummary, readSideSummary } from '../focus/summary.ts';
import type { SideSummaryRef } from '../focus/summary';
import { answerScope } from '../workspaces/model.ts';
import { metadataWithAttachedContext, readAttachedContext } from './imageAttachments.ts';
import type { AttachedContext } from './imageAttachments';
import { cleanFiles, limitedToFiles, metadataWithAboutFiles, readAboutFiles } from './fileQuestion.ts';
import type {
  ChatMessage,
  RawSearchResult,
  ResponseMetadata,
  RunRecord,
  RunStatus,
  RunStep,
  SendOptions,
} from './types';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isRunStatus(value: unknown): value is RunStatus {
  return value === 'running' || value === 'done' || value === 'failed' || value === 'cancelled';
}

/** Legacy run record of answers produced before the agent harness. */
function readRun(value: unknown): RunRecord | undefined {
  if (!isRecord(value) || !isRunStatus(value.status) || typeof value.startedAt !== 'string') return undefined;
  const steps = Array.isArray(value.steps) ? (value.steps.filter(isRecord) as unknown as RunStep[]) : [];
  // A run persisted while "running" can only come from an interrupted session.
  const status: RunStatus = value.status === 'running' ? 'cancelled' : value.status;
  return {
    status,
    startedAt: value.startedAt,
    elapsedMs: typeof value.elapsedMs === 'number' ? value.elapsedMs : undefined,
    steps: steps.map(s => (s.status === 'running' ? { ...s, status: 'stopped' as const } : s)),
    error: typeof value.error === 'string' ? value.error : undefined,
  };
}

/** A stored message as the chat shows it. */
export function fromStored(m: ConversationMessage): ChatMessage {
  const metadata = metadataWithAboutFiles(
    metadataWithAttachedContext(metadataWithSummary(metadataWithoutThreads(m.metadata), null), []),
    [],
  );
  const threads = threadsFromMetadata(m.metadata);
  const sideSummary = m.role === 'user' ? readSideSummary(m.metadata) : null;
  const attachedContext = m.role === 'user' ? readAttachedContext(m.metadata) : [];
  const aboutFiles = m.role === 'user' ? readAboutFiles(m.metadata) : [];
  return {
    id: m.id,
    role: m.role,
    content: m.content,
    timestamp: m.timestamp,
    artifacts: m.artifacts,
    searchResults: m.searchResults as RawSearchResult[] | undefined,
    metadata: metadata ? (metadata as ResponseMetadata) : undefined,
    run: readRun(m.run),
    transcript: fromPersisted(m.transcript) ?? undefined,
    threads: threads.length > 0 ? threads : undefined,
    sideSummary: sideSummary ?? undefined,
    attachedContext: attachedContext.length > 0 ? attachedContext : undefined,
    aboutFiles: aboutFiles.length > 0 ? aboutFiles : undefined,
  };
}

/** A message as it is stored with its conversation. */
export function toStored(m: ChatMessage): ConversationMessage {
  const stored: ConversationMessage = {
    id: m.id,
    role: m.role,
    content: m.content,
    timestamp: m.timestamp,
  };
  if (m.artifacts && m.artifacts.length > 0) stored.artifacts = m.artifacts;
  if (m.searchResults && m.searchResults.length > 0) stored.searchResults = m.searchResults;
  const metadata = metadataWithAboutFiles(
    metadataWithAttachedContext(
      metadataWithThreads(metadataWithSummary(m.metadata, m.sideSummary ?? null), m.threads ?? []),
      m.attachedContext ?? [],
    ),
    m.aboutFiles ?? [],
  );
  if (metadata) stored.metadata = metadata;
  if (m.run) {
    const { activity: _activity, ...persistable } = m.run;
    stored.run = persistable as unknown as Record<string, unknown>;
  }
  if (m.transcript) stored.transcript = toPersisted(m.transcript) as unknown as Record<string, unknown>;
  return stored;
}

/** Whether a message is settled (saved as itself, not as a checkpoint). */
export function isPersistable(m: ChatMessage): boolean {
  if (m.run?.status === 'running') return false;
  return !(m.transcript && isLive(m.transcript));
}

/**
 * Stored form of a running answer whose text is complete (its check against
 * the sources has started): kept if the app closes before the run ends.
 */
export function toStoredCheckpoint(m: ChatMessage): ConversationMessage {
  return toStored(m.transcript ? { ...m, transcript: toCheckpoint(m.transcript) } : m);
}

/**
 * The answer's search limit from the send options and the conversation's workspace (whose
 * sources the backend limits search to, unless "search all my library" is on); null when
 * neither applies.
 */
export function scopeOf(options: SendOptions | null, workspaceId: string | null): AnswerScope | null {
  const sourceIds = options?.sourceIds?.filter(id => id.trim().length > 0) ?? [];
  const sourceFiles = options?.sourceFiles?.filter(f => f.trim().length > 0) ?? [];
  const limit = sourceIds.length === 0 && sourceFiles.length === 0 ? null : { sourceIds, sourceFiles };
  return answerScope(limit, workspaceId, options?.searchAll === true);
}

/** What a question carries besides its words. */
export interface QuestionExtras {
  sideSummary?: SideSummaryRef;
  attachedContext?: readonly AttachedContext[];
}

/**
 * The user message of a question. The files it is about (`options.sourceFiles`) stay
 * with it, so a retry asks about them again.
 */
export function questionMessage(
  id: string,
  content: string,
  timestamp: string,
  options: SendOptions,
  extras: QuestionExtras = {},
): ChatMessage {
  const attachedContext = (extras.attachedContext ?? []).filter(a => a.text.trim().length > 0);
  const aboutFiles = cleanFiles(options.sourceFiles);
  return {
    id,
    role: 'user',
    content,
    timestamp,
    ...(extras.sideSummary ? { sideSummary: extras.sideSummary } : {}),
    ...(attachedContext.length > 0 ? { attachedContext: [...attachedContext] } : {}),
    ...(aboutFiles.length > 0 ? { aboutFiles } : {}),
  };
}

/** Index of the question the answer at `answerIndex` replies to; -1 when there is none. */
export function questionIndexBefore(messages: readonly Pick<ChatMessage, 'role'>[], answerIndex: number): number {
  for (let i = Math.min(answerIndex, messages.length) - 1; i >= 0; i--) {
    if (messages[i].role === 'user') return i;
  }
  return -1;
}

/**
 * The options a retry of `asked` runs with: the current ones, limited to the files the
 * question was about (only those, whatever sources are selected now).
 */
export function retryOptions(options: SendOptions, asked: Pick<ChatMessage, 'aboutFiles'>): SendOptions {
  return limitedToFiles(options, asked.aboutFiles);
}

/** A new answer and the stored form it is saved with from its start. */
export interface StartedAnswer {
  message: ChatMessage;
  /**
   * Saved in place of the live answer until it settles, so an app closed (or reloaded)
   * before the answer ends shows it as interrupted, with Retry, instead of leaving a
   * question with no answer.
   */
  checkpoint: ConversationMessage;
}

/** The answer a run starts with: empty, running, saved from its start. */
export function startedAnswer(
  id: string,
  runId: string,
  startedAtMs: number,
  fallback: Parameters<typeof initialTranscript>[2] = null,
  priorRunIds: readonly string[] = [],
): StartedAnswer {
  const transcript: TranscriptState = {
    ...initialTranscript(runId, startedAtMs, fallback),
    ...(priorRunIds.length > 0 ? { priorRunIds: [...priorRunIds] } : {}),
  };
  const message: ChatMessage = {
    id,
    role: 'assistant',
    content: '',
    timestamp: new Date(startedAtMs).toISOString(),
    transcript,
  };
  return { message, checkpoint: toStored(message) };
}

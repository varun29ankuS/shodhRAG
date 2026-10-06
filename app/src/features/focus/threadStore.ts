/**
 * Side threads: validation, pure list operations, and the two places they
 * are kept.
 *
 * - Threads anchored to a message are stored inside that message's opaque
 *   `metadata` (key `focusThreads`), which `save_conversation` already
 *   persists to conversations.json. They sync with the conversation.
 * - Threads with no parent message (e.g. about a task) are stored on the
 *   conversation itself (`focusThreads`, `ConversationRecord.focus_threads`).
 *   Earlier versions kept them in this device's localStorage; those are moved
 *   into the conversation once (`mergeThreads`) and the old key removed after
 *   the conversation saved.
 *
 * Pure module (storage is injected) so it is unit-tested with Node
 * (`app/tests/focusThreads.test.ts`).
 */

import type {
  FocusDocumentRef,
  FocusExtras,
  FocusPageSpan,
  FocusParamValue,
  FocusSourceHit,
  FocusTarget,
  FocusTaskSnapshot,
  FocusThread,
  ThreadAnchor,
  ThreadTurn,
} from './focusTypes.ts';
import type { AnswerScope } from '../agent/useAgentSession.ts';
import { readFollowups, stripFollowups } from './followups.ts';
import { safeTruncate } from './markdownSafe.ts';
import { SVG_MAX_CHARS } from '../ask/visual/svgSanitize.ts';
import { PLOT_MAX_CHARS } from '../ask/visual/plotSpec.ts';
import { SIMULATION_MAX_CHARS } from '../ask/visual/simulationSpec.ts';
import { parseRegions } from '../ask/viewer/regionGeometry.ts';
import { readSymbolNotes } from '../ask/visual/symbols.ts';

/** Key of the side threads inside a message's `metadata`. */
export const METADATA_KEY = 'focusThreads';
/** localStorage key prefix where earlier versions kept threads without a parent message. */
export const LOCAL_KEY_PREFIX = 'shodh.focusThreads.v1.';
/** Threads without a parent message kept per conversation (oldest dropped first). */
export const MAX_LOCAL_THREADS = 40;
/** Turns kept per thread (oldest dropped first). */
export const MAX_TURNS = 200;
/** Characters of a stored object payload (diagram source, table cells, …). */
export const MAX_TARGET_CHARS = 40_000;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function str(value: unknown): value is string {
  return typeof value === 'string';
}

function capped(text: string, max = MAX_TARGET_CHARS): string {
  const chars = Array.from(text);
  return chars.length > max ? chars.slice(0, max).join('') : text;
}

function readSpan(value: unknown): FocusPageSpan | null {
  if (!isRecord(value)) return null;
  const { start, end } = value;
  if (typeof start !== 'number' || typeof end !== 'number' || !Number.isFinite(start) || !Number.isFinite(end)) return null;
  return { start, end };
}

function readHit(value: unknown): FocusSourceHit | null {
  if (!isRecord(value) || !str(value.sourceFile) || !value.sourceFile) return null;
  const lr = value.lineRange;
  return {
    number: typeof value.number === 'number' ? value.number : 0,
    sourceFile: value.sourceFile,
    fileName: str(value.fileName) ? value.fileName : '',
    title: str(value.title) ? value.title : '',
    text: str(value.text) ? capped(value.text) : '',
    snippet: str(value.snippet) ? capped(value.snippet) : '',
    score: typeof value.score === 'number' ? value.score : 0,
    page: readSpan(value.page),
    lineRange: Array.isArray(lr) && lr.length === 2 && lr.every(n => typeof n === 'number') ? [lr[0], lr[1]] : null,
    url: str(value.url) ? value.url : null,
    ...(value.regions !== undefined && value.regions !== null ? { regions: parseRegions(value.regions) } : {}),
  };
}

function finiteNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

/** A stored snippet rectangle (PDF points, top-left origin), or null. */
function readRect(value: unknown): { x: number; y: number; width: number; height: number } | null {
  if (!isRecord(value)) return null;
  const { x, y, width, height } = value;
  if (!finiteNumber(x) || !finiteNumber(y) || !finiteNumber(width) || !finiteNumber(height)) return null;
  if (x < 0 || y < 0 || width <= 0 || height <= 0) return null;
  return { x, y, width, height };
}

function readBox(value: unknown): { x0: number; y0: number; x1: number; y1: number } | null {
  if (!isRecord(value)) return null;
  const { x0, y0, x1, y1 } = value;
  if (!finiteNumber(x0) || !finiteNumber(y0) || !finiteNumber(x1) || !finiteNumber(y1)) return null;
  return x1 > x0 && y1 > y0 ? { x0, y0, x1, y1 } : null;
}

function readTask(value: unknown): FocusTaskSnapshot | null {
  if (!isRecord(value) || !str(value.id) || !str(value.title)) return null;
  return {
    id: value.id,
    title: value.title,
    status: str(value.status) ? value.status : '',
    priority: str(value.priority) ? value.priority : '',
    dueDate: str(value.dueDate) ? value.dueDate : null,
    notes: str(value.notes) ? capped(value.notes) : '',
    tags: Array.isArray(value.tags) ? value.tags.filter(str) : [],
    subtasks: Array.isArray(value.subtasks)
      ? value.subtasks.filter(isRecord).filter(s => str(s.title)).map(s => ({ title: s.title as string, completed: s.completed === true }))
      : [],
    project: str(value.project) ? value.project : null,
  };
}

function readDocument(value: unknown): FocusDocumentRef | null {
  if (!isRecord(value) || !str(value.sourceFile) || !value.sourceFile) return null;
  const page = typeof value.page === 'number' && Number.isInteger(value.page) && value.page > 0 ? value.page : null;
  return { sourceFile: value.sourceFile, fileName: str(value.fileName) ? value.fileName : '', page };
}

/** Slider positions kept with a plot or simulation target. */
export const MAX_PARAM_VALUES = 8;

/** Valid slider positions (well-formed names, finite values, no duplicates), capped. */
export function readParamValues(value: unknown): FocusParamValue[] {
  if (!Array.isArray(value)) return [];
  const out: FocusParamValue[] = [];
  for (const v of value) {
    if (out.length >= MAX_PARAM_VALUES) break;
    if (!isRecord(v) || !str(v.name) || !/^[A-Za-z][A-Za-z0-9_]{0,23}$/.test(v.name)) continue;
    if (typeof v.value !== 'number' || !Number.isFinite(v.value)) continue;
    if (out.some(o => o.name === v.name)) continue;
    out.push({ name: v.name, value: v.value });
  }
  return out;
}

/**
 * Source of a drawn visual, or null when it is over the render cap: cutting
 * it would leave markup or JSON that cannot be drawn again.
 */
function visualSource(value: unknown, max: number): string | null {
  return str(value) && value.trim() && value.length <= max ? value : null;
}

/** Characters of a selected text and of its paragraph kept with a thread. */
export const MAX_SELECTION_TARGET_CHARS = 4_000;
export const MAX_PARAGRAPH_TARGET_CHARS = 3_000;
/** Characters of a diagram's parse error kept with a thread. */
export const MAX_DIAGRAM_ERROR_CHARS = 1_000;

/** A stored target, or null when it is not a valid one. */
export function readTarget(value: unknown): FocusTarget | null {
  if (!isRecord(value) || !str(value.kind)) return null;
  const label = str(value.label) && value.label.trim() ? value.label : null;
  if (!label) return null;
  switch (value.kind) {
    case 'mermaid': {
      if (!str(value.source)) return null;
      const error = str(value.error) && value.error.trim() ? capped(value.error.trim(), MAX_DIAGRAM_ERROR_CHARS) : '';
      return { kind: 'mermaid', label, source: capped(value.source), ...(error ? { error } : {}) };
    }
    case 'chart':
      return str(value.source) ? { kind: value.kind, label, source: capped(value.source) } : null;
    case 'svg': {
      const source = visualSource(value.source, SVG_MAX_CHARS);
      return source ? { kind: 'svg', label, source } : null;
    }
    case 'plot':
    case 'simulation': {
      const source = visualSource(value.source, value.kind === 'plot' ? PLOT_MAX_CHARS : SIMULATION_MAX_CHARS);
      return source ? { kind: value.kind, label, source, values: readParamValues(value.values) } : null;
    }
    case 'equation': {
      if (!str(value.tex)) return null;
      const symbols = readSymbolNotes(value.symbols);
      return { kind: 'equation', label, tex: capped(value.tex), ...(symbols.length > 0 ? { symbols } : {}) };
    }
    case 'figure': {
      if (!str(value.filePath) || !value.filePath) return null;
      const page = value.page;
      const box = readBox(value.bbox);
      if (!box || typeof page !== 'number' || !Number.isInteger(page) || page < 1) return null;
      return {
        kind: 'figure',
        label,
        filePath: value.filePath,
        fileName: str(value.fileName) ? value.fileName : '',
        page,
        bbox: box,
        figureId: str(value.figureId) && value.figureId ? value.figureId : null,
        caption: str(value.caption) ? capped(value.caption, MAX_SELECTION_TARGET_CHARS) : '',
        nearby: str(value.nearby) ? capped(value.nearby, MAX_PARAGRAPH_TARGET_CHARS) : '',
      };
    }
    case 'derivation_step': {
      const { index, total } = value;
      if (!str(value.latex) || !value.latex.trim()) return null;
      if (typeof index !== 'number' || typeof total !== 'number' || !Number.isInteger(index) || !Number.isInteger(total) || index < 0 || index >= total) return null;
      const symbols = readSymbolNotes(value.symbols);
      return {
        kind: 'derivation_step',
        label,
        title: str(value.title) ? value.title : '',
        index,
        total,
        latex: capped(value.latex),
        justification: str(value.justification) ? capped(value.justification, MAX_SELECTION_TARGET_CHARS) : '',
        previous: str(value.previous) ? capped(value.previous) : null,
        next: str(value.next) ? capped(value.next) : null,
        ...(symbols.length > 0 ? { symbols } : {}),
      };
    }
    case 'table': {
      if (!Array.isArray(value.rows)) return null;
      const rows = value.rows.filter(Array.isArray).map(r => (r as unknown[]).map(c => (str(c) ? c : String(c ?? ''))));
      return rows.length > 0 ? { kind: 'table', label, rows } : null;
    }
    case 'image':
      return { kind: 'image', label, src: str(value.src) ? value.src : null, alt: str(value.alt) ? value.alt : '' };
    case 'source': {
      const hit = readHit(value.hit);
      return hit ? { kind: 'source', label, hit } : null;
    }
    case 'task': {
      const task = readTask(value.task);
      return task ? { kind: 'task', label, task } : null;
    }
    case 'snippet': {
      if (!str(value.snippetId) || !value.snippetId || !str(value.filePath) || !value.filePath) return null;
      const rect = readRect(value.rect);
      const page = value.page;
      if (!rect || typeof page !== 'number' || !Number.isInteger(page) || page < 1) return null;
      return {
        kind: 'snippet',
        label,
        snippetId: value.snippetId,
        filePath: value.filePath,
        fileName: str(value.fileName) ? value.fileName : '',
        page,
        rect,
        text: str(value.text) ? capped(value.text, MAX_SELECTION_TARGET_CHARS * 2) : '',
      };
    }
    case 'selection': {
      if (!str(value.text) || !value.text.trim()) return null;
      return {
        kind: 'selection',
        label,
        text: capped(value.text, MAX_SELECTION_TARGET_CHARS),
        paragraph: str(value.paragraph) ? capped(value.paragraph, MAX_PARAGRAPH_TARGET_CHARS) : '',
        origin: value.origin === 'document' ? 'document' : 'answer',
        document: readDocument(value.document),
      };
    }
    default:
      return null;
  }
}

function readTurn(value: unknown): ThreadTurn | null {
  if (!isRecord(value) || !str(value.id) || !str(value.content) || !str(value.timestamp)) return null;
  if (value.role !== 'user' && value.role !== 'assistant') return null;
  const turn: ThreadTurn = { id: value.id, role: value.role, content: value.content, timestamp: value.timestamp };
  if (str(value.selection) && value.selection) turn.selection = value.selection;
  if (typeof value.page === 'number' && Number.isFinite(value.page)) turn.page = value.page;
  if (isRecord(value.transcript)) turn.transcript = value.transcript;
  const followups = readFollowups(value.followups);
  if (followups && turn.role === 'assistant') turn.followups = followups;
  const from = value.summaryOf;
  if (turn.role === 'user' && isRecord(from) && str(from.threadId) && from.threadId && str(from.label)) {
    turn.summaryOf = { threadId: from.threadId, label: from.label };
  }
  return turn;
}

function readAnchor(value: unknown): ThreadAnchor | null {
  if (!isRecord(value) || !str(value.conversationId) || !value.conversationId) return null;
  const target = readTarget(value.target);
  if (!target) return null;
  const parent = str(value.parentMessageId) && value.parentMessageId ? value.parentMessageId : null;
  return { conversationId: value.conversationId, parentMessageId: parent, target };
}

/** A stored thread, or null when it cannot be used. Bad turns are dropped. */
export function readThread(value: unknown): FocusThread | null {
  if (!isRecord(value) || !str(value.id) || !value.id) return null;
  const anchor = readAnchor(value.anchor);
  if (!anchor) return null;
  const turns = Array.isArray(value.turns) ? value.turns.map(readTurn).filter((t): t is ThreadTurn => t !== null) : [];
  const createdAt = str(value.createdAt) ? value.createdAt : new Date(0).toISOString();
  const thread: FocusThread = {
    id: value.id,
    anchor,
    turns: turns.slice(-MAX_TURNS),
    createdAt,
    updatedAt: str(value.updatedAt) ? value.updatedAt : createdAt,
  };
  // Threads saved before drill-down existed have no parent: they are roots.
  if (str(value.parentThreadId) && value.parentThreadId && value.parentThreadId !== value.id) {
    thread.parentThreadId = value.parentThreadId;
    if (str(value.parentTurnId) && value.parentTurnId) thread.parentTurnId = value.parentTurnId;
  }
  return thread;
}

/** Valid threads of a stored list; anything malformed is dropped, never thrown. */
export function readThreads(value: unknown): FocusThread[] {
  if (!Array.isArray(value)) return [];
  const seen = new Set<string>();
  const out: FocusThread[] = [];
  for (const item of value) {
    const thread = readThread(item);
    if (thread && !seen.has(thread.id)) {
      seen.add(thread.id);
      out.push(thread);
    }
  }
  return out;
}

/**
 * Threads of both lists, one per id: on a clash the one updated later wins.
 * Ordered by last update, capped at `MAX_LOCAL_THREADS` (oldest dropped).
 */
export function mergeThreads(existing: readonly FocusThread[], incoming: readonly FocusThread[]): FocusThread[] {
  const byId = new Map<string, FocusThread>();
  for (const thread of [...existing, ...incoming]) {
    const kept = byId.get(thread.id);
    if (!kept || thread.updatedAt > kept.updatedAt) byId.set(thread.id, thread);
  }
  return [...byId.values()]
    .sort((a, b) => (a.updatedAt < b.updatedAt ? -1 : a.updatedAt > b.updatedAt ? 1 : 0))
    .slice(-MAX_LOCAL_THREADS);
}

/** Insert or replace a thread (by id), newest-updated last. */
export function upsertThread(list: readonly FocusThread[], thread: FocusThread): FocusThread[] {
  const rest = list.filter(t => t.id !== thread.id);
  return [...rest, { ...thread, turns: thread.turns.slice(-MAX_TURNS) }];
}

/** Append a turn (or replace one with the same id) to a thread. */
export function appendTurn(thread: FocusThread, turn: ThreadTurn): FocusThread {
  const exists = thread.turns.some(t => t.id === turn.id);
  const turns = exists ? thread.turns.map(t => (t.id === turn.id ? turn : t)) : [...thread.turns, turn];
  return { ...thread, turns: turns.slice(-MAX_TURNS), updatedAt: turn.timestamp };
}

export function removeThread(list: readonly FocusThread[], threadId: string): FocusThread[] {
  return list.filter(t => t.id !== threadId);
}

/**
 * Threads of one parent message, oldest first. Includes nested
 * (drill-down) threads; see `threadTree` for the hierarchy.
 */
export function threadsForMessage(list: readonly FocusThread[], messageId: string): FocusThread[] {
  return list.filter(t => t.anchor.parentMessageId === messageId);
}

/** Threads stored in a message's metadata. */
export function threadsFromMetadata(metadata: unknown): FocusThread[] {
  return isRecord(metadata) ? readThreads(metadata[METADATA_KEY]) : [];
}

/** Metadata without the thread key (what the rest of the app reads). */
export function metadataWithoutThreads(metadata: unknown): Record<string, unknown> | undefined {
  if (!isRecord(metadata)) return undefined;
  if (!(METADATA_KEY in metadata)) return metadata;
  const { [METADATA_KEY]: _threads, ...rest } = metadata;
  return Object.keys(rest).length > 0 ? rest : undefined;
}

/** Metadata with `threads` merged in, keeping every other key. */
export function metadataWithThreads(metadata: unknown, threads: readonly FocusThread[]): Record<string, unknown> | undefined {
  const base = metadataWithoutThreads(metadata) ?? {};
  if (threads.length === 0) return Object.keys(base).length > 0 ? base : undefined;
  return { ...base, [METADATA_KEY]: threads };
}

/** The part of the Web Storage API the local store needs. */
export interface KeyValueStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
  /** Key enumeration, used to drop threads of deleted conversations. */
  readonly length?: number;
  key?(index: number): string | null;
}

export interface LocalThreadStore {
  /** Threads kept for a conversation; empty when nothing (valid) is stored. */
  list(conversationId: string): FocusThread[];
  /** Replace the conversation's threads. False when storage refused the write. */
  save(conversationId: string, threads: readonly FocusThread[]): boolean;
  /** Forget the conversation's threads. */
  clear(conversationId: string): void;
  /**
   * Drop threads of conversations that no longer exist. Returns how many
   * conversations' threads were removed.
   */
  prune(keepConversationIds: ReadonlySet<string>): number;
}

/**
 * Threads without a parent message, per conversation, in Web Storage.
 * Unreadable or corrupt entries read as empty; a full or unavailable
 * storage reports failure instead of throwing.
 */
export function createLocalThreadStore(storage: KeyValueStorage | null): LocalThreadStore {
  const key = (conversationId: string) => `${LOCAL_KEY_PREFIX}${conversationId}`;
  return {
    list(conversationId) {
      if (!storage) return [];
      let raw: string | null;
      try {
        raw = storage.getItem(key(conversationId));
      } catch {
        return [];
      }
      if (!raw) return [];
      try {
        return readThreads(JSON.parse(raw)).filter(t => t.anchor.conversationId === conversationId);
      } catch {
        return [];
      }
    },
    save(conversationId, threads) {
      if (!storage) return false;
      const kept = threads.filter(t => t.anchor.conversationId === conversationId).slice(-MAX_LOCAL_THREADS);
      try {
        if (kept.length === 0) storage.removeItem(key(conversationId));
        else storage.setItem(key(conversationId), JSON.stringify(kept));
        return true;
      } catch {
        return false;
      }
    },
    clear(conversationId) {
      if (!storage) return;
      try {
        storage.removeItem(key(conversationId));
      } catch {
        // Storage unavailable: nothing to clear.
      }
    },
    prune(keep) {
      if (!storage || typeof storage.key !== 'function') return 0;
      const stale: string[] = [];
      try {
        const count = storage.length ?? 0;
        for (let i = 0; i < count; i++) {
          const k = storage.key(i);
          if (k && k.startsWith(LOCAL_KEY_PREFIX) && !keep.has(k.slice(LOCAL_KEY_PREFIX.length))) stale.push(k);
        }
        for (const k of stale) storage.removeItem(k);
      } catch {
        return 0;
      }
      return stale.length;
    },
  };
}

export interface HistoryTurnLike {
  role: 'user' | 'assistant';
  content: string;
}

/**
 * Turns replayed into a side thread's fresh agent session: the main
 * conversation up to the parent message, then the thread's own earlier
 * turns. Thread turns win when the limit is reached.
 */
export function threadHistory(main: readonly HistoryTurnLike[], thread: readonly HistoryTurnLike[], limit = 10): HistoryTurnLike[] {
  const usable = (t: HistoryTurnLike) => t.content.trim().length > 0;
  const own = thread
    .map(t => (t.role === 'assistant' ? { role: t.role, content: stripFollowups(t.content) } : t))
    .filter(usable)
    .slice(-limit);
  const room = Math.max(0, limit - own.length);
  const lead = room > 0 ? main.filter(usable).slice(-room) : [];
  return [...lead, ...own];
}

/**
 * Adjacent turns of the same role joined into one, so replayed history
 * alternates (a summary brought back from a nested discussion is a user
 * turn that was never answered).
 */
export function alternateTurns(turns: readonly HistoryTurnLike[]): HistoryTurnLike[] {
  const merged: HistoryTurnLike[] = [];
  for (const turn of turns) {
    const last = merged[merged.length - 1];
    if (last && last.role === turn.role) merged[merged.length - 1] = { role: last.role, content: `${last.content}\n\n${turn.content}` };
    else merged.push({ role: turn.role, content: turn.content });
  }
  return merged;
}

/** "3 replies about Revenue by quarter" ("1 question about …" before the first answer). */
export function repliesLabel(thread: Pick<FocusThread, 'turns' | 'anchor'>): string {
  const answers = thread.turns.filter(t => t.role === 'assistant').length;
  if (answers === 0) {
    const questions = thread.turns.filter(t => t.role === 'user').length;
    return `${questions} ${questions === 1 ? 'question' : 'questions'} about ${thread.anchor.target.label}`;
  }
  return `${answers} ${answers === 1 ? 'reply' : 'replies'} about ${thread.anchor.target.label}`;
}

/** An agent session id for a side thread: `[A-Za-z0-9_-]` only, within the backend's 200-char limit. */
export function sideSessionKey(conversationId: string, threadId: string): string {
  const safe = (s: string) => s.replace(/[^A-Za-z0-9_-]/g, '_');
  return `${safe(conversationId).slice(0, 90)}--focus--${safe(threadId).slice(0, 90)}`;
}

/**
 * Every agent session a side thread may have started: its discussion, its summary
 * and its visual refinement (each runs in a session of its own).
 */
export function sideSessionKeys(conversationId: string, threadId: string): string[] {
  return [threadId, `${threadId}-summary`, `${threadId}-refine`].map(t => sideSessionKey(conversationId, t));
}

/** Whether two targets are the same object (same kind and content). */
export function sameTarget(a: FocusTarget, b: FocusTarget): boolean {
  if (a.kind !== b.kind) return false;
  switch (a.kind) {
    case 'source': {
      const other = b as typeof a;
      return a.hit.sourceFile === other.hit.sourceFile && a.hit.number === other.hit.number && a.hit.text === other.hit.text;
    }
    case 'task':
      return a.task.id === (b as typeof a).task.id;
    case 'snippet':
      return a.snippetId === (b as typeof a).snippetId;
    default:
      return JSON.stringify(a) === JSON.stringify(b);
  }
}

/** Longest answer excerpt placed in the default summary. */
export const SUMMARY_ANSWER_CHARS = 700;

/**
 * Fallback summary of a side discussion, used when the agent could not
 * write one: the last question and (an excerpt of) its answer, as Markdown.
 * It has no "From a side discussion about …" header: the card and the
 * prompt that carry it already say so. Empty before the first answer.
 */
export function threadSummary(thread: Pick<FocusThread, 'turns'>): string {
  const lastAnswerIndex = (() => {
    for (let i = thread.turns.length - 1; i >= 0; i--) if (thread.turns[i].role === 'assistant' && thread.turns[i].content.trim()) return i;
    return -1;
  })();
  if (lastAnswerIndex < 0) return '';
  let question = '';
  for (let i = lastAnswerIndex - 1; i >= 0; i--) {
    if (thread.turns[i].role === 'user') {
      question = thread.turns[i].content.replace(/\s+/g, ' ').trim();
      break;
    }
  }
  const answer = stripFollowups(thread.turns[lastAnswerIndex].content).trim();
  // Never cut inside an equation or a fenced block: they would render as raw source.
  const excerpt = safeTruncate(answer, SUMMARY_ANSWER_CHARS);
  return [question ? `**Asked:** ${question}` : '', excerpt].filter(Boolean).join('\n\n');
}

/** Pages either side of a passage a document side question also searches. */
const SCOPE_PAGE_MARGIN = 1;

/** An indexed file a side question can be limited to (web results and virtual sources are not). */
function indexedFile(sourceFile: string, url: string | null): boolean {
  return !url && Boolean(sourceFile) && !sourceFile.includes('://');
}

/**
 * What a side question may search. About a passage of an indexed file, or
 * text selected in an indexed file's viewer: that file only, on the page
 * being viewed (else the passage's pages, else the page the selection was
 * made on), one page either side so text running over a page break is
 * found. Other targets (tasks, charts, web results, text selected in an
 * answer) search everything.
 */
export function sideScope(target: FocusTarget, extras: FocusExtras): AnswerScope | null {
  let sourceFile: string;
  let known: FocusPageSpan | null;
  if (target.kind === 'source') {
    const { hit } = target;
    if (!indexedFile(hit.sourceFile, hit.url)) return null;
    sourceFile = hit.sourceFile;
    known = hit.page;
  } else if (target.kind === 'snippet') {
    if (!indexedFile(target.filePath, null)) return null;
    sourceFile = target.filePath;
    known = { start: target.page, end: target.page };
  } else if (target.kind === 'selection' && target.origin === 'document' && target.document) {
    const { document } = target;
    if (!indexedFile(document.sourceFile, null)) return null;
    sourceFile = document.sourceFile;
    known = document.page !== null ? { start: document.page, end: document.page } : null;
  } else {
    return null;
  }
  const span = typeof extras.page === 'number' && extras.page > 0
    ? { start: extras.page, end: extras.page }
    : known;
  const scope: AnswerScope = { sourceIds: [], sourceFiles: [sourceFile] };
  if (span && span.start > 0 && span.end >= span.start) {
    const first = Math.max(1, Math.floor(span.start) - SCOPE_PAGE_MARGIN);
    const last = Math.floor(span.end) + SCOPE_PAGE_MARGIN;
    scope.pages = Array.from({ length: last - first + 1 }, (_, i) => first + i);
  }
  return scope;
}

/**
 * Scope of a question asked at the innermost of `targets` (outermost
 * first: the pop-out's levels down to the one asked at). The innermost
 * level's own scope when it is about a document, with the page being viewed
 * (`extras`); otherwise the nearest outer level about a document (a chart
 * or a selection opened from a document's discussion is still about that
 * document), at that level's own passage pages since the viewed page
 * belongs to the inner level. Null (search everything) when no level is.
 */
export function scopeForLevels(targets: readonly FocusTarget[], extras: FocusExtras): AnswerScope | null {
  for (let i = targets.length - 1; i >= 0; i--) {
    const scope = sideScope(targets[i], i === targets.length - 1 ? extras : {});
    if (scope) return scope;
  }
  return null;
}

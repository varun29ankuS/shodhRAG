/**
 * One answer as the phone shows it, folded from the computer's agent events
 * (`agent_event`, app/src/features/agent/events.ts) or read from the
 * transcript the computer saved with it. Only what the phone shows: the
 * answer's text without the model's working notes and replaced drafts, the
 * numbered sources it cites, what it is doing now, and the steps waiting for
 * an approval.
 *
 * The desktop's own reducer (app/src/features/agent/reducer.ts) folds far
 * more (plans, grounding, commands, reasoning) and is type-checked with the
 * desktop's looser settings, so the phone keeps this small fold of the same
 * event and transcript shapes.
 */

export type AnswerStatus = 'starting' | 'running' | 'completed' | 'aborted' | 'error';

/** A numbered source the answer cites as `[n]`: a passage of a file, or a web page. */
export interface Passage {
  n: number;
  /** The file's name, or the web page's title. */
  title: string;
  /** Where to open it: the file on the computer (as its file system spells it), or the URL. */
  path: string;
  /** 1-based page, for a paged file. */
  page: number | null;
  heading: string | null;
  text: string;
  web: boolean;
}

export interface Waiting {
  stepId: string;
  label: string;
  tool: string;
}

export interface AnswerState {
  runId: string | null;
  status: AnswerStatus;
  /** Text by message, in order. */
  blocks: Array<{ id: string; text: string }>;
  /** Messages that are working notes ("I'll search…") or drafts a revision replaced. */
  hidden: string[];
  passages: Passage[];
  /** What the assistant is doing now, in its words, while a step runs. */
  doing: string | null;
  waiting: Waiting[];
  error: string | null;
}

export function startingAnswer(): AnswerState {
  return { runId: null, status: 'starting', blocks: [], hidden: [], passages: [], doing: null, waiting: [], error: null };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function text(value: unknown): string | null {
  return typeof value === 'string' && value.trim() !== '' ? value : null;
}

/** The first page of a page or span ("4", "4-5", 4). */
export function firstPage(value: unknown): number | null {
  const n = typeof value === 'number' ? value : typeof value === 'string' ? Number(/^\s*(\d+)/.exec(value)?.[1]) : NaN;
  return Number.isInteger(n) && n > 0 ? n : null;
}

/** The file name of a path, either separator. */
export function fileName(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

/** Numbered sources in a step's detail (`passages` of a document search, `webSources` of a web tool). */
export function passagesFromDetail(detail: unknown): Passage[] {
  if (!isRecord(detail)) return [];
  const out: Passage[] = [];
  if (Array.isArray(detail.webSources)) {
    for (const entry of detail.webSources) {
      if (!isRecord(entry)) continue;
      const url = text(entry.url);
      if (typeof entry.n !== 'number' || !Number.isInteger(entry.n) || entry.n < 1 || !url || !/^https?:\/\//i.test(url)) continue;
      out.push({ n: entry.n, title: text(entry.title) ?? url, path: url, page: null, heading: null, text: typeof entry.snippet === 'string' ? entry.snippet : '', web: true });
    }
  }
  if (Array.isArray(detail.passages)) {
    for (const entry of detail.passages) {
      if (!isRecord(entry)) continue;
      const path = text(entry.path);
      if (typeof entry.n !== 'number' || !Number.isInteger(entry.n) || entry.n < 1 || !path) continue;
      // `path` is lowercased on Windows; the file system's spelling opens the file.
      const shown = text(entry.display_path) ?? text(entry.displayPath) ?? path;
      out.push({
        n: entry.n,
        title: text(entry.file) ?? fileName(shown),
        path: shown,
        page: firstPage(entry.page),
        heading: text(entry.heading),
        text: typeof entry.text === 'string' ? entry.text : '',
        web: false,
      });
    }
  }
  return out;
}

function mergePassages(existing: Passage[], incoming: Passage[]): Passage[] {
  if (incoming.length === 0) return existing;
  const byNumber = new Map<number, Passage>();
  for (const p of existing) byNumber.set(p.n, p);
  for (const p of incoming) byNumber.set(p.n, p);
  return [...byNumber.values()].sort((a, b) => a.n - b.n);
}

function appendText(blocks: AnswerState['blocks'], id: string, delta: string): AnswerState['blocks'] {
  const index = blocks.findIndex(b => b.id === id);
  if (index < 0) return [...blocks, { id, text: delta }];
  const next = blocks.slice();
  next[index] = { id, text: next[index].text + delta };
  return next;
}

const STATUSES: readonly AnswerStatus[] = ['completed', 'aborted', 'error'];

/** Fold one event of the answer's run (events of other runs are ignored). */
export function foldAnswer(state: AnswerState, event: unknown): AnswerState {
  if (!isRecord(event) || typeof event.type !== 'string') return state;
  if (state.runId !== null && typeof event.runId === 'string' && event.runId !== state.runId) return state;
  const running: AnswerState = state.status === 'starting' ? { ...state, status: 'running' } : state;
  switch (event.type) {
    case 'run_started':
      return running;
    case 'text_delta': {
      const id = text(event.messageId);
      if (!id || typeof event.delta !== 'string') return state;
      return { ...running, blocks: appendText(running.blocks, id, event.delta) };
    }
    case 'text_interim': {
      const id = text(event.messageId);
      if (!id) return state;
      const hidden = running.hidden.filter(h => h !== id);
      return { ...running, hidden: event.interim === true ? [...hidden, id] : hidden };
    }
    case 'step_started':
      return { ...running, doing: text(event.label) ?? running.doing };
    case 'step_finished': {
      const stepId = text(event.stepId);
      return {
        ...running,
        doing: null,
        passages: mergePassages(running.passages, passagesFromDetail(event.detail)),
        waiting: running.waiting.filter(w => w.stepId !== stepId),
      };
    }
    case 'approval_requested': {
      const stepId = text(event.stepId);
      if (!stepId) return state;
      const waiting = running.waiting.filter(w => w.stepId !== stepId);
      return { ...running, waiting: [...waiting, { stepId, label: text(event.label) ?? 'Approve a step', tool: text(event.tool) ?? '' }] };
    }
    case 'grounding': {
      const report = isRecord(event.report) ? event.report : null;
      const replaced = report && Array.isArray(report.supersededMessageIds) ? report.supersededMessageIds.filter((id): id is string => typeof id === 'string') : [];
      return replaced.length === 0 ? running : { ...running, hidden: [...new Set([...running.hidden, ...replaced])] };
    }
    case 'run_finished': {
      const status = STATUSES.includes(event.status as AnswerStatus) ? (event.status as AnswerStatus) : 'error';
      return { ...running, status, doing: null, waiting: [], error: typeof event.error === 'string' ? event.error : null };
    }
    default:
      return running;
  }
}

/** The answer's text: its messages in order, without working notes and replaced drafts. */
export function answerText(state: AnswerState): string {
  const hidden = new Set(state.hidden);
  return state.blocks
    .filter(b => !hidden.has(b.id))
    .map(b => b.text.trim())
    .filter(t => t !== '')
    .join('\n\n');
}

/** An answer as the computer saved it: its transcript, or only its text when it has none. */
export function savedAnswer(content: string, transcript: unknown): AnswerState {
  const fallback: AnswerState = { ...startingAnswer(), status: 'completed', blocks: content.trim() ? [{ id: 'saved', text: content }] : [] };
  if (!isRecord(transcript) || !Array.isArray(transcript.blocks)) return fallback;
  const blocks = transcript.blocks
    .filter(isRecord)
    .filter(b => b.kind === 'text' && typeof b.id === 'string' && typeof b.text === 'string')
    .map(b => ({ id: b.id as string, text: b.text as string }));
  const hidden = new Set<string>(Array.isArray(transcript.interimIds) ? transcript.interimIds.filter((id): id is string => typeof id === 'string') : []);
  if (Array.isArray(transcript.groundings)) {
    for (const report of transcript.groundings) {
      if (isRecord(report) && Array.isArray(report.supersededMessageIds)) {
        for (const id of report.supersededMessageIds) if (typeof id === 'string') hidden.add(id);
      }
    }
  }
  const passages: Passage[] = [];
  if (Array.isArray(transcript.passages)) {
    for (const p of transcript.passages) {
      if (!isRecord(p) || typeof p.n !== 'number' || !text(p.path)) continue;
      const path = p.path as string;
      const shown = p.web === true ? path : (text(p.displayPath) ?? path);
      passages.push({
        n: p.n,
        title: text(p.file) ?? fileName(shown),
        path: shown,
        page: p.web === true ? null : firstPage(p.page),
        heading: text(p.heading),
        text: typeof p.text === 'string' ? p.text : '',
        web: p.web === true,
      });
    }
  }
  // Saved while it runs: the computer is still writing it, and saves it again when it ends
  // (`conversation_changed`), so it shows as being written until then.
  const live = transcript.status === 'starting' || transcript.status === 'running';
  const status: AnswerStatus = live ? 'running' : STATUSES.includes(transcript.status as AnswerStatus) ? (transcript.status as AnswerStatus) : 'completed';
  const state: AnswerState = {
    runId: typeof transcript.runId === 'string' ? transcript.runId : null,
    status,
    blocks,
    hidden: [...hidden],
    passages: passages.sort((a, b) => a.n - b.n),
    doing: null,
    waiting: [],
    error: typeof transcript.error === 'string' ? transcript.error : null,
  };
  return answerText(state) === '' && content.trim() !== '' ? { ...fallback, passages: state.passages } : state;
}

/** Whether the answer is still being written. */
export function isLive(state: AnswerState): boolean {
  return state.status === 'starting' || state.status === 'running';
}

/** Source numbers in the order the answer first cites them. */
export function citedNumbers(answer: string): number[] {
  const seen: number[] = [];
  for (const match of answer.matchAll(/\[(\d+(?:\s*[,;]\s*\d+)*)\]/g)) {
    for (const part of match[1].split(/[,;]/)) {
      const n = Number(part.trim());
      if (Number.isInteger(n) && n > 0 && !seen.includes(n)) seen.push(n);
    }
  }
  return seen;
}

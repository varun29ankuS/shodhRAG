/**
 * The one model correction a diagram may get after its source failed to
 * parse (and the deterministic repair did not help): the request, the
 * reading of the reply, and the ledger that caps it at one attempt per
 * diagram source.
 *
 * The ledger is keyed by a hash of the source, so the cap holds however
 * often the block remounts (streaming, scrolling, reopening the
 * conversation) and across restarts: an attempt is recorded before the
 * model is asked, and an attempt that never finished counts as failed.
 * A run that could not start (another answer holds the agent) is released
 * without using the attempt.
 *
 * Pure module (no React, no mermaid), unit-tested with Node
 * (`app/tests/mermaidRetry.test.ts`).
 */

import { fenced } from '../../focus/contextBlock.ts';
import { isMermaidLanguage, mermaidSource } from './mathText.ts';

/** Longest diagram source sent to the model for correction. */
export const MAX_REPAIR_SOURCE_CHARS = 20_000;
/** Longest parser message kept (sent to the model, kept with a side thread). */
export const MAX_PARSE_ERROR_CHARS = 1_000;
/** Corrections remembered; the oldest are forgotten first. */
export const MAX_LEDGER_ENTRIES = 100;
/** Storage key of the ledger. */
export const LEDGER_STORAGE_KEY = 'shodh.mermaidRepairs.v1';

const HEADER = /^\s*(graph|flowchart|sequenceDiagram|classDiagram|stateDiagram(-v2)?|erDiagram|journey|gantt|pie|gitGraph|mindmap|timeline|quadrantChart|xychart-beta|block-beta|sankey-beta)\b/;

/** Stable key of a diagram source: FNV-1a over its UTF-16 code units, with its length. */
export function sourceKey(source: string): string {
  let hash = 0x811c9dc5;
  for (let i = 0; i < source.length; i++) {
    hash ^= source.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return `mmd-${source.length.toString(36)}-${hash.toString(36)}`;
}

/** A parser message as kept: the whole message, capped; and its first line for display. */
export function parseErrorText(message: string): { full: string; short: string } {
  const full = Array.from(message.replace(/\r\n?/g, '\n').trim()).slice(0, MAX_PARSE_ERROR_CHARS).join('');
  const short = full.split('\n').find(l => l.trim())?.trim() ?? '';
  return { full, short: short || 'The diagram could not be drawn.' };
}

/** The message that asks the model for a corrected diagram. */
export function composeMermaidFixRequest(source: string, error: string): string {
  return [
    'This mermaid diagram from your previous answer does not draw: the mermaid parser rejects it.',
    '',
    'Parser message:',
    fenced(parseErrorText(error).full, 'text'),
    '',
    'Diagram:',
    fenced(source.trim(), 'mermaid'),
    '',
    'Reply with exactly one ```mermaid code block holding the complete corrected diagram, and nothing else. '
      + 'Keep its meaning, nodes, links and labels; change only what the parser needs. '
      + 'Put any label that contains punctuation in double quotes and write a double quote inside a label as #quot;. '
      + 'Do not search documents and do not use tools.',
  ].join('\n');
}

/**
 * The diagram in a model reply: the single mermaid fence (any diagram fence
 * language), or a reply that is only a diagram. Null when there is none or
 * more than one.
 */
export function extractMermaidReply(reply: string): string | null {
  const text = reply.replace(/\r\n?/g, '\n');
  const lines = text.split('\n');
  const found: string[] = [];
  let fences = 0;
  for (let i = 0; i < lines.length; i++) {
    const open = /^\s{0,3}(`{3,}|~{3,})\s*([\w-]*)\s*$/.exec(lines[i]);
    if (!open) continue;
    const marker = open[1];
    let close = -1;
    for (let j = i + 1; j < lines.length; j++) {
      const m = /^\s{0,3}(`{3,}|~{3,})\s*$/.exec(lines[j]);
      if (m && m[1][0] === marker[0] && m[1].length >= marker.length) {
        close = j;
        break;
      }
    }
    if (close < 0) break;
    fences += 1;
    const lang = open[2];
    const body = lines.slice(i + 1, close).join('\n');
    // An empty fence is no diagram (mermaidSource would give it a bare header).
    if (body.trim() && (isMermaidLanguage(lang) || (!lang && HEADER.test(body)))) {
      found.push(mermaidSource(lang || 'mermaid', body).trim());
    }
    i = close;
  }
  if (found.length === 1) return found[0] || null;
  if (found.length > 1) return null;
  const bare = text.trim();
  return fences === 0 && HEADER.test(bare) ? bare : null;
}

export type RepairAttempt =
  | { status: 'none' }
  | { status: 'pending' }
  | { status: 'fixed'; source: string }
  | { status: 'failed'; message: string };

/** What a diagram that did not draw offers. */
export type RepairOffer =
  /** Ask the model now, without a click. */
  | 'auto'
  /** Show the "Fix with the model" button. */
  | 'button'
  /** No model correction (not an agent answer, already tried, or too long). */
  | 'none';

export interface RepairOfferInput {
  /** The diagram is in a finished agent answer of the main or a side thread. */
  eligible: boolean;
  /** That answer is the newest one where it is shown (old answers never ask by themselves). */
  latest: boolean;
  attempt: RepairAttempt;
  sourceChars: number;
}

export function repairOffer(input: RepairOfferInput): RepairOffer {
  if (!input.eligible || input.attempt.status !== 'none') return 'none';
  if (input.sourceChars > MAX_REPAIR_SOURCE_CHARS) return 'none';
  return input.latest ? 'auto' : 'button';
}

/** The storage the ledger keeps attempts in (localStorage in the app). */
export interface LedgerStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export interface RepairLedger {
  get(key: string): RepairAttempt;
  /** Record an attempt for `key`; false when one was already made (or is running). */
  begin(key: string): boolean;
  /** Forget a recorded attempt that never reached the model (the run could not start). */
  release(key: string): void;
  succeed(key: string, source: string): void;
  fail(key: string, message: string): void;
  subscribe(listener: () => void): () => void;
}

const INTERRUPTED = 'The earlier correction did not finish.';
const NONE: RepairAttempt = { status: 'none' };

function readAttempt(value: unknown): RepairAttempt | null {
  if (typeof value !== 'object' || value === null) return null;
  const v = value as Record<string, unknown>;
  if (v.status === 'pending') return { status: 'failed', message: INTERRUPTED };
  if (v.status === 'fixed' && typeof v.source === 'string' && v.source.trim()) return { status: 'fixed', source: v.source };
  if (v.status === 'failed') return { status: 'failed', message: typeof v.message === 'string' ? v.message : INTERRUPTED };
  return null;
}

/**
 * The attempt ledger. `storage` null keeps it in memory only; storage
 * errors (quota, private mode) are ignored and the in-memory cap still holds.
 */
export function createRepairLedger(storage: LedgerStorage | null, maxEntries = MAX_LEDGER_ENTRIES): RepairLedger {
  const entries = new Map<string, RepairAttempt>();
  const listeners = new Set<() => void>();

  try {
    const raw = storage?.getItem(LEDGER_STORAGE_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : null;
    if (Array.isArray(parsed)) {
      for (const item of parsed.slice(-maxEntries)) {
        if (!Array.isArray(item) || typeof item[0] !== 'string') continue;
        const attempt = readAttempt(item[1]);
        if (attempt) entries.set(item[0], attempt);
      }
    }
  } catch {
    // Unreadable ledger: start empty.
  }

  const save = () => {
    if (!storage) return;
    try {
      storage.setItem(LEDGER_STORAGE_KEY, JSON.stringify([...entries]));
    } catch {
      // Not persisted; the in-memory ledger still caps attempts this session.
    }
  };

  const set = (key: string, attempt: RepairAttempt | null) => {
    entries.delete(key);
    if (attempt) entries.set(key, attempt);
    while (entries.size > maxEntries) {
      const oldest = entries.keys().next().value;
      if (oldest === undefined) break;
      entries.delete(oldest);
    }
    save();
    for (const listener of [...listeners]) listener();
  };

  return {
    get: key => entries.get(key) ?? NONE,
    begin: key => {
      if (entries.has(key)) return false;
      set(key, { status: 'pending' });
      return true;
    },
    release: key => {
      if (entries.get(key)?.status === 'pending') set(key, null);
    },
    succeed: (key, source) => {
      if (entries.get(key)?.status === 'pending') set(key, { status: 'fixed', source });
    },
    fail: (key, message) => {
      if (entries.get(key)?.status === 'pending') set(key, { status: 'failed', message: parseErrorText(message).short });
    },
    subscribe: listener => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

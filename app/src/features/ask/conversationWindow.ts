/**
 * Pure parts of the windowed conversation (AskView): how tall an unmeasured
 * message is assumed to be, when the view follows new output, which messages
 * stay mounted outside the visible range, and find-in-conversation over
 * messages that are not mounted. Tested in `app/tests/conversationWindow.test.ts`.
 */

/** Space between messages (the conversation column's former flex gap). */
export const MESSAGE_GAP_PX = 26;

/** Distance from the bottom within which the view follows new output. */
export const STICK_TO_BOTTOM_PX = 160;

/** What estimating needs of a message. */
export interface SizedMessage {
  role: 'user' | 'assistant' | 'system';
  content: string;
}

/** Characters per line of an answer at the column's width (700px at 16px). */
const ANSWER_CHARS_PER_LINE = 80;
const ANSWER_LINE_PX = 28;
const USER_CHARS_PER_LINE = 60;
const USER_LINE_PX = 23;

/**
 * Height assumed for a message before it is measured, gap included. Close
 * estimates keep the scrollbar steady; the real height replaces it on mount.
 */
export function estimateMessageHeight(message: SizedMessage): number {
  const text = message.content;
  if (message.role === 'user') {
    const lines = text.split('\n').reduce((n, line) => n + Math.max(1, Math.ceil(line.length / USER_CHARS_PER_LINE)), 0);
    return 22 + lines * USER_LINE_PX + MESSAGE_GAP_PX;
  }
  let lines = 0;
  let visuals = 0;
  let inFence: string | null = null;
  for (const line of text.split('\n')) {
    const fence = /^\s*```\s*([\w-]*)/.exec(line);
    if (fence) {
      if (inFence === null) {
        inFence = fence[1] ?? '';
        if (inFence !== '' && inFence !== 'chart' && inFence !== 'mermaid') lines += 2; // code header
        if (inFence === 'chart' || inFence === 'mermaid') visuals += 1;
      } else {
        inFence = null;
      }
      continue;
    }
    if (inFence === 'chart' || inFence === 'mermaid') continue;
    if (inFence !== null) {
      lines += 0.8; // 13px code lines
      continue;
    }
    if (line.trim() === '') {
      lines += 0.4;
      continue;
    }
    lines += Math.max(1, Math.ceil(line.length / ANSWER_CHARS_PER_LINE));
  }
  // Answers carry a step line, source chips and the action row besides the text.
  const chrome = message.role === 'assistant' ? 150 : 40;
  return Math.round(chrome + lines * ANSWER_LINE_PX + visuals * 340 + MESSAGE_GAP_PX);
}

/** Distance of the view from the end of the conversation. */
export function distanceFromBottom(scrollHeight: number, scrollTop: number, clientHeight: number): number {
  return Math.max(0, scrollHeight - scrollTop - clientHeight);
}

/**
 * Whether the view follows new output. Decided from where the reader was
 * (scroll position before the update), so a large chunk of new output never
 * reads as the reader having scrolled away.
 */
export function followsOutput(scrollHeight: number, scrollTop: number, clientHeight: number): boolean {
  return distanceFromBottom(scrollHeight, scrollTop, clientHeight) < STICK_TO_BOTTOM_PX;
}

/**
 * Indexes to mount: the visible range plus messages that must stay mounted
 * wherever the view is (the streaming answer, the one holding focus, the one
 * whose source is open, a go-to or find target). Sorted, unique, in range.
 */
export function mountedIndexes(visible: readonly number[], keep: readonly (number | null | undefined)[], count: number): number[] {
  const set = new Set<number>();
  for (const i of visible) if (i >= 0 && i < count) set.add(i);
  for (const i of keep) if (typeof i === 'number' && i >= 0 && i < count) set.add(i);
  return [...set].sort((a, b) => a - b);
}

/** Fenced blocks drawn as visuals: their source is not text on screen. */
const VISUAL_FENCE = /```(?:chart|mermaid|flowchart|sequence|svg|plot|simulation|figure|diagram|symbols)[^\n]*\n[\s\S]*?```/g;

/**
 * The text a reader sees in a message, near enough for finding it without
 * rendering: markdown syntax, citation markers and the sources of visuals
 * are dropped; code is kept.
 */
export function searchableText(content: string): string {
  return content
    .replace(VISUAL_FENCE, ' ')
    .replace(/^\s*```[^\n]*$/gm, ' ')
    .replace(/【[^】]*】/g, '')
    .replace(/\[(\d+(?:\s*[,–-]\s*\d+)*)\]/g, '')
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/^\s{0,3}#{1,6}\s+/gm, '')
    .replace(/^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?\s*$/gm, ' ')
    .replace(/\|/g, ' ')
    .replace(/(\*\*|__|\*|_|`|~~)/g, '')
    .replace(/^\s*(?:[-*+]|\d+\.)\s+/gm, '')
    .replace(/[ \t]+/g, ' ');
}

/**
 * Lower case with the length kept, so offsets found in the folded text are
 * offsets in the original (a few characters lengthen when lowered).
 */
export function foldCase(text: string): string {
  const lower = text.toLowerCase();
  if (lower.length === text.length) return lower;
  return text.replace(/[\s\S]/g, c => {
    const l = c.toLowerCase();
    return l.length === 1 ? l : c;
  });
}

/** Occurrences of `query` in `text`, ignoring case. */
export function countOccurrences(text: string, query: string): number {
  const needle = foldCase(query);
  if (needle.length === 0) return 0;
  const haystack = foldCase(text);
  let count = 0;
  let at = haystack.indexOf(needle);
  while (at >= 0) {
    count += 1;
    at = haystack.indexOf(needle, at + needle.length);
  }
  return count;
}

/** A message that contains the query, and how often. */
export interface MessageMatch {
  index: number;
  count: number;
}

/** Messages containing `query` (case-insensitive), in conversation order. */
export function findInMessages(texts: readonly string[], query: string): MessageMatch[] {
  const q = query.trim();
  if (q.length === 0) return [];
  const matches: MessageMatch[] = [];
  texts.forEach((text, index) => {
    const count = countOccurrences(text, q);
    if (count > 0) matches.push({ index, count });
  });
  return matches;
}

/** One occurrence: the `match`-th matching message, its `occurrence`-th hit. */
export interface FindCursor {
  match: number;
  occurrence: number;
}

/** Total occurrences across matches. */
export function totalOccurrences(matches: readonly MessageMatch[]): number {
  return matches.reduce((sum, m) => sum + m.count, 0);
}

/** 1-based position of `cursor` among all occurrences (for "3 of 12"). */
export function occurrenceNumber(matches: readonly MessageMatch[], cursor: FindCursor): number {
  let n = 0;
  for (let i = 0; i < cursor.match && i < matches.length; i++) n += matches[i].count;
  return n + cursor.occurrence + 1;
}

/** The next (`step` 1) or previous (`step` -1) occurrence, wrapping around. */
export function stepCursor(matches: readonly MessageMatch[], cursor: FindCursor, step: 1 | -1): FindCursor {
  if (matches.length === 0) return { match: 0, occurrence: 0 };
  const current = matches[Math.min(cursor.match, matches.length - 1)];
  if (step === 1) {
    if (cursor.occurrence + 1 < current.count) return { match: cursor.match, occurrence: cursor.occurrence + 1 };
    return { match: (cursor.match + 1) % matches.length, occurrence: 0 };
  }
  if (cursor.occurrence > 0) return { match: cursor.match, occurrence: cursor.occurrence - 1 };
  const prev = (cursor.match - 1 + matches.length) % matches.length;
  return { match: prev, occurrence: matches[prev].count - 1 };
}

/**
 * Where a new search starts: the first match at or after the first message
 * in view, else the last match (the reader is past every match).
 */
export function startCursor(matches: readonly MessageMatch[], firstVisibleIndex: number): FindCursor {
  if (matches.length === 0) return { match: 0, occurrence: 0 };
  const at = matches.findIndex(m => m.index >= firstVisibleIndex);
  return { match: at >= 0 ? at : matches.length - 1, occurrence: 0 };
}

/** Start and end offsets of each occurrence of `query` in `text`, ignoring case. */
export function occurrenceOffsets(text: string, query: string): Array<[number, number]> {
  const needle = foldCase(query);
  if (needle.length === 0) return [];
  const haystack = foldCase(text);
  const out: Array<[number, number]> = [];
  let at = haystack.indexOf(needle);
  while (at >= 0) {
    out.push([at, at + needle.length]);
    at = haystack.indexOf(needle, at + needle.length);
  }
  return out;
}

/**
 * Shortening Markdown without breaking it: a cut never lands inside a code
 * fence or a math span ($…$, $$…$$, \(…\), \[…\]), so a shortened answer
 * still renders its equations and diagrams, or leaves them out whole.
 *
 * Pure module, unit-tested with Node (`app/tests/focusSummary.test.ts`).
 */

/** A region of the text that must be kept whole or dropped whole. */
export interface ProtectedSpan {
  start: number;
  /** Exclusive end. */
  end: number;
}

const FENCE_OPEN = /^[ \t]{0,3}(`{3,}|~{3,})/;

/** Code fences (``` or ~~~, closed by a run at least as long); an unclosed fence runs to the end. */
function fenceSpans(text: string): ProtectedSpan[] {
  const spans: ProtectedSpan[] = [];
  let offset = 0;
  let open: { start: number; char: string; length: number } | null = null;
  for (const line of text.split('\n')) {
    const lineEnd = offset + line.length;
    const match = FENCE_OPEN.exec(line);
    if (open === null) {
      if (match) open = { start: offset, char: match[1][0], length: match[1].length };
    } else if (match && match[1][0] === open.char && match[1].length >= open.length && line.trim() === match[1]) {
      spans.push({ start: open.start, end: lineEnd });
      open = null;
    }
    offset = lineEnd + 1;
  }
  if (open !== null) spans.push({ start: open.start, end: text.length });
  return spans;
}

function isEscaped(text: string, index: number): boolean {
  let slashes = 0;
  for (let i = index - 1; i >= 0 && text[i] === '\\'; i--) slashes++;
  return slashes % 2 === 1;
}

/** Math spans outside the given fences. Unclosed math runs to the end of the text. */
function mathSpans(text: string, fences: readonly ProtectedSpan[]): ProtectedSpan[] {
  const spans: ProtectedSpan[] = [];
  const inFence = (i: number) => fences.find(f => i >= f.start && i < f.end) ?? null;
  let i = 0;
  while (i < text.length) {
    const fence = inFence(i);
    if (fence) {
      i = fence.end;
      continue;
    }
    const ch = text[i];
    if (ch === '\\' && (text[i + 1] === '[' || text[i + 1] === '(') && !isEscaped(text, i)) {
      const close = text[i + 1] === '[' ? '\\]' : '\\)';
      const end = text.indexOf(close, i + 2);
      const stop = end < 0 ? text.length : end + 2;
      spans.push({ start: i, end: stop });
      i = stop;
      continue;
    }
    if (ch === '$' && !isEscaped(text, i)) {
      if (text[i + 1] === '$') {
        const end = text.indexOf('$$', i + 2);
        const stop = end < 0 ? text.length : end + 2;
        spans.push({ start: i, end: stop });
        i = stop;
        continue;
      }
      // Inline $…$ on one line, not "$5 and $6" (a closing $ follows a non-space).
      const lineEnd = text.indexOf('\n', i);
      const limit = lineEnd < 0 ? text.length : lineEnd;
      let j = i + 1;
      let found = -1;
      if (text[j] !== ' ' && j < limit) {
        for (; j < limit; j++) {
          if (text[j] === '$' && !isEscaped(text, j) && text[j - 1] !== ' ') {
            found = j;
            break;
          }
        }
      }
      if (found > i + 1) {
        spans.push({ start: i, end: found + 1 });
        i = found + 1;
        continue;
      }
    }
    i++;
  }
  return spans;
}

/** Every region a cut must not enter, in order. */
export function protectedSpans(text: string): ProtectedSpan[] {
  const fences = fenceSpans(text);
  return [...fences, ...mathSpans(text, fences)].sort((a, b) => a.start - b.start);
}

function isHighSurrogate(code: number): boolean {
  return code >= 0xd800 && code <= 0xdbff;
}

/**
 * At most `max` characters (UTF-16 units) of `text`, ending with "…" when
 * shortened. The cut moves back to the start of any fence or math span it
 * would split, then to the last paragraph, line or word break when one is
 * close enough, so the result is valid Markdown.
 */
export function safeTruncate(text: string, max: number): string {
  if (max <= 0) return '';
  if (text.length <= max) return text;
  const limit = Math.max(0, max - 1); // room for the ellipsis
  const spans = protectedSpans(text);
  const inside = (at: number) => spans.find(s => at > s.start && at < s.end) ?? null;
  let cut = limit;
  const split = inside(cut);
  if (split) cut = split.start;
  if (cut > 0 && isHighSurrogate(text.charCodeAt(cut - 1))) cut -= 1;
  // Prefer a natural break, if it keeps most of the text; a break inside a
  // span (a space in an equation) falls back to the start of that span.
  const head = text.slice(0, cut);
  const floor = Math.floor(cut * 0.6);
  const breaks = [head.lastIndexOf('\n\n'), head.lastIndexOf('\n'), head.search(/\s\S*$/)];
  for (const at of breaks) {
    if (at >= floor && at > 0) {
      cut = inside(at)?.start ?? at;
      break;
    }
  }
  const kept = text.slice(0, cut).trimEnd();
  return kept ? `${kept}…` : '';
}

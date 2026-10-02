/**
 * Fuzzy location of a retrieved passage inside a document's text.
 *
 * The passage the index stored and the text the viewer shows come from
 * different extractors (pdf_extract vs pdf.js, docx XML vs rendered text), so
 * they differ in whitespace, line breaks, hyphenation, ligatures and reading
 * order. Both sides are therefore reduced to a stream of case-folded letters
 * and digits (NFKD with combining marks dropped, so ligatures, full-width
 * forms and composed vs decomposed accents all compare equal), with every
 * normalized unit mapped back to its original UTF-16 offset. Matching then finds the longest common run of
 * the passage in the document (suffix automaton, linear time) and recursively
 * aligns the remaining passage text on either side of it, producing one or
 * more matched blocks. Blocks are reported separately (not as one span from
 * first to last) so a passage whose reading order differs from the page
 * layout (multi-column PDFs) never highlights the neighbouring column.
 */

/** A document range in original UTF-16 offsets, end exclusive. */
export interface TextRange {
  start: number;
  end: number;
}

export interface PassageMatch {
  /** Matched ranges in document order, in original haystack offsets. */
  ranges: TextRange[];
  /** Fraction of the normalized passage that was matched (0..1). */
  coverage: number;
  /** Length of the longest single matched run (normalized units). */
  longestRun: number;
  /** True when only the opening of the passage could be located. */
  partial: boolean;
}

interface NormalizedText {
  /** Normalized UTF-16 units. */
  units: string;
  /** Original start offset of each normalized unit. */
  starts: Int32Array;
  /** Original end offset (exclusive) of each normalized unit. */
  ends: Int32Array;
}

const WORD_CHAR = /[\p{L}\p{N}]/u;

/** Minimum length of a secondary aligned block. */
const MIN_BLOCK = 10;
/** Minimum longest run for a passage to count as found. */
const MIN_ANCHOR = 24;
/** Opening of the passage used as a fallback needle. */
const FALLBACK_PREFIX = 120;
/** Matched blocks separated by at most this many normalized units merge. */
const MERGE_GAP = 24;
const MAX_BLOCKS = 256;

/** Reduce text to case-folded word characters with an offset map back. */
export function normalizeForMatch(text: string): NormalizedText {
  const units: string[] = [];
  const starts: number[] = [];
  const ends: number[] = [];
  let i = 0;
  while (i < text.length) {
    const code = text.charCodeAt(i);
    if (code < 0x80) {
      // ASCII fast path.
      if ((code >= 48 && code <= 57) || (code >= 97 && code <= 122)) {
        units.push(text[i]);
        starts.push(i);
        ends.push(i + 1);
      } else if (code >= 65 && code <= 90) {
        units.push(String.fromCharCode(code + 32));
        starts.push(i);
        ends.push(i + 1);
      }
      i += 1;
      continue;
    }
    const cp = text.codePointAt(i) ?? code;
    const width = cp > 0xffff ? 2 : 1;
    const folded = String.fromCodePoint(cp).normalize('NFKD').toLowerCase();
    for (const ch of folded) {
      if (!WORD_CHAR.test(ch)) continue;
      for (let k = 0; k < ch.length; k += 1) {
        units.push(ch[k]);
        starts.push(i);
        ends.push(i + width);
      }
    }
    i += width;
  }
  return { units: units.join(''), starts: Int32Array.from(starts), ends: Int32Array.from(ends) };
}

/** Suffix automaton of a needle, tracking the first end position per state. */
class SuffixAutomaton {
  private readonly len: number[] = [0];
  private readonly link: number[] = [-1];
  private readonly firstEnd: number[] = [-1];
  private readonly next: Map<number, number>[] = [new Map()];

  constructor(text: string) {
    let last = 0;
    for (let pos = 0; pos < text.length; pos += 1) {
      const c = text.charCodeAt(pos);
      const cur = this.addState(this.len[last] + 1, pos);
      let p = last;
      while (p !== -1 && !this.next[p].has(c)) {
        this.next[p].set(c, cur);
        p = this.link[p];
      }
      if (p === -1) {
        this.link[cur] = 0;
      } else {
        const q = this.next[p].get(c) as number;
        if (this.len[p] + 1 === this.len[q]) {
          this.link[cur] = q;
        } else {
          const clone = this.addState(this.len[p] + 1, this.firstEnd[q]);
          this.next[clone] = new Map(this.next[q]);
          this.link[clone] = this.link[q];
          while (p !== -1 && this.next[p].get(c) === q) {
            this.next[p].set(c, clone);
            p = this.link[p];
          }
          this.link[q] = clone;
          this.link[cur] = clone;
        }
      }
      last = cur;
    }
  }

  private addState(length: number, firstEnd: number): number {
    this.len.push(length);
    this.link.push(-1);
    this.firstEnd.push(firstEnd);
    this.next.push(new Map());
    return this.len.length - 1;
  }

  /**
   * Longest substring of the needle occurring in `hay[from, to)`.
   * Returns its length, start in the haystack and start in the needle.
   */
  longestIn(hay: string, from: number, to: number): { length: number; hayStart: number; needleStart: number } {
    let state = 0;
    let length = 0;
    let best = { length: 0, hayStart: -1, needleStart: -1 };
    for (let pos = from; pos < to; pos += 1) {
      const c = hay.charCodeAt(pos);
      while (state !== 0 && !this.next[state].has(c)) {
        state = this.link[state];
        length = this.len[state];
      }
      const target = this.next[state].get(c);
      if (target === undefined) {
        state = 0;
        length = 0;
        continue;
      }
      state = target;
      length += 1;
      if (length > best.length) {
        best = {
          length,
          hayStart: pos - length + 1,
          needleStart: this.firstEnd[state] - length + 1,
        };
      }
    }
    return best;
  }
}

interface Block {
  hay: number;
  needle: number;
  length: number;
}

function alignBlocks(
  needle: string,
  nLo: number,
  nHi: number,
  hay: string,
  hLo: number,
  hHi: number,
  minLength: number,
  out: Block[],
): void {
  if (out.length >= MAX_BLOCKS) return;
  if (nHi - nLo < minLength || hHi - hLo < minLength) return;
  const sam = new SuffixAutomaton(needle.slice(nLo, nHi));
  const found = sam.longestIn(hay, hLo, hHi);
  if (found.length < minLength) return;
  const needleStart = nLo + found.needleStart;
  const hayStart = found.hayStart;

  // Text before the run must sit before it in the document, within a window
  // proportional to its length (tolerates inserted headers/footers).
  const leftLength = needleStart - nLo;
  if (leftLength >= MIN_BLOCK) {
    alignBlocks(needle, nLo, needleStart, hay, Math.max(hLo, hayStart - leftLength * 2 - 64), hayStart, MIN_BLOCK, out);
  }
  out.push({ hay: hayStart, needle: needleStart, length: found.length });
  const rightStart = needleStart + found.length;
  const rightLength = nHi - rightStart;
  if (rightLength >= MIN_BLOCK) {
    const hayRight = hayStart + found.length;
    alignBlocks(needle, rightStart, nHi, hay, hayRight, Math.min(hHi, hayRight + rightLength * 2 + 64), MIN_BLOCK, out);
  }
}

/** Haystack prepared once and matched against many passages. */
export interface PreparedHaystack {
  readonly normalized: NormalizedText;
}

export function prepareHaystack(text: string): PreparedHaystack {
  return { normalized: normalizeForMatch(text) };
}

function matchNormalized(needle: string, hay: NormalizedText, partial: boolean): PassageMatch | null {
  if (needle.length === 0 || hay.units.length === 0) return null;
  // A very short passage must match in full; otherwise require a solid run.
  const minAnchor = Math.min(MIN_ANCHOR, needle.length);
  const blocks: Block[] = [];
  alignBlocks(needle, 0, needle.length, hay.units, 0, hay.units.length, minAnchor, blocks);
  if (blocks.length === 0) return null;

  const longestRun = blocks.reduce((m, b) => Math.max(m, b.length), 0);
  const matched = blocks.reduce((sum, b) => sum + b.length, 0);
  const coverage = matched / needle.length;
  // Accept a long anchor outright; a shorter one needs the rest of the
  // passage to line up around it.
  if (longestRun < 60 && coverage < 0.4) return null;

  const sorted = [...blocks].sort((a, b) => a.hay - b.hay);
  const merged: { start: number; end: number }[] = [];
  for (const block of sorted) {
    const prev = merged[merged.length - 1];
    if (prev && block.hay - prev.end <= MERGE_GAP) {
      prev.end = Math.max(prev.end, block.hay + block.length);
    } else {
      merged.push({ start: block.hay, end: block.hay + block.length });
    }
  }
  const ranges = merged.map(r => ({ start: hay.starts[r.start], end: hay.ends[r.end - 1] }));
  return { ranges, coverage, longestRun, partial };
}

/**
 * Locate `passage` in a prepared haystack. Tries the whole passage first and
 * falls back to its opening ~120 characters. Returns null when neither yields
 * a confident match.
 */
export function findPassage(passage: string, haystack: PreparedHaystack): PassageMatch | null {
  const needle = normalizeForMatch(passage).units;
  if (needle.length === 0) return null;
  const full = matchNormalized(needle, haystack.normalized, false);
  if (full) return full;
  if (needle.length > FALLBACK_PREFIX) {
    return matchNormalized(needle.slice(0, FALLBACK_PREFIX), haystack.normalized, true);
  }
  return null;
}

/** Score used to rank candidate pages: matched share, then run length. */
export function matchScore(match: PassageMatch): number {
  return match.coverage * 1000 + Math.min(match.longestRun, 999);
}

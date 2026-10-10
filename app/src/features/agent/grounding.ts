/**
 * Grounding in the transcript: citation markers, claim flags and the
 * grounding summary. Pure, no React, so it is unit-tested with Node
 * (`app/tests/grounding.test.ts`).
 *
 * The citation grammar mirrors `crates/shodh-rag/src/harness/grounding/citations.rs`;
 * both are tested against `harness/fixtures/citations.json`, so a pill and a
 * verifier flag never disagree about what `[…]` means.
 */

import type { ClaimCheck, ClaimOutcome, GroundingReport, GroundingSummary, NeedCheck } from './events';

/** Widest range expanded into citations (`[1-10]`); wider ones are prose. */
export const MAX_RANGE_SPAN = 10;

export interface CitationMarker {
  start: number;
  end: number;
  numbers: number[];
}

const BRACKET = /\[(?:document\s+)?(\d{1,6}(?:\s*[-–]\s*\d{1,6})?(?:\s*,\s*(?:document\s+)?\d{1,6}(?:\s*[-–]\s*\d{1,6})?)*)\]/gi;
const LENTICULAR = /【(\d{1,6})†[^】]*】/g;

/** Numbers of a bracket group's inside, or null when it is not a citation. */
function groupNumbers(inner: string): number[] | null {
  const out: number[] = [];
  for (const raw of inner.split(',')) {
    const part = raw.trim().replace(/^document\s+/i, '');
    const range = /^(\d+)\s*[-–]\s*(\d+)$/.exec(part);
    const from = range ? Number(range[1]) : Number(part);
    const to = range ? Number(range[2]) : from;
    if (!Number.isInteger(from) || !Number.isInteger(to)) return null;
    if (from === 0 || to < from || to - from + 1 > MAX_RANGE_SPAN) return null;
    // Citations increase; "[4, 9, 1]" is a tuple.
    if (out.length > 0 && from <= out[out.length - 1]) return null;
    for (let n = from; n <= to; n++) out.push(n);
  }
  return out.length > 0 ? out : null;
}

/** Character ranges of fenced and inline code, where brackets are code. */
export function codeSpans(text: string): Array<[number, number]> {
  const spans: Array<[number, number]> = [];
  let offset = 0;
  let fence: { start: number; marker: string } | null = null;
  for (const line of text.split(/(?<=\n)/)) {
    const trimmed = line.trimStart();
    if (fence) {
      if (trimmed.startsWith(fence.marker)) {
        spans.push([fence.start, offset + line.length]);
        fence = null;
      }
    } else if (trimmed.startsWith('```')) {
      fence = { start: offset, marker: '```' };
    } else if (trimmed.startsWith('~~~')) {
      fence = { start: offset, marker: '~~~' };
    }
    offset += line.length;
  }
  if (fence) spans.push([fence.start, text.length]);
  let i = 0;
  while (i < text.length) {
    const inFence = spans.find(([s, e]) => i >= s && i < e);
    if (inFence) {
      i = inFence[1];
      continue;
    }
    if (text[i] === '`') {
      const runStart = i;
      while (i < text.length && text[i] === '`') i++;
      const ticks = text.slice(runStart, i);
      const close = text.indexOf(ticks, i);
      if (close >= 0) {
        spans.push([runStart, close + ticks.length]);
        i = close + ticks.length;
      }
      continue;
    }
    i++;
  }
  return spans.sort((a, b) => a[0] - b[0]);
}

function inside(spans: Array<[number, number]>, at: number): boolean {
  return spans.some(([s, e]) => at >= s && at < e);
}

/** Every citation marker of `text` outside code, in order. */
export function parseCitationMarkers(text: string): CitationMarker[] {
  const code = codeSpans(text);
  const out: CitationMarker[] = [];
  for (const m of text.matchAll(BRACKET)) {
    const start = m.index ?? 0;
    const end = start + m[0].length;
    if (inside(code, start) || text[end] === '(') continue;
    const numbers = groupNumbers(m[1]);
    if (numbers) out.push({ start, end, numbers });
  }
  for (const m of text.matchAll(LENTICULAR)) {
    const start = m.index ?? 0;
    if (inside(code, start)) continue;
    const n = Number(m[1]);
    if (n > 0) out.push({ start, end: start + m[0].length, numbers: [n] });
  }
  return out.sort((a, b) => a.start - b.start);
}

/** Every number `text` cites. */
export function citedNumbersIn(text: string): Set<number> {
  const out = new Set<number>();
  for (const m of parseCitationMarkers(text)) for (const n of m.numbers) out.add(n);
  return out;
}

/** Outcomes shown as a flag in the answer. */
export function isFlagged(outcome: ClaimOutcome): boolean {
  return outcome === 'unsupported' || outcome === 'uncited_factual' || outcome === 'invalid_citation';
}

/** Short visible text of a claim's flag. */
export function flagLabel(check: ClaimCheck): string {
  switch (check.outcome) {
    case 'invalid_citation':
      return check.invalid.length === 1
        ? `source [${check.invalid[0]}] does not exist`
        : `sources ${check.invalid.map(n => `[${n}]`).join('')} do not exist`;
    case 'uncited_factual':
      return 'no source';
    case 'unsupported':
      return check.missingNumbers.length > 0
        ? `${check.missingNumbers.join(', ')} not in the cited source`
        : 'not found in the cited source';
    case 'weak':
      return 'partly supported';
    default:
      return '';
  }
}

/** Full sentence for tooltips and screen readers. */
export function flagDescription(check: ClaimCheck, closestLabel: string | null): string {
  const what = (() => {
    switch (check.outcome) {
      case 'invalid_citation':
        return `This statement cites ${check.invalid.map(n => `[${n}]`).join('')}, which is not one of this answer's sources.`;
      case 'uncited_factual':
        return 'This statement has no source.';
      case 'unsupported':
        return check.missingNumbers.length > 0
          ? `The cited source does not contain ${check.missingNumbers.join(', ')}.`
          : 'This statement was not found in the cited source.';
      case 'weak':
        return 'The cited source supports this statement only in part.';
      default:
        return '';
    }
  })();
  return closestLabel ? `${what} Closest passage: ${closestLabel}.` : what;
}

/** Placeholder for a claim flag; survives markdown parsing like citation placeholders. */
export const FLAG_OPEN = 'XFSHODH';
export const FLAG_CLOSE = 'XGSHODH';
export const FLAG_PATTERN = new RegExp(`${FLAG_OPEN}(\\d+)${FLAG_CLOSE}`, 'g');

/**
 * Insert a flag placeholder after each flagged claim of `content`. Claims are
 * found by their anchor text, in order (never by offset). Returns the text and
 * the indexes (into `checks`) of the flags placed.
 */
export function insertFlagMarkers(content: string, checks: readonly ClaimCheck[]): { text: string; placed: number[] } {
  const inserts: Array<{ at: number; index: number }> = [];
  let cursor = 0;
  checks.forEach((check, index) => {
    if (!isFlagged(check.outcome) || check.anchor.length === 0) return;
    let at = content.indexOf(check.anchor, cursor);
    if (at < 0) at = content.indexOf(check.anchor);
    if (at < 0) return;
    const end = at + check.anchor.length;
    inserts.push({ at: end, index });
    cursor = end;
  });
  inserts.sort((a, b) => a.at - b.at);
  let text = '';
  let last = 0;
  for (const { at, index } of inserts) {
    text += content.slice(last, at) + `${FLAG_OPEN}${index}${FLAG_CLOSE}`;
    last = at;
  }
  text += content.slice(last);
  return { text, placed: inserts.map(i => i.index) };
}

/** The report that describes the answer: the final one, else the latest. */
export function answerReport(reports: readonly GroundingReport[]): GroundingReport | null {
  if (reports.length === 0) return null;
  return reports.find(r => r.isFinal) ?? reports[reports.length - 1];
}

/** Text blocks replaced by a revised answer. */
export function supersededMessages(reports: readonly GroundingReport[]): Set<string> {
  const out = new Set<string>();
  for (const r of reports) for (const id of r.supersededMessageIds) out.add(id);
  return out;
}

/** Checks of the report that belong to one text block. */
export function checksForMessage(report: GroundingReport | null, messageId: string): ClaimCheck[] {
  return report ? report.claims.filter(c => c.messageId === messageId) : [];
}

/** "Grounded 14/16": supported and weak claims of the checkable ones. */
export function summaryLabel(summary: GroundingSummary): string {
  const checkable = summary.checked - summary.unchecked;
  return `Grounded ${summary.supported + summary.weak}/${checkable}`;
}

/** Spoken summary for the chip. */
export function summaryDescription(summary: GroundingSummary): string {
  const flagged = summary.unsupported + summary.uncited + summary.invalid;
  const parts = [`${summary.supported} of ${summary.checked - summary.unchecked} checked statements supported by their sources`];
  if (summary.weak > 0) parts.push(`${summary.weak} partly supported`);
  if (flagged > 0) parts.push(`${flagged} flagged`);
  if (summary.unchecked > 0) parts.push(`${summary.unchecked} could not be checked`);
  return parts.join(', ');
}

/** Tone of the chip: all supported, some partly, or some flagged. */
export function summaryTone(summary: GroundingSummary): 'ok' | 'partial' | 'flagged' {
  if (summary.unsupported + summary.uncited + summary.invalid > 0) return 'flagged';
  if (summary.weak > 0) return 'partial';
  return 'ok';
}

/** Needs the report found no passage for. */
export function missingNeeds(report: GroundingReport | null): NeedCheck[] {
  return report ? report.needs.filter(n => n.state === 'missing') : [];
}

/** Human name of the scoring method, for the chip's details. */
export function methodLabel(method: GroundingReport['method']): string {
  switch (method) {
    case 'entailment':
      return 'checked with the local entailment model and number checks';
    case 'cross_encoder':
      return 'checked for topic and numbers with the local reranker (install the answer checking model for full checks)';
    default:
      return 'checked by word overlap and numbers only (search models not installed)';
  }
}

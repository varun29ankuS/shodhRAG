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
import type { TranscriptBlock, TranscriptState } from './reducer';

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
  return (
    outcome === 'unsupported' ||
    outcome === 'uncited_factual' ||
    outcome === 'invalid_citation' ||
    outcome === 'unverified_absence'
  );
}

/**
 * The passage the check found supporting an unsupported claim that cites another one (a
 * miscitation), or null. Reports saved before the check named it have none.
 */
export function suggestedPassage(check: ClaimCheck): number | null {
  const n = check.suggested;
  if (check.outcome !== 'unsupported' || typeof n !== 'number' || !Number.isInteger(n) || n <= 0) return null;
  return check.cited.includes(n) ? null : n;
}

/** "3" or "3, 7": the numbers a claim cites, as the flag says them. */
function citedList(check: ClaimCheck): string {
  return check.cited.join(', ');
}

/** Short visible text of a claim's flag. */
export function flagLabel(check: ClaimCheck): string {
  const suggested = suggestedPassage(check);
  if (suggested !== null) return `Supported by passage ${suggested} (answer cited ${citedList(check)})`;
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
    case 'unverified_absence':
      return 'not looked up';
    default:
      return '';
  }
}

/** Full sentence for tooltips and screen readers. */
export function flagDescription(check: ClaimCheck, closestLabel: string | null): string {
  const suggested = suggestedPassage(check);
  const what = (() => {
    if (suggested !== null) {
      return `Passage ${suggested} supports this statement; the answer cited ${check.cited.map(n => `[${n}]`).join('')}, which does not.`;
    }
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
      case 'unverified_absence':
        return 'This says you have none of something, but nothing in this answer looked it up.';
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

/** A claim whose citation the user moved to the suggested passage ("Cite N instead"). */
export function isCorrected(check: ClaimCheck): boolean {
  return Array.isArray(check.correctedFrom) && check.correctedFrom.length > 0;
}

/** What a corrected claim's note says: the change is the user's, not the model's. */
export function correctionLabel(check: ClaimCheck): string {
  const from = (check.correctedFrom ?? []).map(n => `[${n}]`).join('');
  const to = check.cited.map(n => `[${n}]`).join('');
  return `citation corrected by you (${from} → ${to})`;
}

/**
 * Insert a flag placeholder after each flagged claim of `content`, and after each claim whose
 * citation the user corrected (its note). Claims are found by their anchor text, in order
 * (never by offset). Returns the text and the indexes (into `checks`) of the flags placed.
 */
export function insertFlagMarkers(content: string, checks: readonly ClaimCheck[]): { text: string; placed: number[] } {
  const inserts: Array<{ at: number; index: number }> = [];
  let cursor = 0;
  checks.forEach((check, index) => {
    if (!(isFlagged(check.outcome) || isCorrected(check)) || check.anchor.length === 0) return;
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

/**
 * Where each claim's anchor is in `content`, in reading order (never by offset: Rust byte
 * offsets and JavaScript indexes differ); null for a claim whose anchor is not in it.
 */
function anchorOffsets(content: string, checks: readonly ClaimCheck[]): Array<number | null> {
  let cursor = 0;
  return checks.map(check => {
    if (check.anchor.length === 0) return null;
    let at = content.indexOf(check.anchor, cursor);
    if (at < 0) at = content.indexOf(check.anchor);
    if (at < 0) return null;
    cursor = at + check.anchor.length;
    return at;
  });
}

/** A marker's span widened over the spaces before it, so "latency [3]." reads "latency.". */
function withLeadingSpace(text: string, start: number, end: number): [number, number] {
  let from = start;
  while (from > 0 && (text[from - 1] === ' ' || text[from - 1] === '\t')) from--;
  return [from, end];
}

/** `text` without the character ranges `cuts` (any order; a range inside an earlier one is skipped). */
function cutRanges(text: string, cuts: ReadonlyArray<[number, number]>): string {
  const sorted = [...cuts].sort((a, b) => a[0] - b[0]);
  let out = '';
  let last = 0;
  for (const [from, to] of sorted) {
    if (from < last) continue;
    out += text.slice(last, from);
    last = to;
  }
  return out + text.slice(last);
}

/** Outcomes whose citation markers are not passed on as context. */
function citesWrongly(outcome: ClaimOutcome): boolean {
  return outcome === 'unsupported' || outcome === 'invalid_citation';
}

/**
 * The claims that inherited citations from the markers of `claims[index]`: claims of the same
 * text block with no marker of their own that cite a number those markers cite. The verifier
 * gives an uncited sentence the citations of the next cited one in its paragraph, and list
 * items and table rows those of their list (`claims.rs` `inherit_citations`), so one marker
 * can stand for several claims.
 */
function inheritorsOf(claims: readonly ClaimCheck[], index: number, numbers: ReadonlySet<number>): number[] {
  const owner = claims[index];
  const out: number[] = [];
  claims.forEach((c, i) => {
    if (i === index || c.messageId !== owner.messageId) return;
    if (parseCitationMarkers(c.anchor).length > 0) return;
    if (c.cited.some(n => numbers.has(n))) out.push(i);
  });
  return out;
}

/**
 * An answer as context for a later question (wherever an answer travels on, `answerAsContext`):
 * the citation markers of claims the check found unsupported by what they cite, or citing a
 * passage that does not exist, are removed, so a wrong citation is not handed on as if it
 * were right; the model then picks the passage from the ones it is given. Supported, partly
 * supported and unchecked claims keep their markers. Only markers inside a flagged claim's
 * own text are removed, and a marker another claim inherited and was not flagged for stays:
 * it is that claim's citation too.
 */
export function withoutFlaggedCitations(content: string, report: GroundingReport | null): string {
  if (!report || report.claims.length === 0) return content;
  const claims = report.claims;
  const offsets = anchorOffsets(content, claims);
  const cuts: Array<[number, number]> = [];
  claims.forEach((check, i) => {
    const at = offsets[i];
    if (at === null || !citesWrongly(check.outcome)) return;
    for (const m of parseCitationMarkers(check.anchor)) {
      const numbers = new Set(m.numbers);
      const shared = inheritorsOf(claims, i, numbers).some(j => !citesWrongly(claims[j].outcome));
      if (!shared) cuts.push(withLeadingSpace(content, at + m.start, at + m.end));
    }
  });
  return cuts.length > 0 ? cutRanges(content, cuts) : content;
}

/** The report that describes a stored answer (a side answer's persisted transcript), or null. */
export function storedAnswerReport(transcript: unknown): GroundingReport | null {
  if (typeof transcript !== 'object' || transcript === null) return null;
  const groundings = (transcript as { groundings?: unknown }).groundings;
  if (!Array.isArray(groundings)) return null;
  const reports = groundings.filter(
    (r): r is GroundingReport => typeof r === 'object' && r !== null && Array.isArray((r as GroundingReport).claims),
  );
  return answerReport(reports);
}

/**
 * A stored answer as context for a later question, wherever an answer travels on (the outer
 * levels of a nested question, a side thread's replayed history, the main conversation replayed
 * into a side session, a discussion summarised to be brought back): `withoutFlaggedCitations`
 * with the final report of its stored transcript. An answer without one is passed on as written.
 */
export function answerAsContext(content: string, transcript: unknown): string {
  return withoutFlaggedCitations(content, storedAnswerReport(transcript));
}

/** Trailing sentence punctuation of an anchor, with closing quotes and emphasis. */
const ANCHOR_END = /[.!?。;:]+["'”’)*_]*\s*$/;

/**
 * `anchor` citing passage `n` instead of what it cites: its first citation marker becomes
 * `[n]` and its other markers go (with the separators between them); an anchor without a
 * marker (its citation was inherited from the paragraph) gets `[n]` before its full stop.
 */
export function citeInAnchor(anchor: string, n: number): string {
  const markers = parseCitationMarkers(anchor);
  if (markers.length === 0) return withMarkerAtEnd(anchor, [n]);
  let out = `${anchor.slice(0, markers[0].start)}[${n}]`;
  let last = markers[0].end;
  for (let i = 1; i < markers.length; i++) {
    const gap = anchor.slice(last, markers[i].start);
    // "[3], [7]" or "[3][7]": a run of markers becomes the one marker.
    if (!/^[\s,;]*$/.test(gap)) out += gap.replace(/[ \t]+$/, '');
    last = markers[i].end;
  }
  return out + anchor.slice(last);
}

/** `anchor` with a marker citing `numbers` (increasing) put before its full stop. */
function withMarkerAtEnd(anchor: string, numbers: readonly number[]): string {
  const end = ANCHOR_END.exec(anchor);
  const at = end ? end.index : anchor.trimEnd().length;
  return `${anchor.slice(0, at).trimEnd()} [${numbers.join(', ')}]${anchor.slice(at)}`;
}

/**
 * The passage "Cite N instead" puts in a claim's place, or null: a miscitation whose citations
 * can be rewritten where the claim is written. A table row's anchor is its last cell, so a row
 * whose citations sit in other cells is not rewritten (it would then cite both).
 */
export function citeInsteadOffer(check: ClaimCheck): number | null {
  const n = suggestedPassage(check);
  if (n === null) return null;
  if (check.kind === 'table_row') {
    const inAnchor = citedNumbersIn(check.anchor);
    if (!check.cited.every(c => inAnchor.has(c))) return null;
  }
  return n;
}

/** Counts and score of a set of checked claims (mirrors `verify::summarise`). */
export function summariseClaims(claims: readonly ClaimCheck[]): GroundingSummary {
  const s: GroundingSummary = { checked: claims.length, supported: 0, weak: 0, unsupported: 0, uncited: 0, invalid: 0, unchecked: 0, score: null };
  for (const c of claims) {
    switch (c.outcome) {
      case 'supported':
        s.supported += 1;
        break;
      case 'weak':
        s.weak += 1;
        break;
      case 'unsupported':
      case 'unverified_absence':
        s.unsupported += 1;
        break;
      case 'uncited_factual':
        s.uncited += 1;
        break;
      case 'invalid_citation':
        s.invalid += 1;
        break;
      default:
        s.unchecked += 1;
    }
  }
  const scored = s.checked - s.unchecked;
  s.score = scored > 0 ? (s.supported + 0.5 * s.weak) / scored : null;
  return s;
}

/** What "Cite N instead" changed, for the transcript and the audit log. */
export interface CitationRewrite {
  transcript: TranscriptState;
  /** The claim's position among the claims of the answer's final report. */
  claimIndex: number;
  /** The text block the claim is in. */
  messageId: string;
  from: number[];
  to: number;
  suggestedScore: number | null;
  summary: GroundingSummary;
}

/**
 * A finished answer with one miscited claim citing the passage the check found supporting it
 * (`citeInsteadOffer`), as the user asked with "Cite N instead":
 * - the claim's markers become `[N]`, and it is recorded as supported by that passage with the
 *   support and contradiction the check measured there (none in reports saved before they
 *   were kept), and `correctedFrom` says the change is the user's;
 * - claims that inherited the old marker and were not flagged for it (`inheritorsOf`) get it
 *   written into their own text, so they keep the citation the check supported them with;
 * - the final report's summary is recomputed.
 * Null while the answer is running, and when the claim, its text block or its anchor is no
 * longer there.
 */
export function citeSuggested(state: TranscriptState, check: ClaimCheck): CitationRewrite | null {
  if (state.status === 'starting' || state.status === 'running') return null;
  const n = citeInsteadOffer(check);
  const report = answerReport(state.groundings);
  if (n === null || !report) return null;
  const reportIndex = state.groundings.indexOf(report);
  // The check the flag was drawn from; a copy is matched by its fields.
  const same = report.claims.indexOf(check);
  const claimIndex = same >= 0
    ? same
    : report.claims.findIndex(
        c => c.messageId === check.messageId && c.anchor === check.anchor && c.text === check.text && c.outcome === check.outcome,
      );
  const blockIndex = state.blocks.findIndex(b => b.kind === 'text' && b.id === check.messageId);
  const block = blockIndex >= 0 ? state.blocks[blockIndex] : null;
  if (claimIndex < 0 || !block || block.kind !== 'text') return null;
  // The places of the block's claims, found the way their flags are.
  const own = report.claims.map((c, i) => ({ c, i })).filter(({ c }) => c.messageId === check.messageId);
  const offsets = anchorOffsets(block.text, own.map(({ c }) => c));
  const offsetOf = (i: number): number | null => offsets[own.findIndex(o => o.i === i)] ?? null;
  const at = offsetOf(claimIndex);
  if (at === null) return null;

  const oldNumbers = citedNumbersIn(check.anchor);
  const anchors = new Map<number, string>([[claimIndex, citeInAnchor(check.anchor, n)]]);
  for (const j of inheritorsOf(report.claims, claimIndex, oldNumbers)) {
    const c = report.claims[j];
    if (citesWrongly(c.outcome) || offsetOf(j) === null) continue;
    const kept = c.cited.filter(x => oldNumbers.has(x)).sort((a, b) => a - b);
    anchors.set(j, withMarkerAtEnd(c.anchor, kept));
  }
  // Rewrite from the end of the block so earlier offsets stay valid.
  let text = block.text;
  const edits = [...anchors.entries()]
    .map(([i, anchor]) => ({ at: offsetOf(i) as number, from: report.claims[i].anchor, to: anchor }))
    .sort((a, b) => b.at - a.at);
  for (const e of edits) text = text.slice(0, e.at) + e.to + text.slice(e.at + e.from.length);

  const claims: ClaimCheck[] = report.claims.map((c, i) => {
    const anchor = anchors.get(i);
    if (anchor === undefined) return c;
    if (i !== claimIndex) return { ...c, anchor };
    return {
      ...c,
      anchor,
      cited: [n],
      outcome: 'supported' as const,
      support: typeof c.suggestedScore === 'number' ? c.suggestedScore : null,
      contradiction: typeof c.suggestedContradiction === 'number' ? c.suggestedContradiction : null,
      missingNumbers: [],
      closest: null,
      closestScore: null,
      suggested: null,
      suggestedScore: null,
      suggestedContradiction: null,
      correctedFrom: [...c.cited],
    };
  });
  const summary = summariseClaims(claims);
  const groundings = state.groundings.map((r, i) => (i === reportIndex ? { ...r, claims, summary } : r));
  const rewritten: TranscriptBlock = { kind: 'text', id: block.id, text };
  const blocks: TranscriptBlock[] = state.blocks.map((b, i) => (i === blockIndex ? rewritten : b));
  return {
    transcript: { ...state, blocks, groundings },
    claimIndex,
    messageId: check.messageId,
    from: [...check.cited],
    to: n,
    suggestedScore: typeof check.suggestedScore === 'number' ? check.suggestedScore : null,
    summary,
  };
}

/** The audit record of a correction (`audit_record_citation_correction`): ids, numbers and counts, never text. */
export interface CitationCorrection {
  conversationId: string;
  runId: string | null;
  messageId: string;
  claimIndex: number;
  from: number[];
  to: number;
  suggestedScore: number | null;
  summary: GroundingSummary;
}

/** What the audit log keeps of a rewrite. */
export function citationCorrection(rewrite: CitationRewrite, conversationId: string): CitationCorrection {
  const s = rewrite.summary;
  return {
    conversationId,
    runId: rewrite.transcript.runId || null,
    messageId: rewrite.messageId,
    claimIndex: rewrite.claimIndex,
    from: [...rewrite.from],
    to: rewrite.to,
    suggestedScore: rewrite.suggestedScore,
    summary: {
      checked: s.checked,
      supported: s.supported,
      weak: s.weak,
      unsupported: s.unsupported,
      uncited: s.uncited,
      invalid: s.invalid,
      unchecked: s.unchecked,
      score: s.score,
    },
  };
}

/** How a "Cite N instead" ended. */
export type CorrectionOutcome =
  /** Recorded in the audit log, then applied and saved. */
  | 'applied'
  /** The claim is no longer in the answer as it was checked: nothing to change. */
  | 'stale'
  /** The audit log refused or could not take the record: nothing was changed. */
  | 'not_recorded';

/** The notice for a correction that could not be recorded. */
export const CORRECTION_NOT_RECORDED = 'Could not record this correction in Activity, so nothing was changed.';

/**
 * Audit first: the correction is applied (and saved) only once the audit log has taken its
 * record, so the answer never changes without a record of who changed it. A failed record
 * applies nothing.
 */
export async function applyAuditedCorrection(record: () => Promise<unknown>, apply: () => void): Promise<CorrectionOutcome> {
  try {
    await record();
  } catch {
    return 'not_recorded';
  }
  apply();
  return 'applied';
}

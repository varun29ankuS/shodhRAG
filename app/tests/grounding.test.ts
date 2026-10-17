/**
 * Grounding in the transcript: the shared citation grammar, claim flags and
 * the grounding summary. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/grounding.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import type { ClaimCheck, GroundingReport, GroundingSummary } from '../src/features/agent/events.ts';
import {
  FLAG_CLOSE,
  FLAG_OPEN,
  answerReport,
  checksForMessage,
  citeInAnchor,
  citeInsteadOffer,
  citeSuggested,
  citedNumbersIn,
  flagDescription,
  flagLabel,
  insertFlagMarkers,
  isFlagged,
  parseCitationMarkers,
  summaryDescription,
  summaryLabel,
  storedAnswerReport,
  suggestedPassage,
  summariseClaims,
  summaryTone,
  supersededMessages,
  withoutFlaggedCitations,
} from '../src/features/agent/grounding.ts';
import { answerText, initialTranscript } from '../src/features/agent/reducer.ts';
import type { TranscriptState } from '../src/features/agent/reducer.ts';

const shared = JSON.parse(
  readFileSync(new URL('../../crates/shodh-rag/src/harness/fixtures/citations.json', import.meta.url), 'utf8'),
) as { cases: Array<{ text: string; markers: number[][] }> };

test('the citation grammar matches the Rust verifier on every shared case', () => {
  assert.ok(shared.cases.length >= 10);
  for (const c of shared.cases) {
    assert.deepEqual(parseCitationMarkers(c.text).map(m => m.numbers), c.markers, c.text);
  }
});

test('marker spans cover the brackets and cited numbers are collected', () => {
  const text = 'Sixty days [1]. Also [2, 4-5].';
  const markers = parseCitationMarkers(text);
  assert.equal(text.slice(markers[1].start, markers[1].end), '[2, 4-5]');
  assert.deepEqual([...citedNumbersIn(text)].sort((a, b) => a - b), [1, 2, 4, 5]);
});

function check(partial: Partial<ClaimCheck>): ClaimCheck {
  return {
    messageId: 'm1',
    text: 'The fee is 900 EUR.',
    anchor: 'The fee is 900 EUR [2].',
    kind: 'sentence',
    outcome: 'unsupported',
    cited: [2],
    invalid: [],
    support: 0.1,
    contradiction: null,
    missingNumbers: [],
    closest: null,
    closestScore: null,
    ...partial,
  };
}

test('flags go after their anchors, in order, and only for flagged claims', () => {
  const content = 'Notice is 60 days [1]. The fee is 900 EUR [2]. Renewal is yearly [9].\n\n| Item | Fee |\n|---|---|\n| Setup | 50 [3] |';
  const checks = [
    check({ anchor: 'Notice is 60 days [1].', outcome: 'supported' }),
    check({ anchor: 'The fee is 900 EUR [2].', outcome: 'unsupported', missingNumbers: ['900'] }),
    check({ anchor: 'Renewal is yearly [9].', outcome: 'invalid_citation', invalid: [9] }),
    check({ anchor: '50 [3]', kind: 'table_row', outcome: 'uncited_factual' }),
  ];
  const { text, placed } = insertFlagMarkers(content, checks);
  assert.deepEqual(placed, [1, 2, 3]);
  assert.ok(text.includes(`The fee is 900 EUR [2].${FLAG_OPEN}1${FLAG_CLOSE}`));
  assert.ok(text.includes(`Renewal is yearly [9].${FLAG_OPEN}2${FLAG_CLOSE}`));
  // The table row's flag stays inside its last cell, so the table still parses.
  assert.ok(text.includes(`| Setup | 50 [3]${FLAG_OPEN}3${FLAG_CLOSE} |`));
  assert.ok(!text.includes(`${FLAG_OPEN}0${FLAG_CLOSE}`));
});

test('a claim whose anchor is not in the text gets no flag', () => {
  const { text, placed } = insertFlagMarkers('Other text.', [check({ anchor: 'Missing sentence [2].' })]);
  assert.equal(text, 'Other text.');
  assert.deepEqual(placed, []);
});

test('flag labels say what is wrong in plain words', () => {
  assert.equal(flagLabel(check({ outcome: 'unsupported' })), 'not found in the cited source');
  assert.equal(flagLabel(check({ outcome: 'unsupported', missingNumbers: ['900'] })), '900 not in the cited source');
  assert.equal(flagLabel(check({ outcome: 'uncited_factual' })), 'no source');
  assert.equal(flagLabel(check({ outcome: 'invalid_citation', invalid: [9] })), 'source [9] does not exist');
  assert.equal(
    flagDescription(check({ outcome: 'unsupported' }), '4, contract.pdf p.3'),
    'This statement was not found in the cited source. Closest passage: 4, contract.pdf p.3.',
  );
  assert.ok(isFlagged('invalid_citation') && isFlagged('uncited_factual') && isFlagged('unsupported'));
  assert.ok(!isFlagged('weak') && !isFlagged('supported') && !isFlagged('unchecked'));
  // "You have no tasks" with no tool reading the tasks in that answer.
  assert.ok(isFlagged('unverified_absence'));
  assert.equal(flagLabel(check({ outcome: 'unverified_absence' })), 'not looked up');
  assert.equal(
    flagDescription(check({ outcome: 'unverified_absence' }), null),
    'This says you have none of something, but nothing in this answer looked it up.',
  );
});

function summary(partial: Partial<GroundingSummary>): GroundingSummary {
  return { checked: 16, supported: 14, weak: 0, unsupported: 1, uncited: 1, invalid: 0, unchecked: 0, score: 0.875, ...partial };
}

test('the summary chip counts supported claims of the checkable ones', () => {
  assert.equal(summaryLabel(summary({})), 'Grounded 14/16');
  assert.equal(summaryLabel(summary({ checked: 10, supported: 6, weak: 2, unsupported: 0, uncited: 0, unchecked: 2 })), 'Grounded 8/8');
  assert.equal(summaryTone(summary({})), 'flagged');
  assert.equal(summaryTone(summary({ unsupported: 0, uncited: 0, weak: 2 })), 'partial');
  assert.equal(summaryTone(summary({ unsupported: 0, uncited: 0 })), 'ok');
  assert.equal(
    summaryDescription(summary({})),
    '14 of 16 checked statements supported by their sources, 2 flagged',
  );
});

function report(partial: Partial<GroundingReport>): GroundingReport {
  return {
    round: 0,
    isFinal: false,
    method: 'entailment',
    summary: summary({}),
    claims: [check({}), check({ messageId: 'm2' })],
    needs: [],
    messageIds: ['m1', 'm2'],
    supersededMessageIds: [],
    ...partial,
  };
}

test('the final report describes the answer and superseded drafts are collected', () => {
  const first = report({});
  const final = report({ round: 1, isFinal: true, supersededMessageIds: ['m1', 'm2'], messageIds: ['m3'] });
  assert.equal(answerReport([first]), first);
  assert.equal(answerReport([first, final]), final);
  assert.equal(answerReport([]), null);
  assert.deepEqual([...supersededMessages([first, final])], ['m1', 'm2']);
  assert.equal(checksForMessage(first, 'm2').length, 1);
  assert.equal(checksForMessage(null, 'm2').length, 0);
});

// The reported side answer: it cites [3] for what passage 2 states; the check offers 2.
const miscited = (partial: Partial<ClaimCheck> = {}): ClaimCheck =>
  check({
    text: 'SPANN expands posting lists with the points in the closure, reducing disk-access latency.',
    anchor: 'SPANN expands posting lists with the points in the closure, reducing disk-access latency. [3]',
    cited: [3],
    support: 0.29,
    closest: 2,
    closestScore: 0.97,
    suggested: 2,
    suggestedScore: 0.91,
    ...partial,
  });

test('a miscitation says which passage supports the statement and what the answer cited', () => {
  assert.equal(flagLabel(miscited()), 'Supported by passage 2 (answer cited 3)');
  assert.equal(
    flagDescription(miscited(), '2, spann.pdf p.2'),
    'Passage 2 supports this statement; the answer cited [3], which does not. Closest passage: 2, spann.pdf p.2.',
  );
  assert.equal(suggestedPassage(miscited()), 2);
  assert.equal(citeInsteadOffer(miscited()), 2);
  // A report saved before the check named the passage reads as before.
  const old = miscited();
  delete old.suggested;
  assert.equal(suggestedPassage(old), null);
  assert.equal(flagLabel(old), 'not found in the cited source');
  // Only an unsupported claim is offered a passage, never one it already cites.
  assert.equal(suggestedPassage(miscited({ outcome: 'weak' })), null);
  assert.equal(suggestedPassage(miscited({ cited: [2, 3] })), null);
  // A table row whose citation is in another cell than its anchor is not rewritten.
  assert.equal(citeInsteadOffer(miscited({ kind: 'table_row', anchor: '0.97' })), null);
  assert.equal(citeInsteadOffer(miscited({ kind: 'table_row', anchor: '0.97 [3]' })), 2);
});

test('citing another passage rewrites the markers inside the anchor only', () => {
  assert.equal(citeInAnchor('latency. [3]', 2), 'latency. [2]');
  assert.equal(citeInAnchor('Latency drops [3].', 2), 'Latency drops [2].');
  assert.equal(citeInAnchor('Latency drops [3][7].', 2), 'Latency drops [2].');
  assert.equal(citeInAnchor('Latency drops [3], [7].', 2), 'Latency drops [2].');
  assert.equal(citeInAnchor('Latency drops [3-4].', 2), 'Latency drops [2].');
  // An inherited citation: the marker goes before the full stop.
  assert.equal(citeInAnchor('Latency drops.', 2), 'Latency drops [2].');
  assert.equal(citeInAnchor('Latency drops', 2), 'Latency drops [2]');
});

function transcriptWith(text: string, claims: ClaimCheck[], partial: Partial<TranscriptState> = {}): TranscriptState {
  return {
    ...initialTranscript('run-1', 0),
    status: 'completed',
    blocks: [{ kind: 'text', id: 'm1', text }],
    groundings: [report({ isFinal: true, messageIds: ['m1'], claims, summary: summariseClaims(claims) })],
    ...partial,
  };
}

test('"Cite N instead" rewrites the stored answer and records the claim as supported', () => {
  const supported = check({ text: 'SPANN keeps centroids in memory.', anchor: 'SPANN keeps centroids in memory [1].', outcome: 'supported', cited: [1] });
  const wrong = miscited();
  const text = `${supported.anchor} ${wrong.anchor}`;
  const state = transcriptWith(text, [supported, wrong]);
  assert.equal(state.groundings[0].summary.score, 0.5);
  const next = citeSuggested(state, state.groundings[0].claims[1]);
  assert.ok(next);
  assert.equal(
    answerText(next),
    'SPANN keeps centroids in memory [1]. SPANN expands posting lists with the points in the closure, reducing disk-access latency. [2]',
  );
  const claim = next.groundings[0].claims[1];
  assert.equal(claim.outcome, 'supported');
  assert.deepEqual(claim.cited, [2]);
  assert.equal(claim.anchor, 'SPANN expands posting lists with the points in the closure, reducing disk-access latency. [2]');
  assert.equal(claim.suggested, null);
  assert.equal(claim.support, 0.91, "the claim shows the support the check found in passage 2");
  assert.equal(claim.suggestedScore, null);
  assert.equal(claim.closest, null);
  assert.equal(next.groundings[0].summary.supported, 2);
  assert.equal(next.groundings[0].summary.score, 1);
  // The supported claim and the original state are untouched.
  assert.equal(next.groundings[0].claims[0], supported);
  assert.equal(answerText(state), text);
  // Nothing to rewrite: a running answer, a claim no longer there, a claim without a suggestion.
  assert.equal(citeSuggested({ ...state, status: 'running' }, wrong), null);
  assert.equal(citeSuggested(transcriptWith('Other text.', [wrong]), wrong), null);
  assert.equal(citeSuggested(state, supported), null);
  // A report saved before the score was kept: the claim's support is not known.
  const older = miscited();
  delete older.suggestedScore;
  const rewritten = citeSuggested(transcriptWith(older.anchor, [older]), older);
  assert.ok(rewritten);
  assert.equal(rewritten.groundings[0].claims[0].support, null);
});

test('of two identical miscited sentences, the one whose flag was used is rewritten', () => {
  const first = miscited();
  const second = miscited();
  const text = `${first.anchor}

${second.anchor}`;
  const state = transcriptWith(text, [first, second]);
  const next = citeSuggested(state, state.groundings[0].claims[1]);
  assert.ok(next);
  assert.equal(answerText(next), `${first.anchor}

${second.anchor.replace('[3]', '[2]')}`);
  assert.equal(next.groundings[0].claims[0].outcome, 'unsupported');
  assert.equal(next.groundings[0].claims[1].outcome, 'supported');
});

test('flagged citations are not passed on as context; supported ones are', () => {
  const supported = check({ anchor: 'Centroids stay in memory [1].', outcome: 'supported', cited: [1] });
  const invalid = check({ anchor: 'Search is partial [9].', outcome: 'invalid_citation', cited: [9], invalid: [9] });
  const weak = check({ anchor: 'Recall is high [7].', outcome: 'weak', cited: [7] });
  const answer = `Centroids stay in memory [1]. ${miscited().anchor}

Search is partial [9]. Recall is high [7].`;
  const r = report({ claims: [supported, miscited(), invalid, weak] });
  assert.equal(
    withoutFlaggedCitations(answer, r),
    'Centroids stay in memory [1]. SPANN expands posting lists with the points in the closure, reducing disk-access latency.\n\nSearch is partial. Recall is high [7].',
  );
  assert.equal(withoutFlaggedCitations(answer, null), answer);
  // A stored transcript's final report is the one read.
  assert.equal(storedAnswerReport({ groundings: [report({}), r] }), r);
  assert.equal(storedAnswerReport({ groundings: [r, report({ isFinal: true })] })?.isFinal, true);
  assert.equal(storedAnswerReport(undefined), null);
  assert.equal(storedAnswerReport({ groundings: 'x' }), null);
});

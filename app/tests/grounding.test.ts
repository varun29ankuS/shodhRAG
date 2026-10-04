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
  citedNumbersIn,
  flagDescription,
  flagLabel,
  insertFlagMarkers,
  isFlagged,
  parseCitationMarkers,
  summaryDescription,
  summaryLabel,
  summaryTone,
  supersededMessages,
} from '../src/features/agent/grounding.ts';

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

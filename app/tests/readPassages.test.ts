/**
 * Passages from `open_document` reads: numbered like search passages, so
 * the transcript resolves citations of an answer built only from reads.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/readPassages.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { initialTranscript, passagesFromDetail, reduceAll } from '../src/features/agent/reducer.ts';
import type { TranscriptAction } from '../src/features/agent/reducer.ts';

/** The detail `open_document` returns (shape of `cite_read` in documents.rs). */
const readDetail = {
  path: 'c:/papers/deltanet.pdf',
  location: 'page 4',
  passages: [
    {
      n: 4,
      file: 'deltanet.pdf',
      path: 'c:/papers/deltanet.pdf',
      page: '4',
      section: '3 Method > 3.2 Chunkwise form',
      score: 1,
      text: 'The chunkwise form computes the state once per chunk.',
      regions: [{ page: 4, x0: 72, y0: 120, x1: 520, y1: 160 }],
    },
    {
      n: 5,
      file: 'deltanet.pdf',
      path: 'c:/papers/deltanet.pdf',
      page: '4',
      section: null,
      score: 1,
      text: 'Training throughput doubles.',
    },
  ],
};

test('a read step detail yields numbered passages with page, section and boxes', () => {
  const passages = passagesFromDetail(readDetail);
  assert.deepEqual(passages.map(p => p.n), [4, 5]);
  assert.equal(passages[0].file, 'deltanet.pdf');
  assert.equal(passages[0].page, '4');
  assert.equal(passages[0].section, '3 Method > 3.2 Chunkwise form');
  assert.deepEqual(passages[0].regions, [{ page: 4, x0: 72, y0: 120, x1: 520, y1: 160 }]);
  assert.equal(passages[1].section, null);
  assert.equal(passages[1].regions, null);
  assert.equal(passages[1].web, undefined);
});

test('the transcript keeps passages of reads and searches in one numbering', () => {
  const runId = 'run-1';
  const started = (stepId: string, tool: string): TranscriptAction => ({
    type: 'step_started',
    runId,
    stepId,
    parentStepId: null,
    tool,
    label: tool,
    args: {},
    tier: 'read',
    atMs: 1,
  } as TranscriptAction);
  const finished = (stepId: string, detail: unknown): TranscriptAction => ({
    type: 'step_finished',
    runId,
    stepId,
    ok: true,
    summary: 'done',
    detail,
    durationMs: 1,
  } as TranscriptAction);
  const search = {
    passages: [1, 2, 3].map(n => ({ n, file: 'a.pdf', path: 'c:/a.pdf', page: String(n), score: 0.5, text: `hit ${n}` })),
  };
  const state = reduceAll(initialTranscript(runId, 0), [
    started('s1', 'search_documents'),
    finished('s1', search),
    started('s2', 'open_document'),
    finished('s2', readDetail),
  ]);
  assert.deepEqual(state.passages.map(p => p.n), [1, 2, 3, 4, 5]);
  assert.equal(state.passages[3].path, 'c:/papers/deltanet.pdf');

  // Only reads: every cited number still resolves.
  const readsOnly = reduceAll(initialTranscript(runId, 0), [started('s1', 'open_document'), finished('s1', readDetail)]);
  const known = new Set(readsOnly.passages.map(p => p.n));
  for (const n of [4, 5]) assert.ok(known.has(n), `[${n}] resolves`);
});

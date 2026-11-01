/**
 * The System status: reading the backend's report, and the strip's, the
 * panel's and the CPU notice's words.
 *   node --experimental-strip-types --test app/tests/systemStatus.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  cpuNotice,
  cpuReasonText,
  etaText,
  indexingText,
  pagesPerMinuteText,
  readSystemStatus,
  searchSummary,
} from '../src/features/status/systemStatus.ts';
import type { SearchReport, SystemStatus } from '../src/features/status/systemStatus.ts';

function report(extra: Partial<SearchReport> = {}): SearchReport {
  return {
    accelerator: 'cpu',
    device: null,
    cpuReason: { kind: 'noGpu' },
    threads: 8,
    modelFile: 'model.onnx',
    pagesPerMinute: 96,
    speed: 'slow',
    recentPagesPerMinute: null,
    degraded: false,
    calibratedAt: '2026-11-01T00:00:00Z',
    loaded: true,
    ...extra,
  };
}

function status(extra: Partial<SystemStatus> = {}): SystemStatus {
  return {
    searchInstalled: true,
    useGpu: true,
    search: report(),
    rerankerInstalled: true,
    scanReaderInstalled: true,
    indexingPaused: false,
    running: [],
    ...extra,
  };
}

test('the backend report is read field by field, and junk is dropped', () => {
  const read = readSystemStatus({
    searchInstalled: true,
    useGpu: false,
    search: {
      accelerator: 'directml',
      device: 'NVIDIA GeForce RTX 4060',
      cpuReason: null,
      threads: null,
      modelFile: 'model.onnx',
      pagesPerMinute: 5200,
      speed: 'fast',
      recentPagesPerMinute: null,
      degraded: false,
      calibratedAt: '2026-11-01T00:00:00Z',
      loaded: true,
    },
    rerankerInstalled: true,
    scanReaderInstalled: false,
    indexingPaused: true,
    running: [{ sourceId: 'folder-papers', path: 'C:/Papers' }, { sourceId: 7 }, 'x'],
  });
  assert.ok(read);
  assert.equal(read.useGpu, false);
  assert.equal(read.search?.accelerator, 'directml');
  assert.equal(read.search?.device, 'NVIDIA GeForce RTX 4060');
  assert.deepEqual(read.running, [{ sourceId: 'folder-papers', path: 'C:/Papers' }]);
  assert.equal(readSystemStatus('nope'), null);
  // A report without a known target or speed is not shown as one.
  assert.equal(readSystemStatus({ search: { accelerator: 'tpu', speed: 'fast', pagesPerMinute: 1 } })?.search, null);
});

test('the strip says where search runs and how fast, in plain words', () => {
  assert.deepEqual(searchSummary(status()), {
    short: 'CPU · slow',
    sentence: 'Search runs on the processor (CPU), slow: ≈96 pages a minute.',
    tone: 'bad',
  });
  const gpu = searchSummary(
    status({ search: report({ accelerator: 'directml', device: 'Radeon 780M', cpuReason: null, speed: 'fast', pagesPerMinute: 4321 }) }),
  );
  assert.equal(gpu.short, 'GPU · fast');
  assert.equal(gpu.tone, 'good');
  assert.match(gpu.sentence, /graphics card \(Radeon 780M\), fast: ≈4,300 pages a minute/);
  assert.equal(searchSummary(status({ search: null })).short, 'Not measured yet');
  assert.equal(searchSummary(status({ searchInstalled: false })).short, 'Not set up');
  assert.equal(searchSummary(null).tone, 'idle');
});

test('a machine running below its measured speed says so', () => {
  const slowed = searchSummary(
    status({ search: report({ speed: 'ok', pagesPerMinute: 400, degraded: true, recentPagesPerMinute: 150 }) }),
  );
  assert.equal(slowed.short, 'CPU · slower than usual');
  assert.equal(slowed.tone, 'warn');
  assert.match(slowed.sentence, /≈150 pages a minute instead of ≈400 pages a minute/);
});

test('the CPU notice gives the speed, the reason and what helps', () => {
  const notice = cpuNotice(status());
  assert.ok(notice);
  assert.equal(notice.title, 'Search is using the CPU, so indexing will be slow (≈96 pages a minute).');
  assert.equal(notice.reason, 'Reason: No graphics card that Shodh can use was found.');
  assert.equal(notice.tone, 'bad');
  const failed = cpuNotice(
    status({ search: report({ cpuReason: { kind: 'failed', device: 'Intel Arc', error: 'device removed' } }) }),
  );
  assert.equal(failed?.reason, 'Reason: Intel Arc could not run the search model: device removed');
  assert.equal(failed?.advice, 'Update the graphics driver, then restart Shodh.');
  // No notice on a GPU, or before search has run.
  assert.equal(cpuNotice(status({ search: report({ accelerator: 'coreml', cpuReason: null }) })), null);
  assert.equal(cpuNotice(status({ search: null })), null);
});

test('every reason has its own words', () => {
  assert.match(cpuReasonText({ kind: 'chosen' }), /You chose/);
  assert.match(cpuReasonText({ kind: 'noProvider' }), /no graphics card support/);
  assert.match(
    cpuReasonText({ kind: 'slowerThanCpu', device: 'Intel UHD', gpuTokensPerSecond: 800, cpuTokensPerSecond: 1200 }),
    /Intel UHD ran the search model slower than the processor/,
  );
  assert.match(cpuReasonText({ kind: 'failed', device: null, error: 'DXGI' }), /could not be checked: DXGI/);
});

test('speeds round to two figures and times left to minutes', () => {
  assert.equal(pagesPerMinuteText(96.4), '≈96 pages a minute');
  assert.equal(pagesPerMinuteText(1234), '≈1,200 pages a minute');
  assert.equal(pagesPerMinuteText(1), '≈1 page a minute');
  assert.equal(pagesPerMinuteText(0.2), 'under 1 page a minute');
  assert.equal(etaText(30), 'under a minute left');
  assert.equal(etaText(600), 'about 10 min left');
  assert.equal(etaText(3 * 3600 + 20 * 60), 'about 3 h 20 min left');
  assert.equal(etaText(7200), 'about 2 h left');
  assert.equal(etaText(0), null);
  assert.equal(etaText(Number.NaN), null);
});

test('the indexing line says paused, running or idle', () => {
  assert.equal(indexingText(status({ indexingPaused: true }), 2), 'Indexing paused');
  assert.equal(indexingText(status({ indexingPaused: true }), 0), 'Indexing paused (nothing waiting)');
  assert.equal(indexingText(status(), 1), 'Indexing 1 folder');
  assert.equal(indexingText(status(), 3), 'Indexing 3 folders');
  assert.equal(indexingText(status(), 0), 'Nothing indexing');
});

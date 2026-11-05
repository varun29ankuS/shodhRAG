/**
 * The System status: reading the backend's report, and the strip's, the
 * panel's and the CPU notice's words.
 *   node --experimental-strip-types --test app/tests/systemStatus.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  cpuNotice,
  cpuReasonDetail,
  cpuReasonText,
  etaText,
  indexingText,
  modelSwitchView,
  discardConfirmText,
  pagesPerMinuteText,
  readCpuReason,
  readModelSwitch,
  readSystemStatus,
  searchSummary,
} from '../src/features/status/systemStatus.ts';
import type { ModelSwitch, SearchReport, SystemStatus } from '../src/features/status/systemStatus.ts';

function report(extra: Partial<SearchReport> = {}): SearchReport {
  return {
    accelerator: 'cpu',
    device: null,
    cpuReason: { kind: 'noGpu' },
    threads: 8,
    modelFile: 'model_O4.onnx',
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
    modelSwitch: readModelSwitch(null),
    ...extra,
  };
}

function modelSwitch(extra: Partial<ModelSwitch> = {}): ModelSwitch {
  return { ...readModelSwitch(null), indexVariant: 'fp16', ...extra };
}

test('the faster model switch is read field by field, and junk is dropped', () => {
  const read = readModelSwitch({
    indexVariant: 'fp16',
    target: 'int8',
    offered: true,
    writesBlocked: 'yes',
    phase: 'building',
    finishFailed: false,
    restarting: true,
    done: 120,
    total: 4000,
    etaSeconds: 600,
    downloadedBytes: -3,
    downloadBytes: 'x',
    modelBytes: 278_686_411,
    error: '',
    errorDetail: 'lance: io',
  });
  assert.deepEqual(read, {
    indexVariant: 'fp16',
    target: 'int8',
    offered: true,
    writesBlocked: false,
    phase: 'building',
    finishFailed: false,
    restarting: true,
    done: 120,
    total: 4000,
    etaSeconds: 600,
    downloadedBytes: 0,
    downloadBytes: 0,
    modelBytes: 278_686_411,
    error: null,
    errorDetail: 'lance: io',
  });
  assert.equal(readModelSwitch({ phase: 'exploded' }).phase, 'idle');
  assert.equal(readSystemStatus({})?.modelSwitch.phase, 'idle');
});

test('the faster model is an opt-in that says its trade-off without a speed claim, and there is a way back', () => {
  assert.equal(modelSwitchView(status()), null, 'nothing offered, nothing shown');
  const offered = modelSwitchView(
    status({ modelSwitch: modelSwitch({ offered: true, target: 'int8', modelBytes: 278_686_411 }) }),
  );
  assert.equal(offered?.title, 'Faster search (slightly less accurate) is available for this computer.');
  assert.match(offered?.detail ?? '', /indexes faster on this processor/);
  assert.doesNotMatch(offered?.detail ?? '', /\d+ times/, 'no number without a same-processor measurement');
  assert.match(offered?.detail ?? '', /may rank some passages lower/);
  assert.match(offered?.detail ?? '', /search keeps working meanwhile/);
  assert.match(offered?.detail ?? '', /\(279 MB\)/);
  assert.deepEqual(offered?.actions, [{ action: 'start', label: 'Re-index for faster search' }]);

  const back = modelSwitchView(status({ modelSwitch: modelSwitch({ indexVariant: 'int8', offered: true, target: 'fp16' }) }));
  assert.equal(back?.title, 'Your library uses faster search (slightly less accurate).');
  assert.match(back?.detail ?? '', /ranks passages more accurately/);
  assert.deepEqual(back?.actions, [{ action: 'start', label: 'Re-index for standard search' }]);

  const blocked = modelSwitchView(
    status({ modelSwitch: modelSwitch({ indexVariant: 'int8', offered: true, target: 'fp16', writesBlocked: true }) }),
  );
  assert.equal(blocked?.title, 'Your library uses the faster search model, which this processor can’t run exactly.');
  assert.match(blocked?.detail ?? '', /Nothing new is indexed/);
  assert.equal(blocked?.tone, 'bad');

  const downloading = modelSwitchView(
    status({
      modelSwitch: modelSwitch({ phase: 'downloading', target: 'int8', downloadedBytes: 120_000_000, downloadBytes: 278_686_411 }),
    }),
  );
  assert.equal(downloading?.title, 'Downloading the faster search model');
  assert.equal(downloading?.detail, '120 MB of 279 MB');
  assert.deepEqual(downloading?.actions.map(a => a.action), ['stop', 'discard'], 'a download can be stopped');

  const building = modelSwitchView(
    status({ modelSwitch: modelSwitch({ phase: 'building', target: 'int8', done: 1200, total: 40_000, etaSeconds: 480 }) }),
  );
  assert.equal(building?.title, 'Re-indexing for faster search');
  assert.equal(building?.detail, '1,200 of 40,000 passages · about 8 min left. Search keeps working meanwhile.');
  assert.deepEqual(building?.actions, [
    { action: 'stop', label: 'Stop' },
    { action: 'discard', label: 'Discard' },
  ]);
  const backBuilding = modelSwitchView(status({ modelSwitch: modelSwitch({ phase: 'building', target: 'fp16', done: 1, total: 2 }) }));
  assert.equal(backBuilding?.title, 'Re-indexing for standard search');
  const paused = modelSwitchView(
    status({
      indexingPaused: true,
      modelSwitch: modelSwitch({ phase: 'building', target: 'int8', done: 1200, total: 40_000, etaSeconds: 480 }),
    }),
  );
  assert.equal(paused?.title, 'Re-indexing for faster search (paused)');
  assert.doesNotMatch(paused?.detail ?? '', /left/);
  const restarting = modelSwitchView(
    status({ modelSwitch: modelSwitch({ phase: 'building', target: 'int8', restarting: true, done: 3, total: 9 }) }),
  );
  assert.equal(restarting?.title, 'Getting the switch ready, then restarting Shodh');
  assert.deepEqual(restarting?.actions, []);

  const stopped = modelSwitchView(status({ modelSwitch: modelSwitch({ phase: 'stopped', target: 'int8', done: 5, total: 9 }) }));
  assert.deepEqual(stopped?.actions.map(a => a.action), ['continue', 'discard']);
  assert.match(stopped?.detail ?? '', /5 of 9 passages/);

  const ready = modelSwitchView(status({ modelSwitch: modelSwitch({ phase: 'ready', target: 'int8' }) }));
  assert.equal(ready?.title, 'The faster search model is ready.');
  assert.match(ready?.detail ?? '', /saves your work, then restarts/);
  assert.deepEqual(ready?.actions.map(a => a.action), ['restart', 'discard']);

  const failed = modelSwitchView(status({ modelSwitch: modelSwitch({ phase: 'failed', target: 'int8', error: 'disk full' }) }));
  assert.equal(failed?.detail, 'Reason: disk full');
  assert.equal(failed?.tone, 'bad');
  assert.deepEqual(failed?.actions.map(a => a.action), ['retry', 'discard']);
});

test('a switch that could not finish at start says why first, details behind a toggle, and offers Retry or Cancel', () => {
  const reason =
    'Shodh could not finish switching the search model when it started (the search model it switches to is not installed any more), so your library kept its current model.';
  const view = modelSwitchView(
    status({
      modelSwitch: modelSwitch({ phase: 'failed', finishFailed: true, target: 'int8', error: reason, errorDetail: 'loading: x' }),
    }),
  );
  assert.equal(view?.title, 'Switching to the faster search model did not finish.');
  assert.equal(view?.detail, reason);
  assert.equal(view?.technical, 'loading: x');
  assert.doesNotMatch(view?.title ?? '', /ready/);
  assert.deepEqual(view?.actions, [
    { action: 'retryRestart', label: 'Try again (restarts Shodh)' },
    { action: 'discard', label: 'Cancel the switch' },
  ]);
});

test('a discard asks first and names the work it throws away', () => {
  assert.equal(
    discardConfirmText(status({ modelSwitch: modelSwitch({ phase: 'building', done: 1200, total: 4000 }) })),
    'Discard the 1,200 passages already re-indexed? Starting again re-indexes the library from the beginning.',
  );
  assert.equal(
    discardConfirmText(status({ modelSwitch: modelSwitch({ phase: 'downloading' }) })),
    'Discard the re-index? Starting again re-indexes the library from the beginning.',
  );
});

test('the backend report is read field by field, and junk is dropped', () => {
  const read = readSystemStatus({
    searchInstalled: true,
    useGpu: false,
    search: {
      accelerator: 'directml',
      device: 'NVIDIA GeForce RTX 4060',
      cpuReason: null,
      threads: null,
      modelFile: 'model_O4.onnx',
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
  // A report without a known target is not shown as one.
  assert.equal(readSystemStatus({ search: { accelerator: 'tpu', speed: 'fast', pagesPerMinute: 1 } })?.search, null);
  // A speed that could not be measured is read as unmeasured, not dropped.
  const unmeasured = readSystemStatus({ search: { accelerator: 'cpu', pagesPerMinute: null, speed: null } });
  assert.equal(unmeasured?.search?.pagesPerMinute, null);
  assert.equal(unmeasured?.search?.speed, null);
});

test('the strip says where search runs and how fast, in plain words', () => {
  assert.deepEqual(searchSummary(status()), {
    short: 'CPU · slow',
    sentence: 'The search model runs on the processor (CPU), slow: ≈96 pages a minute.',
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
  const unmeasured = searchSummary(status({ search: report({ pagesPerMinute: null, speed: null }) }));
  assert.equal(unmeasured.short, 'CPU · speed not measured');
});

test('slower than usual is shown only while the model is loaded and indexing runs', () => {
  const slowed = report({ speed: 'ok', pagesPerMinute: 400, degraded: true, recentPagesPerMinute: 150 });
  const shown = searchSummary(status({ search: slowed }));
  assert.equal(shown.short, 'CPU · slower than usual');
  assert.equal(shown.tone, 'warn');
  assert.match(shown.sentence, /≈150 pages a minute instead of ≈400 pages a minute/);
  assert.equal(searchSummary(status({ search: { ...slowed, loaded: false } })).short, 'CPU · moderate');
  assert.equal(searchSummary(status({ search: slowed, indexingPaused: true })).short, 'CPU · moderate');
});

test('the CPU notice gives the search model speed, the reason in plain words and what helps', () => {
  const notice = cpuNotice(status());
  assert.ok(notice);
  assert.equal(notice.title, 'Search is using the CPU, so indexing will be slow (search model speed ≈96 pages a minute).');
  assert.equal(notice.reason, 'Reason: No graphics card that Shodh can use was found.');
  assert.equal(notice.tone, 'bad');
  const failedReason = { kind: 'failed' as const, device: 'Intel Arc', error: 'DirectML: 887A0005 device removed' };
  const failed = cpuNotice(status({ search: report({ cpuReason: failedReason }) }));
  assert.equal(failed?.reason, 'Reason: Intel Arc could not run the search model.');
  assert.doesNotMatch(failed?.reason ?? '', /887A0005/, 'the technical error is not in the notice');
  assert.equal(cpuReasonDetail(failedReason), 'DirectML: 887A0005 device removed');
  assert.match(failed?.advice ?? '', /Update the graphics driver/);
  // At a moderate speed the notice does not call indexing slow.
  const moderate = cpuNotice(status({ search: report({ speed: 'ok', pagesPerMinute: 300 }) }));
  assert.equal(moderate?.title, 'Search is using the CPU (search model speed ≈300 pages a minute).');
  // No notice on a GPU, or before search has run.
  assert.equal(cpuNotice(status({ search: report({ accelerator: 'coreml', cpuReason: null }) })), null);
  assert.equal(cpuNotice(status({ search: null })), null);
});

test('every reason has its own words', () => {
  assert.match(cpuReasonText({ kind: 'chosen' }), /You turned off/);
  assert.match(cpuReasonText({ kind: 'noProvider' }), /no graphics card support/);
  assert.match(
    cpuReasonText({ kind: 'slowerThanCpu', device: 'Intel UHD', gpuTokensPerSecond: 800, cpuTokensPerSecond: 1200 }),
    /Intel UHD ran the search model slower than the processor/,
  );
  assert.equal(cpuReasonText({ kind: 'failed', device: null, error: 'DXGI' }), 'The graphics cards could not be checked.');
  assert.equal(cpuReasonDetail({ kind: 'noGpu' }), null);
  assert.match(cpuReasonText({ kind: 'cpuModel' }), /indexed with the search model made for the processor/);
  assert.deepEqual(readCpuReason({ kind: 'cpuModel' }), { kind: 'cpuModel' });
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

test('the indexing line says paused, running or idle, counting folders and files alike', () => {
  assert.equal(indexingText(status({ indexingPaused: true }), 2), 'Indexing paused');
  assert.equal(indexingText(status({ indexingPaused: true }), 0), 'Indexing paused (nothing waiting)');
  assert.equal(indexingText(status(), 1), 'Indexing 1 item');
  assert.equal(indexingText(status(), 3), 'Indexing 3 items');
  assert.equal(indexingText(status(), 0), 'Nothing indexing');
});

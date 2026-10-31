/**
 * "Getting Shodh ready" state machine and question gate. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/readiness.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  approxMegabytes,
  partsToInstall,
  questionGate,
  queuedQuestionAction,
  readiness,
  setupDisclosure,
  setupHeading,
  waitingLine,
} from '../src/features/setup/readiness.ts';
import type { RuntimeInput, SearchInput } from '../src/features/setup/readiness.ts';
import type { ArtifactStatus, SearchModelsStatus } from '../src/features/setup/searchModels.ts';

const MB = 1_000_000;

function artifact(overrides: Partial<ArtifactStatus>): ArtifactStatus {
  return {
    name: 'E5 embedding model',
    relativePath: 'multilingual-e5-base/model_O4.onnx',
    size: 600 * MB,
    sha256: 'a'.repeat(64),
    state: 'missing',
    presentBytes: 0,
    ...overrides,
  };
}

function searchStatus(artifacts: ArtifactStatus[], ready = false): SearchModelsStatus {
  return { ready, installing: false, modelDir: 'C:/models', totalBytes: artifacts.reduce((n, a) => n + a.size, 0), artifacts };
}

function search(overrides: Partial<SearchInput> = {}): SearchInput {
  return {
    status: searchStatus([artifact({})]),
    statusError: null,
    installing: false,
    progress: null,
    installError: null,
    installOffline: false,
    ...overrides,
  };
}

const READY_SEARCH = search({ status: searchStatus([artifact({ state: 'verified', presentBytes: 600 * MB })], true) });

function runtime(overrides: Partial<RuntimeInput> = {}): RuntimeInput {
  return {
    status: { installed: false, downloadBytes: 235 * MB, source: 'GitHub' },
    statusError: null,
    installing: false,
    progress: null,
    installError: null,
    installOffline: false,
    ...overrides,
  };
}

const READY_RUNTIME = runtime({ status: { installed: true, downloadBytes: 235 * MB, source: 'GitHub' } });

test('fresh install: both parts are needed, the disclosure names sizes and sources, a question waits', () => {
  const state = readiness(search(), runtime());
  assert.equal(state.phase, 'needed');
  assert.deepEqual(state.parts.map(p => p.state), ['needed', 'needed']);
  assert.equal(state.remainingBytes, 835 * MB);
  assert.deepEqual(partsToInstall(state), ['search', 'runtime']);
  const disclosure = setupDisclosure(state, 'GitHub');
  assert.match(disclosure, /search models \(≈600 MB\) from Hugging Face/);
  assert.match(disclosure, /answer engine \(≈235 MB\) from GitHub/);
  assert.match(disclosure, /About 835 MB in all/);
  assert.doesNotMatch(disclosure, /SHA|sha256|checksum|hash/i);
  assert.equal(questionGate(state), 'wait');
  assert.equal(setupHeading(state), 'Get Shodh ready (one time)');
});

test('already installed: ready, nothing to install, questions go at once', () => {
  const state = readiness(READY_SEARCH, READY_RUNTIME);
  assert.equal(state.phase, 'ready');
  assert.deepEqual(partsToInstall(state), []);
  assert.equal(questionGate(state), 'send');
  assert.equal(setupDisclosure(state), 'Everything Shodh needs is on this computer.');
});

test('partial: search models ready, the answer engine missing (an existing user) is offered alone', () => {
  const state = readiness(READY_SEARCH, runtime());
  assert.equal(state.phase, 'needed');
  assert.deepEqual(partsToInstall(state), ['runtime']);
  assert.match(setupDisclosure(state, 'GitHub'), /^Shodh downloads the answer engine \(≈235 MB\) from GitHub/);
  assert.equal(questionGate(state), 'wait');
});

test('partial: an interrupted search download resumes and says what is left', () => {
  const state = readiness(
    search({ status: searchStatus([artifact({ state: 'partial', presentBytes: 400 * MB })]) }),
    READY_RUNTIME,
  );
  assert.equal(state.parts[0].state, 'needed');
  assert.match(state.parts[0].detail, /^Paused/);
  assert.equal(state.remainingBytes, 200 * MB);
});

test('installing: progress per part, and a waiting question says how far setup is', () => {
  const state = readiness(
    READY_SEARCH,
    runtime({ installing: true, progress: { downloaded: 141 * MB, total: 235 * MB } }),
  );
  assert.equal(state.phase, 'installing');
  assert.equal(state.parts[1].state, 'installing');
  assert.equal(Math.round((state.parts[1].fraction ?? 0) * 100), 60);
  assert.equal(questionGate(state), 'wait');
  assert.equal(setupHeading(state), 'Getting Shodh ready');
  assert.equal(
    waitingLine(state),
    'Getting Shodh ready: answer engine 60% (1 of 2 ready). Your question is sent when it finishes.',
  );
});

test('installing with no progress event yet still counts as installing', () => {
  const state = readiness(search({ installing: true }), runtime({ installing: true }));
  assert.equal(state.phase, 'installing');
  assert.deepEqual(state.parts.map(p => p.fraction), [null, null]);
  assert.deepEqual(partsToInstall(state), []);
});

test('failed: a download that failed is offered again with its reason; the question keeps waiting', () => {
  const state = readiness(READY_SEARCH, runtime({ installError: 'Downloading the agent runtime failed: HTTP 500' }));
  assert.equal(state.phase, 'failed');
  assert.equal(state.offline, false);
  assert.equal(state.parts[1].error, 'Downloading the agent runtime failed: HTTP 500');
  assert.deepEqual(partsToInstall(state), ['runtime']);
  assert.equal(questionGate(state), 'wait');
  assert.equal(setupHeading(state), 'Setup stopped');
});

test('offline: both downloads could not connect', () => {
  const state = readiness(
    search({ installError: "Couldn't reach Hugging Face", installOffline: true }),
    runtime({ installError: "Couldn't reach GitHub", installOffline: true }),
  );
  assert.equal(state.phase, 'failed');
  assert.equal(state.offline, true);
  assert.equal(setupHeading(state), "Couldn't download what Shodh needs");
  assert.deepEqual(state.parts.map(p => p.detail), ['Could not connect', 'Could not connect']);
});

test('one part still downloading while the other failed reads as installing', () => {
  const state = readiness(search({ installing: true }), runtime({ installError: 'x' }));
  assert.equal(state.phase, 'installing');
  assert.deepEqual(partsToInstall(state), ['runtime']);
});

test('unknown statuses never hold a question', () => {
  const state = readiness(
    search({ status: null }),
    runtime({ status: null, statusError: 'no runtime command' }),
  );
  assert.equal(state.phase, 'checking');
  assert.equal(questionGate(state), 'send');
  assert.equal(questionGate(readiness(search({ status: null }), READY_RUNTIME)), 'send');
});

test('a waiting question is sent once setup is ready, kept meanwhile, dropped on a conversation switch', () => {
  assert.equal(queuedQuestionAction(null, 'c1', 'send'), 'none');
  assert.equal(queuedQuestionAction('c1', 'c1', 'wait'), 'keep');
  assert.equal(queuedQuestionAction('c1', 'c1', 'send'), 'send');
  assert.equal(queuedQuestionAction('c1', 'c2', 'wait'), 'drop');
  assert.equal(queuedQuestionAction('c1', null, 'send'), 'drop');
});

test('sizes read as rounded megabytes', () => {
  assert.equal(approxMegabytes(500_000), '<1 MB');
  assert.equal(approxMegabytes(12.4 * MB), '≈12 MB');
  assert.equal(approxMegabytes(245_669_888), '≈245 MB');
});

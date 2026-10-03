/**
 * Search setup helper tests. Run with Node 22.6+ (no extra dependencies):
 *   node --experimental-strip-types --test app/tests/searchModels.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  describeProgress,
  errorMessage,
  installFraction,
  isInstallProgress,
  isSearchModelsStatus,
  summarizeSetup,
} from '../src/features/setup/searchModels.ts';
import type { ArtifactStatus, SearchModelsStatus } from '../src/features/setup/searchModels.ts';

function artifact(overrides: Partial<ArtifactStatus>): ArtifactStatus {
  return {
    name: 'E5 embedding model',
    relativePath: 'multilingual-e5-base/model_O4.onnx',
    size: 1000,
    sha256: 'a'.repeat(64),
    state: 'missing',
    presentBytes: 0,
    ...overrides,
  };
}

function status(artifacts: ArtifactStatus[], ready = false): SearchModelsStatus {
  return {
    ready,
    installing: false,
    modelDir: 'C:/models',
    totalBytes: artifacts.reduce((n, a) => n + a.size, 0),
    artifacts,
  };
}

test('fresh install needs every byte', () => {
  const summary = summarizeSetup(status([artifact({}), artifact({ relativePath: 'b', size: 500 })]));
  assert.equal(summary.needed, true);
  assert.equal(summary.resumable, false);
  assert.equal(summary.hasCorrupt, false);
  assert.equal(summary.remainingBytes, 1500);
  assert.equal(summary.totalBytes, 1500);
});

test('partial downloads resume and verified files are skipped', () => {
  const summary = summarizeSetup(
    status([
      artifact({ state: 'partial', presentBytes: 400 }),
      artifact({ relativePath: 'b', size: 500, state: 'verified', presentBytes: 500 }),
      artifact({ relativePath: 'c', size: 200, state: 'corrupt', presentBytes: 10 }),
    ]),
  );
  assert.equal(summary.resumable, true);
  assert.equal(summary.hasCorrupt, true);
  assert.equal(summary.remainingBytes, 600 + 200);
});

test('ready engine needs no setup', () => {
  assert.equal(summarizeSetup(status([artifact({ state: 'verified' })], true)).needed, false);
});

test('progress fraction is clamped and null before the first event', () => {
  assert.equal(installFraction(null), null);
  const p = {
    artifact: 'E5 tokenizer',
    phase: 'downloading' as const,
    artifactBytes: 5,
    artifactTotal: 10,
    overallBytes: 250,
    overallTotal: 1000,
  };
  assert.equal(installFraction(p), 0.25);
  assert.equal(installFraction({ ...p, overallBytes: 2000 }), 1);
  assert.equal(installFraction({ ...p, overallTotal: 0 }), null);
  assert.match(describeProgress(p), /^Downloading E5 tokenizer/);
  assert.match(describeProgress({ ...p, phase: 'verifying' }), /SHA-256/);
  assert.equal(describeProgress(null), 'Connecting…');
});

test('error codes are stripped for display', () => {
  assert.equal(
    errorMessage('search_models_missing: the search models are not installed yet.'),
    'The search models are not installed yet.',
  );
  assert.equal(errorMessage('install_in_progress: already running'), 'Already running');
  assert.equal(errorMessage(new Error('network down')), 'network down');
});

test('payload guards reject malformed values', () => {
  assert.equal(isInstallProgress({ artifact: 'x', phase: 'checking', overallBytes: 0, overallTotal: 1 }), true);
  assert.equal(isInstallProgress({ downloaded: 1 }), false);
  assert.equal(isSearchModelsStatus(status([])), true);
  assert.equal(isSearchModelsStatus({ ready: 'yes' }), false);
  assert.equal(isSearchModelsStatus(null), false);
});

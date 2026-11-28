/**
 * The launch tour's sample agent run (src-tauri/tests/fixtures/launch_probe_run.json):
 * replayed through the Ask view's own reducer, each stage is the state its scene's
 * picture is meant to show, and the window applies it the way the session does.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/launchRun.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { STOPPED_WHEN_CLOSED, answerText, canContinue, currentStep, fromPersisted, isLive, reduceAll, toPersisted } from '../src/features/agent/reducer.ts';
import type { TranscriptAction, TranscriptState } from '../src/features/agent/reducer.ts';
import { workFold } from '../src/features/agent/workSummary.ts';
import { runSummary } from '../src/features/agent/runSummary.ts';
import { LAST_EVENT_AGO_MS, rebaseRun } from '../src/features/launch/runScene.ts';
import { appDir, source, withoutComments } from './jsxScan.ts';

interface Fixture {
  id: string;
  answerId: string;
  seed: TranscriptState;
  actions: TranscriptAction[];
  stages: Array<{ scene: string; actions: number; open?: boolean }>;
}

const fixture = JSON.parse(readFileSync(join(appDir, 'src-tauri', 'tests', 'fixtures', 'launch_probe_run.json'), 'utf8')) as Fixture;
const probe = readFileSync(join(appDir, 'src-tauri', 'src', 'launch_probe.rs'), 'utf8');

function stage(scene: string): TranscriptState {
  const at = fixture.stages.find(s => s.scene === scene);
  assert.ok(at, scene);
  return reduceAll(structuredClone(fixture.seed), fixture.actions.slice(0, at.actions));
}

test('the run scenes are the probe\'s, in order', () => {
  const scenes = /pub const TOUR_SCENES: &\[&str\] = &\[([^\]]*)\];/.exec(probe);
  assert.ok(scenes);
  const names = [...scenes[1].matchAll(/"([^"]+)"/g)].map(m => m[1]);
  assert.deepEqual(fixture.stages.map(s => s.scene), names.filter(n => n.startsWith('run-')));
});

test('plan: the run has started with its plan, nothing done yet', () => {
  const state = stage('run-plan');
  assert.ok(isLive(state));
  assert.equal(state.plan?.length, 3);
  assert.ok(state.plan?.every(item => item.status !== 'done'));
  assert.equal(Object.keys(state.steps).length, 0);
});

test('progress: a step runs, one is done, the plan is part done', () => {
  const state = stage('run-progress');
  assert.ok(isLive(state));
  assert.equal(currentStep(state)?.label, 'Reading ivf-benchmarks.pdf');
  assert.equal(state.plan?.filter(item => item.status === 'done').length, 1);
  assert.ok(workFold(state)?.live);
});

test('stopped: Shodh closed mid-answer; the answer shows it, with Continue', () => {
  const at = fixture.stages.find(s => s.scene === 'run-stopped') as { actions: number; closed?: boolean } | undefined;
  assert.ok(at?.closed, 'shown as the app restores it');
  const live = stage('run-stopped');
  assert.ok(isLive(live), 'closed while it ran');
  const shown = fromPersisted(JSON.parse(JSON.stringify(toPersisted(live))));
  assert.ok(shown);
  assert.equal(shown.error, STOPPED_WHEN_CLOSED);
  assert.ok(canContinue(shown));
  assert.ok(answerText(shown).length > 0, 'with the text written so far');
  // The window shows it so: saved and restored, set on the answer as the session does.
  const host = withoutComments(source('features/launch/LaunchProbeHost.tsx'));
  assert.match(host, /return fromPersisted\(JSON\.parse\(JSON\.stringify\(toPersisted\(transcript\)\)\)\) \?\? transcript;/);
  assert.match(host, /if \(run\.closed\) \{\s*sessionRef\.current\.updateMessage\(run\.answerId, \{ transcript: closedDuring\(transcript\) \}\);/);
});

test('summary: the run finished, with sources to open, its view opened as a click opens it', () => {
  const state = stage('run-summary');
  assert.equal(state.status, 'completed');
  const summary = runSummary(state);
  assert.deepEqual(summary?.sources.map(s => s.label), ['graph-indexes.pdf', 'ivf-benchmarks.pdf']);
  assert.ok(fixture.stages.find(s => s.scene === 'run-summary')?.open);
  // The window reduces the slice it is sent with the reducer, sets it on the answer the
  // way the session updates a message, and opens the view by its own button.
  const host = withoutComments(source('features/launch/LaunchProbeHost.tsx'));
  assert.match(host, /const timed = rebaseRun\(structuredClone\(run\.seed\), run\.actions, Date\.now\(\)\);\s*const transcript = reduceAll\(timed\.seed as TranscriptState, timed\.actions as TranscriptAction\[\]\);/);
  assert.match(host, /sessionRef\.current\.updateMessage\(run\.answerId, \{ transcript \}\);/);
  assert.match(host, /querySelector<HTMLButtonElement>\(`\[data-testid="\$\{RUN_PANEL_TOGGLE\}"\]\[aria-expanded="false"\]`\)\?\.click\(\);/);
});

test('the run clock is moved to now, so a live timer reads seconds', () => {
  const now = 1_900_000_000_000;
  const { seed, actions } = rebaseRun(fixture.seed, fixture.actions, now);
  const at = (a: unknown) => (a as { atMs?: number }).atMs;
  const times = [(seed as TranscriptState).startedAtMs, ...actions.map(at).filter((t): t is number => typeof t === 'number')];
  assert.equal(Math.max(...times), now - LAST_EVENT_AGO_MS);
  // Gaps between events are kept.
  assert.equal(at(actions[4])! - (seed as TranscriptState).startedAtMs, at(fixture.actions[4])! - fixture.seed.startedAtMs);
  const progress = reduceAll(seed as TranscriptState, actions.slice(0, 8) as TranscriptAction[]);
  assert.ok(now - progress.startedAtMs < 60_000, 'the progress timer reads under a minute');
});

/**
 * Automations' words and the form's schedule. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/automations.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  DEFAULT_WHEN,
  callsText,
  estimateText,
  formOf,
  limitTag,
  poolOf,
  readAutomation,
  readRunDetail,
  runOutcome,
  scheduleOf,
  stateChip,
  whyRan,
} from '../src/features/automations/model.ts';
import type { Automation, AutomationState, Run } from '../src/features/automations/model.ts';

const UTC = { locale: 'en-GB', timeZone: 'UTC' };
const NOW = Date.UTC(2026, 10, 25, 8, 0);

function run(patch: Partial<Run> = {}): Run {
  return {
    id: 'r1',
    jobId: 'j1',
    trigger: 'schedule',
    scheduledFor: null,
    missed: 0,
    state: 'succeeded',
    waitReason: null,
    attempt: 0,
    nextAttemptAt: null,
    plannedCalls: 4,
    usedCalls: 3,
    conversationId: 'auto-r1',
    summary: 'Found four papers',
    errorCode: null,
    errorDetail: null,
    startedAt: null,
    endedAt: null,
    createdAt: '2026-11-25T07:00:00Z',
    ...patch,
  };
}

function automation(state: AutomationState, patch: Partial<Automation> = {}): Automation {
  return {
    id: 'auto-1',
    name: 'Weekly digest',
    instruction: 'Summarise the week.',
    workspaceId: null,
    schedule: { kind: 'weekly', days: ['mon'], at: '09:00' },
    when: 'Every week on Mon at 09:00',
    nextDueAt: '2026-11-30T09:00:00Z',
    enabled: true,
    holdReason: null,
    state,
    active: null,
    last: null,
    plannedCalls: 4,
    createdBy: 'user',
    ...patch,
  };
}

test('every state has a short chip and a one-line tooltip', () => {
  const states: AutomationState[] = ['scheduled', 'manual', 'running', 'waiting', 'paused', 'offline', 'retrying', 'needs_you', 'on_hold', 'off'];
  for (const state of states) {
    const chip = stateChip(automation(state), NOW, UTC);
    assert.ok(chip.label.split(' ').length <= 2, `${state}: ${chip.label}`);
    assert.ok(chip.tooltip.length > 0 && !chip.tooltip.includes('\n'), `${state}: ${chip.tooltip}`);
  }
  assert.equal(stateChip(automation('scheduled'), NOW, UTC).tooltip, 'Runs Mon 30 Nov 09:00');
  const paused = automation('paused', { active: run({ state: 'waiting', waitReason: 'quota', nextAttemptAt: '2026-11-26T00:00:00Z' }) });
  assert.equal(stateChip(paused, NOW, UTC).tooltip, "Your provider's limit; goes on after 00:00 tomorrow");
  assert.equal(stateChip(automation('waiting', { active: run({ state: 'waiting', waitReason: 'yield' }) }), NOW, UTC).tooltip, 'Waiting while you chat');
  assert.equal(stateChip(automation('on_hold', { holdReason: 'model_unavailable' }), NOW, UTC).tooltip, 'Model not answering; check the model');
  assert.equal(stateChip(automation('running', { active: run({ state: 'running', usedCalls: 2 }) }), NOW, UTC).tooltip, '2 of 4 calls');
});

test('why a run ran, and what it came to, read in one line', () => {
  assert.equal(whyRan(run({ trigger: 'manual' })), 'Started by you');
  assert.equal(whyRan(run({ trigger: 'catch_up', missed: 2 })), 'Ran when Shodh opened; 2 earlier times were missed');
  assert.equal(whyRan(run({ trigger: 'catch_up', missed: 1 })), 'Ran when Shodh opened; 1 earlier time was missed');
  assert.equal(whyRan(run()), 'On schedule');
  assert.equal(runOutcome(run()), 'Found four papers');
  assert.equal(runOutcome(run({ state: 'waiting', waitReason: 'approval' })), 'Waiting for your OK');
  assert.equal(runOutcome(run({ state: 'failed', errorDetail: 'No model is set up.' })), 'No model is set up.');
  assert.equal(runOutcome(run({ state: 'skipped', summary: null })), 'Skipped');
  assert.equal(callsText(run()), '3 of 4 calls');
});

test('the form gives the schedule the backend stores, and refuses what it cannot run', () => {
  assert.deepEqual(scheduleOf({ ...DEFAULT_WHEN, kind: 'manual' }, 'now'), { schedule: null });
  assert.deepEqual(scheduleOf({ ...DEFAULT_WHEN, kind: 'weekly', days: ['fri', 'mon'] }, 'now'), {
    schedule: { kind: 'weekly', days: ['mon', 'fri'], at: '09:00' },
  });
  assert.deepEqual(scheduleOf({ ...DEFAULT_WHEN, kind: 'weekly', days: [] }, 'now'), { error: 'Pick at least one day.' });
  assert.deepEqual(scheduleOf({ ...DEFAULT_WHEN, kind: 'daily', at: '' }, 'now'), { error: 'Pick a time.' });
  assert.deepEqual(scheduleOf({ ...DEFAULT_WHEN, kind: 'monthly', day: 'last' }, 'now'), {
    schedule: { kind: 'monthly', day: 'last', at: '09:00' },
  });
  assert.deepEqual(scheduleOf({ ...DEFAULT_WHEN, kind: 'every_hours', hours: 6 }, '2026-11-25T08:00:00Z'), {
    schedule: { kind: 'every_hours', hours: 6, from: '2026-11-25T08:00:00Z' },
  });
  assert.deepEqual(scheduleOf({ ...DEFAULT_WHEN, kind: 'every_hours', hours: 30 }, 'now'), { error: 'Every 1 to 24 hours.' });
  assert.equal(formOf(null).kind, 'manual');
  assert.deepEqual(formOf({ kind: 'weekly', days: ['tue'], at: '07:30' }).days, ['tue']);
});

test('the estimate matches the backend line', () => {
  assert.equal(estimateText(null, 4), 'Each run: up to 4 model calls');
  assert.equal(estimateText({ kind: 'weekly', days: ['mon'], at: '09:00' }, 4), 'Each run: up to 4 model calls (about 16 a month)');
  assert.equal(estimateText({ kind: 'daily', at: '09:00' }, 4), 'Each run: up to 4 model calls (about 120 a month)');
});

test("the model's tag says whose limit it counts against", () => {
  const free = { provider: 'openrouter' as const, model: 'nvidia/nemotron:free' };
  const otherFree = { provider: 'openrouter' as const, model: 'meta-llama/llama-3.3-70b-instruct:free' };
  const local = { provider: 'lmstudio' as const, model: 'qwen3-8b' };
  const paid = { provider: 'openrouter' as const, model: 'anthropic/claude-sonnet-4' };
  assert.equal(poolOf(free), 'openrouter:free');
  assert.equal(poolOf(paid), 'openrouter:paid');
  assert.equal(limitTag(null, free).label, "Shares chat's limit");
  const shared = limitTag(otherFree, free);
  assert.equal(shared.label, "Shares chat's limit");
  assert.match(shared.tooltip, /50-a-day/);
  assert.equal(limitTag(local, free).label, 'Local');
  assert.equal(limitTag(paid, free).label, 'Own limit');
  assert.equal(limitTag(null, null).label, 'No model');
});

test('views from the backend are read leniently and refused when malformed', () => {
  assert.equal(readAutomation(null), null);
  assert.equal(readAutomation({ id: 'a' }), null);
  const a = readAutomation({ id: 'a', name: 'N', state: 'manual', when: 'Only when you start it', schedule: null, active: null, last: null });
  assert.equal(a?.state, 'manual');
  const detail = readRunDetail({
    ...run({ state: 'waiting', waitReason: 'approval' }),
    steps: [{ runId: 'r1', idx: 0, kind: 'agent', state: 'done', output: { summary: 'x' }, calls: 3 }],
    approval: { id: 'ap', jobId: 'j1', runId: 'r1', title: 'Wants to make 2 changes', writes: [{ stepId: 's', tool: 'create_task', label: 'Add task A', tier: 'write', args: {}, preview: {} }], createdAt: '', expiresAt: '', decision: null, result: {} },
  });
  assert.equal(detail?.steps.length, 1);
  assert.equal(detail?.approval?.writes[0].label, 'Add task A');
});

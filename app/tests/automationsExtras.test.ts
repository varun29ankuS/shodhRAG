/**
 * Automations started by events, drafted, from templates and skills, and the replay of
 * a run. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/automationsExtras.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  DEFAULT_WHEN,
  REPLAY_GAP_MS,
  estimateText,
  formOf,
  isEventKind,
  plannedCalls,
  prefillFromDraft,
  prefillFromSkill,
  prefillFromTemplate,
  readAutomation,
  readDrafted,
  readRunDetail,
  readTemplates,
  readTrigger,
  replayMoves,
  savedMoves,
  scheduleOf,
  stateChip,
  stepLine,
  whenOf,
} from '../src/features/automations/model.ts';
import type { SavedMove, Step, Template } from '../src/features/automations/model.ts';

const NOW = Date.UTC(2026, 10, 25, 8, 0);

function step(patch: Partial<Step>): Step {
  return { runId: 'r1', idx: 0, kind: 'agent', state: 'done', input: {}, output: {}, calls: 0, ...patch };
}

test('an event is a When of its own, with the folder or the alert it names', () => {
  assert.equal(isEventKind('files_added'), true);
  assert.equal(isEventKind('weekly'), false);
  assert.deepEqual(whenOf({ ...DEFAULT_WHEN, kind: 'files_added', sourceId: 's1' }, 'now'), {
    schedule: null,
    trigger: { kind: 'files_added', sourceId: 's1' },
  });
  assert.deepEqual(whenOf({ ...DEFAULT_WHEN, kind: 'paper_found', alertId: 'a1' }, 'now'), {
    schedule: null,
    trigger: { kind: 'paper_found', alertId: 'a1' },
  });
  assert.deepEqual(whenOf({ ...DEFAULT_WHEN, kind: 'files_added' }, 'now'), { error: 'Pick a folder.' });
  assert.deepEqual(whenOf({ ...DEFAULT_WHEN, kind: 'paper_found' }, 'now'), { error: 'Pick a paper alert.' });
  // A time is still a time, and has no event.
  assert.deepEqual(whenOf({ ...DEFAULT_WHEN, kind: 'daily', at: '07:30' }, 'now'), {
    schedule: { kind: 'daily', at: '07:30' },
    trigger: null,
  });
  assert.deepEqual(whenOf({ ...DEFAULT_WHEN, kind: 'manual' }, 'now'), { schedule: null, trigger: null });
  assert.deepEqual(whenOf({ ...DEFAULT_WHEN, kind: 'weekly', days: [] }, 'now'), { error: 'Pick at least one day.' });
  assert.deepEqual(scheduleOf({ ...DEFAULT_WHEN, kind: 'paper_found', alertId: 'a1' }, 'now'), { schedule: null });
  // The form of a saved automation opens on what starts it.
  assert.equal(formOf(null, { kind: 'paper_found', alertId: 'a1' }).alertId, 'a1');
  assert.equal(formOf(null, { kind: 'paper_found', alertId: 'a1' }).kind, 'paper_found');
  assert.equal(formOf(null, { kind: 'files_added', sourceId: 's1' }).sourceId, 's1');
  assert.equal(formOf(null).kind, 'manual');
  assert.equal(formOf({ kind: 'daily', at: '09:00' }, { kind: 'files_added', sourceId: 's1' }).kind, 'daily');
});

test('what the backend says starts an automation is read, and refused when malformed', () => {
  assert.deepEqual(readTrigger({ kind: 'files_added', sourceId: 's1' }), { kind: 'files_added', sourceId: 's1' });
  assert.deepEqual(readTrigger({ kind: 'paper_found', alertId: 'a1' }), { kind: 'paper_found', alertId: 'a1' });
  assert.equal(readTrigger({ kind: 'files_added' }), null);
  assert.equal(readTrigger({ kind: 'cron', at: '* * * * *' }), null);
  assert.equal(readTrigger(null), null);
  const watching = readAutomation({
    id: 'a',
    name: 'Summarise new papers',
    state: 'watching',
    when: 'When an alert finds papers',
    schedule: null,
    trigger: { kind: 'paper_found', alertId: 'a1' },
    answerOnly: true,
    plannedCalls: 1,
  });
  assert.deepEqual(watching?.trigger, { kind: 'paper_found', alertId: 'a1' });
  assert.equal(watching?.answerOnly, true);
  assert.ok(watching);
  const chip = stateChip(watching, NOW);
  assert.equal(chip.label, 'Watching');
  assert.equal(chip.tooltip, 'When an alert finds papers');
  // An automation whose folder or alert went says so on its chip.
  const held = readAutomation({
    id: 'b',
    name: 'File notes',
    state: 'on_hold',
    holdReason: 'failing',
    last: { id: 'r9', state: 'skipped', summary: 'On hold: its folder left the Library.' },
  });
  assert.ok(held);
  assert.equal(stateChip(held, NOW).tooltip, 'On hold: its folder left the Library.');
  const failing = readAutomation({ id: 'c', name: 'N', state: 'on_hold', holdReason: 'failing', last: { id: 'r8', state: 'failed' } });
  assert.ok(failing);
  assert.equal(stateChip(failing, NOW).tooltip, 'Failed 3 times in a row');
});

test('the estimate of a run that only answers is one call', () => {
  assert.equal(plannedCalls(true), 1);
  assert.equal(plannedCalls(false), 4);
  assert.equal(estimateText(null, 1), 'Each run: 1 model call');
  assert.equal(estimateText({ kind: 'daily', at: '09:00' }, 1), 'Each run: 1 model call (about 30 a month)');
  assert.equal(estimateText(null, 4), 'Each run: up to 4 model calls');
});

test('a draft fills the form, and one that was not used keeps When as it was', () => {
  const drafted = readDrafted({
    name: 'Monday digest',
    instruction: 'Summarise what changed.',
    schedule: { kind: 'weekly', days: ['mon'], at: '08:30' },
    trigger: null,
    workspaceId: 'thesis',
    note: null,
  });
  assert.ok(drafted);
  const filled = prefillFromDraft(drafted, { ...DEFAULT_WHEN, kind: 'manual' });
  assert.equal(filled.name, 'Monday digest');
  assert.equal(filled.when.kind, 'weekly');
  assert.equal(filled.when.at, '08:30');
  assert.equal(filled.workspaceId, 'thesis');
  assert.equal(filled.note, null);
  assert.equal(filled.answerOnly, false);
  const onFiles = readDrafted({ name: 'File notes', instruction: 'Note each file.', trigger: { kind: 'files_added', sourceId: 's1' } });
  assert.ok(onFiles);
  assert.equal(prefillFromDraft(onFiles, DEFAULT_WHEN).when.sourceId, 's1');
  // Not drafted: the user's words are the instruction, and their When is not touched.
  const undrafted = readDrafted({ name: '', instruction: 'every full moon, howl', note: "Couldn't draft it; fill in When." });
  assert.ok(undrafted);
  const mine = { ...DEFAULT_WHEN, kind: 'daily' as const, at: '06:15' };
  const kept = prefillFromDraft(undrafted, mine);
  assert.deepEqual(kept.when, mine);
  assert.equal(kept.instruction, 'every full moon, howl');
  assert.equal(kept.note, "Couldn't draft it; fill in When.");
  assert.equal(readDrafted({ name: 'x' }), null);
  assert.equal(readDrafted(null), null);
});

const TEMPLATES = [
  {
    id: 'summarise_papers',
    name: 'Summarise new papers',
    when: 'When an alert finds papers',
    instruction: 'Summarise each paper listed below.',
    schedule: null,
    triggerKind: 'paper_found',
    needsWorkspace: false,
    answerOnly: true,
    plannedCalls: 1,
    estimate: '1 model call a run',
  },
  {
    id: 'weekly_digest',
    name: 'Weekly workspace digest',
    when: 'Every week on Mon at 09:00',
    instruction: 'Write a short digest of this workspace.',
    schedule: { kind: 'weekly', days: ['mon'], at: '09:00' },
    triggerKind: null,
    needsWorkspace: true,
    answerOnly: false,
    plannedCalls: 4,
    estimate: 'up to 4 model calls a run (about 16 a month)',
  },
];

test('a template opens the form filled in, with what it needs picked or left to pick', () => {
  const [papers, digest] = readTemplates(TEMPLATES) as [Template, Template];
  assert.equal(papers.triggerKind, 'paper_found');
  assert.equal(digest.needsWorkspace, true);
  const have = { folderIds: ['s1'], alertIds: ['a1', 'a2'], workspaceIds: ['thesis', 'side'] };
  const forPapers = prefillFromTemplate(papers, have);
  assert.equal(forPapers.name, 'Summarise new papers');
  assert.equal(forPapers.when.kind, 'paper_found');
  assert.equal(forPapers.when.alertId, 'a1');
  assert.equal(forPapers.answerOnly, true);
  assert.equal(forPapers.workspaceId, null);
  assert.deepEqual(whenOf(forPapers.when, 'now'), { schedule: null, trigger: { kind: 'paper_found', alertId: 'a1' } });
  assert.equal(estimateText(null, plannedCalls(forPapers.answerOnly)), 'Each run: 1 model call');
  const forDigest = prefillFromTemplate(digest, have);
  assert.equal(forDigest.when.kind, 'weekly');
  assert.deepEqual(forDigest.when.days, ['mon']);
  assert.equal(forDigest.workspaceId, 'thesis');
  assert.equal(forDigest.answerOnly, false);
  // With no alert yet, the form says to pick one rather than saving something else.
  const none = prefillFromTemplate(papers, { folderIds: [], alertIds: [], workspaceIds: [] });
  assert.deepEqual(whenOf(none.when, 'now'), { error: 'Pick a paper alert.' });
  assert.equal(prefillFromTemplate(digest, { folderIds: [], alertIds: [], workspaceIds: [] }).workspaceId, null);
  // Malformed templates are left out.
  assert.equal(readTemplates([{ id: 'x' }, null, 3]).length, 0);
  assert.equal(readTemplates('no').length, 0);
});

test('a skill becomes the instruction of a new automation the user completes', () => {
  const filled = prefillFromSkill({ name: ' paper-review ', description: ' Review a paper for method and statistics errors. ' });
  assert.equal(filled.name, 'paper-review');
  assert.equal(filled.instruction, 'Do what the skill "paper-review" is for: Review a paper for method and statistics errors.');
  assert.equal(filled.when.kind, 'manual', 'only when started, until the user picks a time');
  assert.equal(filled.workspaceId, null);
  assert.equal(filled.answerOnly, false);
  assert.equal(prefillFromSkill({ name: 'notes', description: '' }).instruction, 'Do what the skill "notes" is for.');
  const long = prefillFromSkill({ name: 'n'.repeat(200), description: 'd'.repeat(9000) });
  assert.equal(long.name.length, 80);
  assert.equal(long.instruction.length, 4000);
});

test('a run page reads what arrived, what the agent said and what it told', () => {
  const detail = readRunDetail({
    id: 'r1',
    state: 'succeeded',
    trigger: 'event',
    steps: [
      { runId: 'r1', idx: 0, kind: 'input', state: 'done', input: { subjects: [{ title: 'A' }, { title: 'B' }], more: 3 }, output: {}, calls: 0 },
      { runId: 'r1', idx: 1, kind: 'agent', state: 'done', input: {}, output: { summary: 'Two papers.', navigations: [] }, calls: 1 },
      { runId: 'r1', idx: 2, kind: 'notify', state: 'done', input: {}, output: { summary: 'Two papers.' }, calls: 0 },
    ],
  });
  assert.ok(detail);
  assert.deepEqual(detail.steps.map(stepLine), [
    { label: 'Arrived', text: '5 items' },
    { label: 'Agent', text: 'Two papers.' },
    { label: 'Told you', text: 'Two papers.' },
  ]);
  assert.deepEqual(stepLine(step({ kind: 'input', input: { subjects: [{}] } })), { label: 'Arrived', text: '1 item' });
  assert.deepEqual(stepLine(step({ kind: 'agent', state: 'running' })), { label: 'Agent', text: 'running' });
});

test('show me replays the places through the gate one by one', async () => {
  const steps = [
    step({ kind: 'input', input: { subjects: [] } }),
    step({
      idx: 1,
      output: {
        navigations: [
          { view: 'library', target: { kind: 'document', path: 'a.pdf' }, label: 'a.pdf' },
          { view: 'tasks', target: null, label: null },
          'not a move',
          { view: 'library', target: { kind: 'document', path: 'b.pdf' }, label: 'b.pdf' },
        ],
      },
    }),
    step({ idx: 2, kind: 'notify', output: { navigations: [{ view: 'activity' }] } }),
  ];
  const moves = savedMoves(steps);
  assert.deepEqual(moves.map(m => m.view), ['library', 'tasks', 'library'], 'the agent step\'s, in order');
  assert.equal(moves[0].label, 'a.pdf');
  assert.equal(savedMoves([step({ output: {} })]).length, 0);

  const log: string[] = [];
  const deps = (typingAfter: number) => {
    let went = 0;
    return {
      go: (move: SavedMove) => {
        went += 1;
        log.push(`go ${String(move.view)}`);
      },
      wait: async (ms: number) => {
        log.push(`wait ${ms}`);
      },
      typing: () => went >= typingAfter,
    };
  };
  // Every place, one at a time, with a pause between them and none before the first.
  assert.equal(await replayMoves(moves, deps(99)), 3);
  assert.deepEqual(log, ['go library', `wait ${REPLAY_GAP_MS}`, 'go tasks', `wait ${REPLAY_GAP_MS}`, 'go library']);
  // The user starts typing after the first: nothing more moves.
  log.length = 0;
  assert.equal(await replayMoves(moves, deps(1)), 1);
  assert.deepEqual(log, ['go library', `wait ${REPLAY_GAP_MS}`]);
  // Typing from the start: nothing moves at all.
  log.length = 0;
  assert.equal(await replayMoves(moves, deps(0)), 0);
  assert.deepEqual(log, []);
  assert.equal(await replayMoves([], deps(99)), 0);
  // A caller may pace it.
  log.length = 0;
  await replayMoves(moves.slice(0, 2), { ...deps(99), gapMs: 10 });
  assert.deepEqual(log, ['go library', 'wait 10', 'go tasks']);
});

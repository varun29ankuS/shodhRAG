/**
 * The floating chat's protocol (lib/dockProtocol): main's snapshot out, the dock's actions in.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/dockProtocol.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import {
  MAX_DRAFT_CHARS,
  MAX_SNAPSHOT_BYTES,
  fitTranscript,
  parseDockAction,
  parseDockSnapshot,
  snapshotOf,
} from '../src/lib/dockProtocol.ts';
import type { DockAction, SnapshotInput } from '../src/lib/dockProtocol.ts';
import type { TranscriptState } from '../src/features/agent/reducer.ts';
import type { ChatMessage } from '../src/features/ask/types.ts';
import type { RuntimeSetupState } from '../src/features/setup/runtimeSetupHost.ts';

const RUST_MAX_RELAY_BYTES = (() => {
  const source = readFileSync(new URL('../src-tauri/src/dock_layer.rs', import.meta.url), 'utf8');
  const found = source.match(/pub const MAX_RELAY_BYTES: usize = ([\d_]+);/);
  assert.ok(found, 'dock_layer.rs declares MAX_RELAY_BYTES');
  return Number(found[1].replace(/_/g, ''));
})();

function fixtureTranscript(name: string): TranscriptState {
  const path = fileURLToPath(new URL(`./fixtures/transcripts/${name}`, import.meta.url));
  return JSON.parse(readFileSync(path, 'utf8')).expected as TranscriptState;
}

/** A transcript of `count` steps, each a copy of the fixture's first step with a large file write. */
function longTranscript(count: number): TranscriptState {
  const base = fixtureTranscript('01-cited-answer.json');
  const [firstId] = Object.keys(base.steps);
  const first = base.steps[firstId];
  const steps: TranscriptState['steps'] = {};
  const blocks: TranscriptState['blocks'] = [];
  for (let i = 0; i < count; i += 1) {
    const id = `step-${i}`;
    steps[id] = { ...first, id, args: { path: `page-${i}.html`, content: `<p>${'x'.repeat(4000)}</p>` } };
    blocks.push({ kind: 'text', id: `m${i}`, text: `Writing page ${i}.` });
    blocks.push({ kind: 'step', stepId: id });
  }
  return { ...base, status: 'running', blocks, steps };
}

function message(role: 'user' | 'assistant', content: string, transcript?: TranscriptState): ChatMessage {
  return { id: `${role}-${content}`, role, content, timestamp: '2026-11-22T10:00:00Z', ...(transcript ? { transcript } : {}) } as ChatMessage;
}

const RUNTIME: RuntimeSetupState = {
  status: null,
  statusError: null,
  installed: true,
  installing: false,
  progress: null,
  installError: null,
  installOffline: false,
  justInstalled: null,
  installedAt: null,
};

function input(messages: ChatMessage[], extra: Partial<SnapshotInput> = {}): SnapshotInput {
  return {
    conversationId: 'c1',
    title: 'Landing page',
    messages,
    isStreaming: false,
    streamingConversationId: null,
    sideRun: null,
    readOnlyConnector: null,
    draft: '',
    dictating: false,
    voiceSupported: true,
    open: true,
    unread: 0,
    theme: 'dark',
    runtime: RUNTIME,
    work: 'research',
    permission: 'ask',
    codeAvailable: true,
    model: {
      label: 'Gemma 4 31B',
      ready: true,
      dot: 'green',
      choices: [
        { id: 'pick:free', title: 'Free', detail: 'Gemma 4 31B · OpenRouter', selected: true },
        { id: 'model:openrouter:meta/llama-4:free', title: 'Llama 4', detail: 'Recent · OpenRouter', selected: false },
      ],
    },
    sendKey: 'enter',
    ...extra,
  };
}

test('every action round-trips', () => {
  const actions: DockAction[] = [
    { kind: 'send' },
    { kind: 'steer' },
    { kind: 'cancel' },
    { kind: 'approve', id: 'call_0123-abc', allow: true },
    { kind: 'approve', id: 'step-7', allow: false },
    { kind: 'draft', text: 'Make the header blue' },
    { kind: 'draft', text: '' },
    { kind: 'dictate', on: true },
    { kind: 'dictate', on: false },
    { kind: 'open', open: true },
    { kind: 'open', open: false },
    { kind: 'openInAsk' },
    { kind: 'close' },
    { kind: 'approve', id: 'step-9', allow: true, forConversation: true },
    { kind: 'approve', id: 'step-9', allow: true, reviewToken: 'rt_0123' },
    { kind: 'runtime', op: 'install' },
    { kind: 'runtime', op: 'refresh' },
    { kind: 'runtime', op: 'dismissInstalled' },
    { kind: 'runtimeInstalled', installed: false },
    { kind: 'review', code: 'fn main() {}', language: 'rust' },
    { kind: 'openUrl', url: 'https://example.com/page?q=1' },
    { kind: 'stopCommand', id: 'cmd-42' },
    { kind: 'openSettings' },
    { kind: 'setWork', work: 'research' },
    { kind: 'setWork', work: 'code' },
    { kind: 'setWork', work: 'creator' },
    { kind: 'setPermission', mode: 'ask' },
    { kind: 'setPermission', mode: 'auto' },
    { kind: 'pickModel' },
    { kind: 'setModel', id: 'pick:free' },
    { kind: 'setModel', id: 'model:openrouter:meta/llama-4:free' },
  ];
  for (const action of actions) {
    assert.deepEqual(parseDockAction(JSON.parse(JSON.stringify(action))), action);
  }
});

test('unknown or malformed actions are refused', () => {
  for (const bad of [
    { kind: 'eval' },
    { kind: 'send', extra: 1 },
    { kind: 'draft', text: 'x'.repeat(MAX_DRAFT_CHARS + 1) },
    { kind: 'draft' },
    { kind: 'draft', text: 5 },
    { kind: 'approve', id: '', allow: true },
    { kind: 'approve', id: 'has space', allow: true },
    { kind: 'approve', id: 'x'.repeat(129), allow: true },
    { kind: 'approve', id: 'a', allow: 'yes' },
    { kind: 'approve', id: 'a' },
    { kind: 'dictate', on: 1 },
    { kind: 'approve', id: 'a', allow: true, reviewToken: 'has space' },
    { kind: 'approve', id: 'a', allow: true, forConversation: 'yes' },
    { kind: 'runtime', op: 'uninstall' },
    { kind: 'runtime' },
    { kind: 'review', code: 'x'.repeat(200001), language: 'ts' },
    { kind: 'review', code: 'x', language: 'a b' },
    { kind: 'openUrl', url: 'file:///C:/Users/' },
    { kind: 'openUrl', url: 'javascript:alert(1)' },
    { kind: 'openUrl', url: 'https://user:pw@example.com/' },
    { kind: 'openUrl', url: 'not a url' },
    // The mode menu: only the three kinds of work, and never Creator as a permission.
    { kind: 'setWork', work: 'yolo' },
    { kind: 'setWork' },
    { kind: 'setPermission', mode: 'creator' },
    { kind: 'setPermission', mode: 'bypass' },
    { kind: 'pickModel', model: 'x' },
    // The model menu: an id of printable characters without spaces, and nothing else.
    { kind: 'setModel' },
    { kind: 'setModel', id: '' },
    { kind: 'setModel', id: 'two words' },
    { kind: 'setModel', id: 'x'.repeat(257) },
    { kind: 'setModel', id: 7 },
    { kind: 'setModel', id: 'pick:free', sessionOverride: true },
    { kind: 'stopCommand', id: '' },
    { kind: 'toString' },
    { kind: '__proto__' },
    [],
    null,
    'send',
    42,
  ]) {
    assert.equal(parseDockAction(bad), null, JSON.stringify(bad));
  }
  assert.equal(MAX_DRAFT_CHARS, 20000);
});

test('the snapshot carries only the last exchange', () => {
  const first = fixtureTranscript('01-cited-answer.json');
  const second = fixtureTranscript('02-approval-steer-interrupt.json');
  const third = fixtureTranscript('03-code-commands-error.json');
  const snapshot = snapshotOf(input([
    message('user', 'first question'),
    message('assistant', 'first answer', first),
    message('user', 'second question'),
    message('assistant', 'second answer', second),
    message('user', 'third question'),
    message('assistant', 'third answer', third),
  ]));
  assert.deepEqual(snapshot.lastUser, { content: 'third question' });
  assert.deepEqual(snapshot.transcript, third);
  assert.equal(snapshot.trimmed, false);
  assert.deepEqual(snapshot.runtime, RUNTIME);
  const text = JSON.stringify(snapshot);
  assert.doesNotMatch(text, /first question|second question/);
  // Through JSON (the relay) and back, the dock reads the same snapshot.
  assert.deepEqual(parseDockSnapshot(JSON.parse(text)), snapshot);
});

test('a 200-step transcript fits the relay, newest activity kept', () => {
  assert.equal(MAX_SNAPSHOT_BYTES, RUST_MAX_RELAY_BYTES);
  const long = longTranscript(200);
  assert.ok(JSON.stringify(long).length > MAX_SNAPSHOT_BYTES, 'the fixture is larger than the relay takes');
  const snapshot = snapshotOf(input([message('user', 'build it'), message('assistant', '', long)], { isStreaming: true, streamingConversationId: 'c1' }));
  const bytes = new TextEncoder().encode(JSON.stringify(snapshot)).length;
  assert.ok(bytes <= MAX_SNAPSHOT_BYTES, `${bytes} bytes`);
  const transcript = snapshot.transcript;
  assert.ok(transcript);
  // The last block (the step running now) is still there, with its step.
  assert.deepEqual(transcript.blocks.at(-1), { kind: 'step', stepId: 'step-199' });
  assert.ok(transcript.steps['step-199']);
  // Every block's step is kept.
  for (const block of transcript.blocks) {
    if (block.kind === 'step') assert.ok(transcript.steps[block.stepId], block.stepId);
  }
  // The chat is told that earlier work was left out, so it can say so.
  assert.equal(snapshot.trimmed, true);
  // The in-page chat reads it whole.
  const whole = snapshotOf(input([message('user', 'build it'), message('assistant', '', long)]), false);
  assert.equal(whole.transcript, long);
  assert.equal(whole.trimmed, false);
  // A small transcript is never changed.
  const small = fixtureTranscript('02-approval-steer-interrupt.json');
  assert.equal(fitTranscript(small, MAX_SNAPSHOT_BYTES), small);
});

test('dictation state travels in the snapshot', () => {
  const on = snapshotOf(input([message('user', 'q')], { dictating: true, draft: 'make the logo', voiceSupported: true }));
  assert.equal(on.dictating, true);
  assert.equal(on.voiceSupported, true);
  assert.equal(on.draft, 'make the logo');
  const parsed = parseDockSnapshot(JSON.parse(JSON.stringify(on)));
  assert.equal(parsed?.dictating, true);
  const off = snapshotOf(input([], { dictating: false, voiceSupported: false }));
  assert.equal(off.dictating, false);
  assert.equal(off.voiceSupported, false);
  assert.equal(off.lastUser, null);
  assert.equal(off.transcript, null);
});

test('busy elsewhere and the side question come from main as the in-page chat reads them', () => {
  assert.equal(snapshotOf(input([], { streamingConversationId: 'other', isStreaming: false })).busyElsewhere, true);
  assert.equal(snapshotOf(input([], { streamingConversationId: 'c1', isStreaming: true })).busyElsewhere, false);
  const side = snapshotOf(input([], { sideRun: { label: 'Revenue by quarter' } }));
  assert.equal(side.busyElsewhere, true);
  assert.equal(side.sideRunLabel, 'Revenue by quarter');
});

test('a malformed snapshot is not read', () => {
  const good = snapshotOf(input([message('user', 'q')]));
  for (const bad of [
    null,
    { ...good, v: 2 },
    { ...good, theme: 'blue' },
    { ...good, unread: -1 },
    { ...good, draft: 5 },
    { ...good, transcript: { blocks: 'no' } },
    { ...good, lastUser: { content: 1 } },
    { ...good, trimmed: 'no' },
    { ...good, runtime: null },
    { ...good, runtime: { ...RUNTIME, installing: 'yes' } },
    { ...good, work: 'yolo' },
    { ...good, permission: 'creator' },
    { ...good, codeAvailable: 'yes' },
    { ...good, model: null },
    { ...good, model: { label: 1, ready: true } },
    // The model's state is one of the four dots, and its menu a bounded list of well-formed rows.
    { ...good, model: { label: 'x', ready: true, choices: [] } },
    { ...good, model: { label: 'x', ready: true, dot: 'blue', choices: [] } },
    { ...good, model: { label: 'x', ready: true, dot: 'green' } },
    { ...good, model: { label: 'x', ready: true, dot: 'green', choices: [{ id: 'a b', title: 't', detail: 'd', selected: false }] } },
    { ...good, model: { label: 'x', ready: true, dot: 'green', choices: [{ id: 'a', title: 't'.repeat(161), detail: 'd', selected: false }] } },
    { ...good, model: { label: 'x', ready: true, dot: 'green', choices: [{ id: 'a', title: 't', detail: 'd' }] } },
    { ...good, model: { label: 'x', ready: true, dot: 'green', choices: Array.from({ length: 13 }, (_, i) => ({ id: `m${i}`, title: 't', detail: 'd', selected: false })) } },
    { ...good, sendKey: 'altEnter' },
  ]) {
    assert.equal(parseDockSnapshot(bad), null, JSON.stringify(bad)?.slice(0, 80));
  }
});

test('a finished answer always fits the relay, so the chat never stays on a run that ended', () => {
  // A long answer, its sources, its reasoning and its checks: everything the relay used to send
  // whole. A snapshot over the relay's limit was refused, and the floating chat kept the last
  // one it got, still "Working…" with its clock running, after the answer had finished.
  const base = longTranscript(4);
  const passages = Array.from({ length: 60 }, (_, n) => ({ n: n + 1, file: `f${n}.pdf`, path: `/docs/f${n}.pdf`, page: null, heading: null, score: 1, text: 'p'.repeat(6000) }));
  const done: TranscriptState = {
    ...base,
    status: 'completed',
    durationMs: 229_000,
    blocks: [...base.blocks, { kind: 'text', id: 'final', text: 'z'.repeat(300_000) }],
    passages,
    reasoning: { segments: [{ at: 0, text: 'r'.repeat(32_000) }], truncated: false },
  } as TranscriptState;
  assert.ok(new TextEncoder().encode(JSON.stringify(done)).length > MAX_SNAPSHOT_BYTES, 'the fixture is larger than the relay takes');
  const snapshot = snapshotOf(input([message('user', 'build it'), message('assistant', 'done', done)]));
  const bytes = new TextEncoder().encode(JSON.stringify(snapshot)).length;
  assert.ok(bytes <= MAX_SNAPSHOT_BYTES, `${bytes} bytes`);
  // The run's end reaches the chat: its status, duration and model, read as one.
  const parsed = parseDockSnapshot(JSON.parse(JSON.stringify(snapshot)));
  assert.ok(parsed?.transcript);
  assert.equal(parsed.transcript.status, 'completed');
  assert.equal(parsed.transcript.durationMs, 229_000);
  assert.equal(snapshot.trimmed, true);
  // The same holds while it runs, at any budget the snapshot leaves.
  for (const budget of [MAX_SNAPSHOT_BYTES / 2, 20_000, 4_000]) {
    const fitted = fitTranscript({ ...done, status: 'running' }, budget);
    assert.ok(new TextEncoder().encode(JSON.stringify(fitted)).length <= budget, String(budget));
    assert.equal(fitted.status, 'running');
  }
});

test("the launch probe's snapshot is one the floating chat reads", () => {
  // The Windows and macOS launch tests publish this snapshot (launch_probe.rs) and check the
  // open chat's rectangle: a snapshot the dock refuses leaves it closed and fails them.
  const probe = readFileSync(new URL('../src-tauri/src/launch_probe.rs', import.meta.url), 'utf8');
  const body = probe.slice(probe.indexOf('fn snapshot()'));
  const start = body.indexOf('serde_json::json!(') + 'serde_json::json!('.length;
  const end = body.indexOf('\n    })') + '\n    }'.length;
  const parsed = parseDockSnapshot(JSON.parse(body.slice(start, end)));
  assert.ok(parsed, 'parseDockSnapshot refused the launch probe snapshot');
  assert.equal(parsed.open, true);
});

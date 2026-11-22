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
    zoom: 1,
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
    { ...good, zoom: 0 },
    { ...good, unread: -1 },
    { ...good, draft: 5 },
    { ...good, transcript: { blocks: 'no' } },
    { ...good, lastUser: { content: 1 } },
  ]) {
    assert.equal(parseDockSnapshot(bad), null, JSON.stringify(bad)?.slice(0, 80));
  }
});

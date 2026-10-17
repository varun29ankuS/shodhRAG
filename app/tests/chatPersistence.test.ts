/**
 * Chat persistence decision tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/chatPersistence.test.ts
 *
 * Replays the order that lost a finished answer: the grounding report folded
 * at an animation frame (display only, the run still live), then
 * `run_finished` applied at once (the finished answer, to be saved), with the
 * frame render's save effect still pending when the second update is made.
 * The provider's effect is modelled by `effect`: the same `saveDue` /
 * `messagesToSave` calls `ChatSessionContext.saveView` makes, against one
 * render's view.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  changeMessages,
  emptyView,
  loadConversation,
  markChange,
  messagesToSave,
  NO_CHANGES,
  publishMessage,
  saveDue,
  upsertById,
} from '../src/features/ask/chatPersistence.ts';
import type { PersistView } from '../src/features/ask/chatPersistence.ts';
import type { AgentEvent } from '../src/features/agent/events.ts';
import { fromPersisted, initialTranscript, reduceAll, toCheckpoint, toPersisted } from '../src/features/agent/reducer.ts';

interface Msg {
  id: string;
  text: string;
  live: boolean;
}

interface Stored {
  id: string;
  text: string;
  checkpoint?: boolean;
}

type View = PersistView<Msg, Stored>;

interface Save {
  conversationId: string;
  messages: Stored[];
  immediate: boolean;
}

/** The chat provider's save path: one ref of the latest seq written, and the writes made. */
function store() {
  let lastSavedSeq = 0;
  const saves: Save[] = [];
  const effect = (v: View) => {
    const due = saveDue(v.persist, lastSavedSeq);
    if (!due) return;
    lastSavedSeq = due.seq;
    if (!v.conversationId) return;
    saves.push({
      conversationId: v.conversationId,
      messages: messagesToSave(v, m => !m.live, m => ({ id: m.id, text: m.text })),
      immediate: due.immediate,
    });
  };
  return { effect, saves };
}

const user: Msg = { id: 'u1', text: 'What is closure augmentation?', live: false };

function loaded(conversationId: string, messages: Msg[] = []): View {
  return loadConversation(emptyView<Msg, Stored>(), conversationId, messages, {});
}

test('a stale render cannot consume the mark of a newer state (grounding frame, then run_finished)', () => {
  const { effect, saves } = store();
  let v = loaded('c1');
  v = publishMessage(v, 'c1', user, 'debounced');
  effect(v); // the user message is saved
  v = publishMessage(v, 'c1', { id: 'a1', text: 'Closure augmentation adds…', live: true }, 'none');
  const frameRender = v; // R1: grounding folded at a frame, answer still live; its effect is pending
  v = publishMessage(v, 'c1', { id: 'a1', text: 'Closure augmentation adds… [1]', live: false }, 'immediate');
  const finishedRender = v; // R2: run_finished, the finished answer

  // React flushes R1's pending effect before rendering R2.
  effect(frameRender);
  effect(finishedRender);

  assert.equal(saves.length, 2);
  assert.deepEqual(saves[0].messages.map(m => m.id), ['u1']);
  const last = saves[saves.length - 1];
  assert.deepEqual(last.messages, [
    { id: 'u1', text: 'What is closure augmentation?' },
    { id: 'a1', text: 'Closure augmentation adds… [1]' },
  ]);
  assert.equal(last.immediate, true, 'a finished answer is saved without the debounce');
});

test('a stale view saved after a newer mark writes only its own data and leaves the newer change due', () => {
  const { effect, saves } = store();
  let v = loaded('c1', [user]);
  v = publishMessage(v, 'c1', { id: 'a1', text: 'draft', live: true }, 'none');
  const older = changeMessages(v, 'c1', ms => [...ms, { id: 'n1', text: 'OCR notice', live: false }], 'debounced');
  const newer = publishMessage(older, 'c1', { id: 'a1', text: 'final', live: false }, 'immediate');
  effect(older);
  assert.deepEqual(saves[0].messages.map(m => m.id), ['u1', 'n1']);
  assert.equal(saves[0].immediate, false);
  // The older save consumed only the older seq: the finished answer is still due.
  assert.ok(saveDue(newer.persist, older.persist.seq));
  effect(newer);
  assert.deepEqual(saves[1].messages.map(m => m.text), ['What is closure augmentation?', 'final', 'OCR notice']);
  assert.equal(saves[1].immediate, true);
});

test('a render whose changes were already saved saves nothing', () => {
  const { effect, saves } = store();
  let v = loaded('c1');
  v = publishMessage(v, 'c1', user, 'debounced');
  effect(v);
  effect(v);
  v = publishMessage(v, 'c1', { id: 'a1', text: 'partial', live: true }, 'none');
  effect(v);
  assert.equal(saves.length, 1, 'streaming frames never write');
});

test('an immediate change stays immediate when a debounced change lands before the save', () => {
  const { effect, saves } = store();
  let v = loaded('c1', [user]);
  v = publishMessage(v, 'c1', { id: 'a1', text: 'final', live: false }, 'immediate');
  v = changeMessages(v, 'c1', ms => ms.map(m => (m.id === 'a1' ? { ...m, text: 'final (threads)' } : m)), 'debounced');
  effect(v);
  assert.equal(saves.length, 1);
  assert.equal(saves[0].immediate, true);
  assert.equal(saves[0].messages[1].text, 'final (threads)');
});

test('a checkpoint saves the running answer at once, a revision saves it again, and the finished answer replaces it', () => {
  const { effect, saves } = store();
  let v = loaded('c1', [user]);
  v = publishMessage(v, 'c1', { id: 'a1', text: 'first draft', live: true }, 'none');
  effect(v);
  assert.equal(saves.length, 0);

  // grounding_started, round 0: text complete, run still live.
  v = publishMessage(v, 'c1', { id: 'a1', text: 'first draft', live: true }, 'immediate', { id: 'a1', text: 'first draft', checkpoint: true });
  effect(v);
  assert.equal(saves.length, 1);
  assert.equal(saves[0].immediate, true);
  assert.deepEqual(saves[0].messages[1], { id: 'a1', text: 'first draft', checkpoint: true });

  // The revision streams (display only), then its own check starts.
  v = publishMessage(v, 'c1', { id: 'a1', text: 'first draft revis', live: true }, 'none');
  effect(v);
  assert.equal(saves.length, 1);
  v = publishMessage(v, 'c1', { id: 'a1', text: 'revised', live: true }, 'immediate', { id: 'a1', text: 'revised', checkpoint: true });
  effect(v);
  assert.equal(saves.length, 2);
  assert.deepEqual(saves[1].messages[1], { id: 'a1', text: 'revised', checkpoint: true });

  // run_finished: the settled answer, not the checkpoint.
  v = publishMessage(v, 'c1', { id: 'a1', text: 'revised [2]', live: false }, 'immediate');
  effect(v);
  assert.equal(saves.length, 3);
  assert.deepEqual(saves[2].messages[1], { id: 'a1', text: 'revised [2]' });
  assert.equal(saves[2].immediate, true);
});

test('a save during the check (another message changed) keeps the checkpointed answer', () => {
  const { effect, saves } = store();
  let v = loaded('c1', [user]);
  v = publishMessage(v, 'c1', { id: 'a1', text: 'answer', live: true }, 'immediate', { id: 'a1', text: 'answer', checkpoint: true });
  effect(v);
  v = changeMessages(v, 'c1', ms => ms.map(m => (m.id === 'u1' ? { ...m, text: 'edited' } : m)), 'debounced');
  effect(v);
  assert.deepEqual(saves[1].messages, [
    { id: 'u1', text: 'edited' },
    { id: 'a1', text: 'answer', checkpoint: true },
  ]);
});

test('a running answer without a checkpoint is not stored', () => {
  const v = publishMessage(loaded('c1', [user]), 'c1', { id: 'a1', text: 'streaming', live: true }, 'debounced');
  assert.deepEqual(messagesToSave(v, m => !m.live, m => ({ id: m.id, text: m.text })), [{ id: 'u1', text: user.text }]);
});

test('switching conversations writes nothing', () => {
  const { effect, saves } = store();
  let v = loaded('c1');
  v = publishMessage(v, 'c1', user, 'debounced');
  effect(v);
  const before = saves.length;
  v = loadConversation(v, 'c2', [{ id: 'x1', text: 'older chat', live: false }], {});
  effect(v);
  v = loadConversation(v, 'c1', [user], {});
  effect(v);
  assert.equal(saves.length, before);
});

test('a change not yet saved when the conversation switches is saved to the conversation it was made in', () => {
  const { effect, saves } = store();
  let v = loaded('c1', [user]);
  v = publishMessage(v, 'c1', { id: 'n1', text: 'notice', live: false }, 'debounced');
  // The switch's layout effect saves the view being left, then loads the other one.
  effect(v);
  v = loadConversation(v, 'c2', [], {});
  effect(v);
  assert.equal(saves.length, 1);
  assert.equal(saves[0].conversationId, 'c1');
  assert.deepEqual(saves[0].messages.map(m => m.id), ['u1', 'n1']);
});

test('changes for another conversation leave the view and its mark alone', () => {
  const v = loaded('c1', [user]);
  assert.equal(publishMessage(v, 'c2', { id: 'a1', text: 'x', live: false }, 'immediate'), v);
  assert.equal(changeMessages(v, 'c2', () => [], 'debounced'), v);
});

test('marks count changes and remember the latest immediate one', () => {
  assert.deepEqual(markChange(NO_CHANGES, 'none'), NO_CHANGES);
  const a = markChange(NO_CHANGES, 'debounced');
  const b = markChange(a, 'immediate');
  const c = markChange(b, 'debounced');
  assert.deepEqual([a, b, c], [{ seq: 1, immediateSeq: 0 }, { seq: 2, immediateSeq: 2 }, { seq: 3, immediateSeq: 2 }]);
  assert.deepEqual(saveDue(c, 1), { seq: 3, immediate: true });
  assert.deepEqual(saveDue(c, 2), { seq: 3, immediate: false });
  assert.equal(saveDue(c, 3), null);
});

test('upsertById replaces in place or appends', () => {
  const list = [{ id: 'a', v: 1 }, { id: 'b', v: 2 }];
  assert.deepEqual(upsertById(list, { id: 'a', v: 9 }), [{ id: 'a', v: 9 }, { id: 'b', v: 2 }]);
  assert.deepEqual(upsertById(list, { id: 'c', v: 3 }), [...list, { id: 'c', v: 3 }]);
});

test('a transcript stored while its answer was being checked restores as interrupted before the check, text intact', () => {
  const events: AgentEvent[] = [
    { type: 'run_started', runId: 'run-1', sessionId: 's1', model: 'm', atMs: 1_000, warning: null },
    { type: 'text_delta', runId: 'run-1', messageId: 'm1', delta: 'The answer. [1]' },
    { type: 'grounding_started', runId: 'run-1', round: 0 },
  ];
  const live = reduceAll(initialTranscript('run-1', 1_000), events);
  assert.equal(live.status, 'running');
  assert.equal(live.checking, true);
  const stored = JSON.parse(JSON.stringify(toPersisted(toCheckpoint(live))));
  assert.equal(stored.savedWhileChecking, true);
  assert.equal(stored.checking, false);
  const restored = fromPersisted(stored);
  assert.ok(restored);
  assert.equal(restored.status, 'aborted');
  assert.match(restored.error ?? '', /being checked against its sources/);
  assert.equal('savedWhileChecking' in restored, false);
  assert.deepEqual(restored.blocks, live.blocks);
});

test('a transcript stored live without a checkpoint keeps the existing interrupted message', () => {
  const live = { ...initialTranscript('run-2', 1_000), status: 'running' as const };
  const restored = fromPersisted(JSON.parse(JSON.stringify(toPersisted(live))));
  assert.ok(restored);
  assert.equal(restored.error, 'The app closed before this answer finished.');
});

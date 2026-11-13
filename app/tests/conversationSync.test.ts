/**
 * Conversations are saved by the app, not the window. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/conversationSync.test.ts
 *
 * The agent session saves each question and answer itself; a window writes only the
 * messages it changed and reloads a conversation when the backend says it changed
 * (`conversation_changed`). The Rust side (`conversation_turns.rs`) has
 * `an_answer_is_saved_without_the_window` and `two_senders_lose_no_turns`.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  CONVERSATION_CHANGED_EVENT,
  applyPatch,
  autoTitle,
  changedConversationId,
  mergePatches,
  reloadedConversation,
  withPatch,
} from '../src/hooks/conversationSync.ts';
import type { MessagePatch } from '../src/hooks/conversationSync.ts';
import { ConversationSaves } from '../src/hooks/conversationSaves.ts';
import type { Conversation, ConversationMessage } from '../src/hooks/useConversations.ts';
import { backendSavesTurn, turnPresave } from '../src/features/ask/chatPersistence.ts';
import { fromStored, questionMessage, startedAnswer, toStored } from '../src/features/ask/messageStore.ts';
const read = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8');

function message(id: string, content = id, role: ConversationMessage['role'] = 'user'): ConversationMessage {
  return { id, role, content, timestamp: '2026-11-13T10:00:00.000Z' };
}

function conversation(messages: ConversationMessage[], extra: Partial<Conversation> = {}): Conversation {
  return {
    id: 'c1',
    title: 'New Chat',
    messages,
    createdAt: '2026-11-13T09:00:00.000Z',
    updatedAt: '2026-11-13T09:00:00.000Z',
    pinned: false,
    ...extra,
  };
}

test('a conversation_changed payload names the conversation', () => {
  assert.equal(CONVERSATION_CHANGED_EVENT, 'conversation_changed');
  assert.equal(changedConversationId({ id: 'c1' }), 'c1');
  for (const bad of [null, 'c1', {}, { id: '' }, { id: 7 }]) assert.equal(changedConversationId(bad), null);
});

test('a reload takes the stored messages, keeps the window settings and its waiting changes', () => {
  // The window shows an older copy: its question, and a notice it has not written yet.
  const existing = conversation([message('q1')], { workspaceId: 'w1', mode: 'code', title: 'New Chat' });
  // Meanwhile the agent session saved the answer, and another device asked a question.
  const stored = conversation(
    [message('q1'), message('a1', 'Sixty days.', 'assistant'), message('q2', 'From the phone')],
    { title: 'q1', updatedAt: '2026-11-13T10:05:00.000Z', workspaceId: undefined },
  );
  const waiting: MessagePatch = { id: 'c1', messages: [message('n1', 'Indexed lease.pdf', 'system')], removed: [] };
  const reloaded = reloadedConversation(existing, stored, waiting);
  assert.deepEqual(reloaded.messages.map(m => m.id), ['q1', 'a1', 'q2', 'n1'], 'no turn is lost, the window change stays');
  assert.equal(reloaded.messages[0], existing.messages[0], 'an unchanged message keeps its object, so it is not rendered again');
  assert.equal(reloaded.workspaceId, 'w1', 'a setting the window saves itself stays as the window has it');
  assert.equal(reloaded.mode, 'code');
  assert.equal(reloaded.title, 'q1', 'the name the first question gave it is shown');
  assert.equal(reloaded.updatedAt, '2026-11-13T10:05:00.000Z');

  // A name the user gave is never undone by a reload.
  const named = reloadedConversation({ ...existing, title: 'Lease' }, stored, undefined);
  assert.equal(named.title, 'Lease');
  // A conversation the window did not have (asked from the phone) is taken as stored.
  assert.equal(reloadedConversation(undefined, stored, undefined), stored);
});

test('patches write messages by id, never the whole list', () => {
  const list = [message('q1'), message('a1', 'old', 'assistant')];
  assert.deepEqual(applyPatch(list, { messages: [message('a1', 'cited [2]', 'assistant')], removed: [] }).map(m => m.content), ['q1', 'cited [2]']);
  assert.deepEqual(applyPatch(list, { messages: [message('n1')], removed: ['a1'] }).map(m => m.id), ['q1', 'n1']);

  const a: MessagePatch = { id: 'c1', messages: [message('n1', 'first'), message('n2')], removed: ['x'] };
  const b: MessagePatch = { id: 'c1', messages: [message('n1', 'second'), message('x')], removed: ['n2'] };
  const merged = mergePatches(a, b);
  assert.deepEqual(merged.messages.map(m => [m.id, m.content]), [['n1', 'second'], ['x', 'x']]);
  assert.deepEqual(merged.removed, ['n2'], 'a message written again is no longer removed');

  const shown = withPatch(conversation([]), { messages: [message('q1', 'What is the notice period in the lease?')], removed: [] }, '2026-11-13T11:00:00.000Z');
  assert.equal(shown.title, autoTitle('What is the notice period in the lease?'));
  assert.equal(shown.updatedAt, '2026-11-13T11:00:00.000Z');
});

test('message saves made within the delay go as one write', async () => {
  const writes: MessagePatch[] = [];
  const timers: Array<() => void> = [];
  const saves = new ConversationSaves<MessagePatch>({
    write: async patch => { writes.push(patch); },
    schedule: run => {
      timers.push(run);
      return () => {
        const at = timers.indexOf(run);
        if (at >= 0) timers.splice(at, 1);
      };
    },
    onError: () => assert.fail('no write fails'),
    delayMs: 500,
    merge: mergePatches,
  });
  saves.save({ id: 'c1', messages: [message('n1')], removed: [] });
  saves.save({ id: 'c1', messages: [message('n2')], removed: [] });
  assert.deepEqual(saves.waiting('c1')?.messages.map(m => m.id), ['n1', 'n2']);
  assert.equal(timers.length, 1);
  timers[0]();
  await saves.drain();
  assert.equal(writes.length, 1);
  assert.deepEqual(writes[0].messages.map(m => m.id), ['n1', 'n2']);
  assert.equal(saves.waiting('c1'), undefined);
});

test('the window reloads on conversation_changed and never writes whole message lists', () => {
  const hook = read('../src/hooks/useConversations.ts');
  assert.match(hook, /listen<unknown>\(CONVERSATION_CHANGED_EVENT/);
  assert.match(hook, /invoke<Conversation \| null>\('load_conversation', \{ conversationId: id \}\)/);
  // `save_conversation` carries a conversation's settings; messages go one by one.
  assert.match(hook, /invoke\('save_conversation_messages', \{/);
  assert.doesNotMatch(hook, /updateConversationMessages/);
  const session = read('../src/features/ask/ChatSessionContext.tsx');
  // The question and the answer go to the agent session with their turn (none while
  // saving is paused), which saves them only when the backend says so ...
  assert.match(session, /live\.savedByBackend = backendSavesTurn\(sendsTurn, null\);\s*live\.sending = true;\s*try \{\s*await api\.send\(/);
  assert.match(session, /live\.savedByBackend = backendSavesTurn\(sendsTurn, toAgentError\(error\)\);/);
  assert.match(session, /turn: sendsTurn\s*\?\s*\{\s*conversationId,\s*question: question \? toStored\(question\) : null,\s*answer: startedStored,/);
  assert.match(session, /const sendsTurn = !savingPaused\(\);/);
  // ... after the window wrote the question and the starting answer, which it starts
  // before the session and awaits before `agent_send`.
  const presaveAt = session.indexOf('const presaved = saveMessagesNow(conversationId, presave.messages, presave.removed);');
  assert.ok(presaveAt > 0, 'the question is written by the window first');
  assert.ok(presaveAt < session.indexOf('await api.start(conversationId'), 'before the session starts');
  assert.match(session, /await presaved;[\s\S]*?await api\.send\(/);
  // ... and are only shown by the window while they run.
  assert.match(session, /publish\(conversationId, userMessage, 'none'\)/);
  assert.match(session, /publish\(conversationId, message, 'none'\);\s*setLiveRun/);
  assert.match(session, /publish\(live\.conversationId, message, 'none'\);\s*setLiveRun/);
});

test('a question saved before its session starts comes back interrupted, with Retry, when the session never takes it', () => {
  // The window writes the question and the starting answer, then the app closes before
  // `agent_send`: the agent session never saw them.
  const question = questionMessage('q1', 'What is the notice period?', '2026-11-13T10:00:00.000Z', { spaceId: null, spaceName: null });
  const { message: started } = startedAnswer('a1', 'run-1', Date.parse('2026-11-13T10:00:00.000Z'));
  const presave = turnPresave(toStored(question), toStored(started), null);
  const saved = withPatch(conversation([]), presave, '2026-11-13T10:00:00.010Z');
  // Read back after the restart.
  const restored = (JSON.parse(JSON.stringify(saved)) as Conversation).messages.map(fromStored);
  assert.deepEqual(restored.map(m => [m.id, m.role]), [['q1', 'user'], ['a1', 'assistant']]);
  assert.equal(restored[0].content, 'What is the notice period?');
  assert.equal(restored[1].transcript?.status, 'aborted', 'the answer is interrupted');
  assert.equal(restored[1].transcript?.error, 'The app closed before this answer finished.', 'and offers Retry');
  assert.equal(saved.title, autoTitle('What is the notice period?'));

  // A retry of the latest answer asks no new question and removes the try it replaces.
  const { message: retried } = startedAnswer('a2', 'run-2', Date.parse('2026-11-13T10:01:00.000Z'), null, ['run-1']);
  const again = turnPresave(null, toStored(retried), 'a1');
  assert.deepEqual(again.removed, ['a1']);
  const afterRetry = withPatch(saved, again, '2026-11-13T10:01:00.010Z').messages.map(fromStored);
  assert.deepEqual(afterRetry.map(m => m.id), ['q1', 'a2']);
  assert.equal(afterRetry[1].transcript?.status, 'aborted');
  assert.deepEqual(afterRetry[1].transcript?.priorRunIds, ['run-1']);
});

test('a turn is saved by the agent session only when the backend says so', () => {
  // Sent with its turn: the session saves it unless it says a failure was not saved.
  assert.equal(backendSavesTurn(true, null), true, 'taken, and not failed');
  assert.equal(backendSavesTurn(true, { turnSaved: true }), true, 'failed after it was saved, and the failure saved');
  assert.equal(backendSavesTurn(true, {}), false, 'refused before it was saved, or its failure not saved: the window saves it');
  assert.equal(backendSavesTurn(true, { turnSaved: false }), false);
  // Saving is paused: no turn is sent, and nothing claims it is saved.
  assert.equal(backendSavesTurn(false, null), false);
  assert.equal(backendSavesTurn(false, { turnSaved: true }), false);
  // The error the window reads keeps what the backend said.
  const client = read('../src/features/agent/useAgentSession.ts');
  assert.match(client, /const saved = error\.turnSaved === true \? \{ turnSaved: true \} : \{\};/);
});

test('a question written before its session starts is awaited, and paused saving writes nothing', async () => {
  const writes: MessagePatch[] = [];
  let release: () => void = () => {};
  const saves = new ConversationSaves<MessagePatch>({
    write: patch => new Promise<void>(resolve => {
      writes.push(patch);
      release = resolve;
    }),
    schedule: () => () => {},
    onError: () => assert.fail('no write fails'),
    delayMs: 500,
    merge: mergePatches,
  });
  // A notice still waiting goes with it, in the same write.
  saves.save({ id: 'c1', messages: [message('n1')], removed: [] });
  let written = false;
  const now = saves.saveNow({ id: 'c1', messages: [message('q1'), message('a1', '', 'assistant')], removed: [] }).then(() => {
    written = true;
  });
  await Promise.resolve();
  assert.deepEqual(writes.map(w => w.messages.map(m => m.id)), [['n1', 'q1', 'a1']]);
  assert.equal(saves.waiting('c1'), undefined);
  assert.equal(written, false, 'not before the store has it');
  release();
  await now;
  assert.equal(written, true);

  saves.pause();
  await saves.saveNow({ id: 'c1', messages: [message('q2')], removed: [] });
  assert.equal(writes.length, 1, 'nothing is written while saving is paused');
});

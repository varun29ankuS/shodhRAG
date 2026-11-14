/**
 * The phone's outbox, sent when the computer is back: in order, one answer
 * at a time, never asking a question twice after the app was killed, and
 * stopping (not failing) when the computer goes away again. The queue's own
 * persistence across kills is tested in Rust (crates/shodh-link/src/outbox.rs,
 * the same test name); here the sender works on a queue in memory.
 */
import { describe, expect, it } from 'vitest';
import { createShim } from '../src/lib/invoke';
import { answerFinished, itemWords, OutboxSender, queuedQuestion, type QueuedItem } from '../src/lib/outbox';
import { Subscriptions } from '../src/lib/subscriptions';
import { MemoryOutbox } from './device';
import { FakeLink } from './fakeLink';

interface Message {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  timestamp: string;
  transcript?: unknown;
}

/** A computer that saves each question and answers it, one at a time per conversation, as the desktop does. */
class Desktop {
  conversations = new Map<string, Message[]>();
  sent: string[] = [];
  /** Answers not finished yet, by session. */
  private running = new Map<string, { conversationId: string; answerId: string; runId: string }>();
  /** When false, answers wait until `finishAll`. */
  answerAtOnce = true;
  /** Conversations the computer refuses to answer from a phone (Code). */
  refused = new Set<string>();
  link: FakeLink;
  private sessions = 0;

  constructor() {
    this.link = new FakeLink(async (command, args) => this.call(command, args));
  }

  private async call(command: string, args: Record<string, unknown>): Promise<unknown> {
    switch (command) {
      case 'agent_start':
        if (this.refused.has(args.conversationId as string)) {
          throw { code: 'refused', message: 'Code mode works on your computer only. Continue this conversation there.' };
        }
        return `session-${args.conversationId as string}-${++this.sessions}`;
      case 'load_conversation': {
        const messages = this.conversations.get(args.conversationId as string);
        return messages ? { id: args.conversationId, title: '', updatedAt: '', messages: messages.map(m => ({ ...m })) } : null;
      }
      case 'agent_send': {
        const turn = args.turn as { conversationId: string; question: Message; answer: Message };
        const messages = this.conversations.get(turn.conversationId) ?? [];
        if ([...this.running.values()].some(r => r.conversationId === turn.conversationId)) {
          throw { code: 'invalid_request', message: 'An answer is already running.' };
        }
        messages.push(turn.question, { ...turn.answer, transcript: { runId: args.requestId, status: 'running', blocks: [] } });
        this.conversations.set(turn.conversationId, messages);
        this.sent.push(args.text as string);
        const runId = args.requestId as string;
        this.running.set(args.sessionId as string, { conversationId: turn.conversationId, answerId: turn.answer.id, runId });
        if (this.answerAtOnce) queueMicrotask(() => this.finish(args.sessionId as string));
        return runId;
      }
      default:
        throw { code: 'runtime_error', message: `no script for ${command}` };
    }
  }

  /** The answer of `sessionId` is written and saved. */
  finish(sessionId: string) {
    const run = this.running.get(sessionId);
    if (!run) return;
    this.running.delete(sessionId);
    const messages = this.conversations.get(run.conversationId) ?? [];
    const answer = messages.find(m => m.id === run.answerId);
    if (answer) {
      answer.content = 'Answered [1].';
      answer.transcript = { runId: run.runId, status: 'completed', blocks: [{ kind: 'text', id: 'm', text: 'Answered [1].' }] };
    }
    this.link.send('agent_event', { sessionId, event: { type: 'text_delta', runId: run.runId, messageId: 'm', delta: 'Answered [1].' } });
    this.link.send('agent_event', { sessionId, event: { type: 'run_finished', runId: run.runId, status: 'completed', durationMs: 1, error: null } });
    this.link.send('conversation_changed', { id: run.conversationId });
  }

  finishAll() {
    for (const session of [...this.running.keys()]) this.finish(session);
  }
}

const counter = () => {
  let n = 0;
  return () => `id-${++n}`;
};

function sender(desktop: Desktop, outbox: MemoryOutbox) {
  const shim = createShim(desktop.link);
  return new OutboxSender(outbox, { desktop: shim, subscriptions: new Subscriptions(ids => shim.subscribe(ids)) });
}

describe('the outbox', () => {
  it('queued_questions_survive_a_kill_and_send_in_order', async () => {
    const desktop = new Desktop();
    const newId = counter();
    const outbox = new MemoryOutbox();
    const q1 = queuedQuestion({ conversationId: 'c1', text: 'First', workspaceId: null }, newId, () => 1);
    const q2 = queuedQuestion({ conversationId: 'c1', text: 'Second', workspaceId: null }, newId, () => 2);
    const q3 = queuedQuestion({ conversationId: 'c2', text: 'Third', workspaceId: 'w1' }, newId, () => 3);
    await outbox.add('i1', q1);
    await outbox.add('i2', q2);
    await outbox.add('i3', q3);

    // The app was killed while the first was being sent, after the computer took it.
    await outbox.mark('i1', 'sending');
    desktop.conversations.set('c1', [
      { id: q1.questionId, role: 'user', content: 'First', timestamp: '' },
      { id: q1.answerId, role: 'assistant', content: 'Answered [1].', timestamp: '', transcript: { runId: q1.requestId, status: 'completed', blocks: [{ kind: 'text', id: 'm', text: 'Answered [1].' }] } },
    ]);

    // The app starts again (a new sender on the same queue) and the computer is back.
    await sender(desktop, outbox).run(new AbortController().signal);

    expect(desktop.sent, 'the first is not asked twice; the rest go in order').toEqual(['Second', 'Third']);
    expect(outbox.items.map(i => i.state)).toEqual(['answered', 'answered', 'answered']);
    expect(outbox.history.filter(([id]) => id !== 'i1')).toEqual([
      ['i2', 'waiting'],
      ['i3', 'waiting'],
      ['i2', 'sending'],
      ['i2', 'sent'],
      ['i2', 'answered'],
      ['i3', 'sending'],
      ['i3', 'sent'],
      ['i3', 'answered'],
    ]);
    // Each was asked with the ids it was queued with, at the time it was asked.
    const second = desktop.link.calls.find(c => c.command === 'agent_send' && c.args.text === 'Second');
    expect(second?.args).toMatchObject({
      requestId: q2.requestId,
      textOrigin: 'typed',
      turn: { conversationId: 'c1', question: { id: q2.questionId, timestamp: new Date(2).toISOString() }, answer: { id: q2.answerId } },
    });
    // The follow-up carries the conversation so far, for a session started afresh.
    expect(second?.args.history).toEqual([
      { role: 'user', content: 'First' },
      { role: 'assistant', content: 'Answered [1].' },
    ]);
    const third = desktop.link.calls.find(c => c.command === 'agent_start' && c.args.conversationId === 'c2');
    expect(third?.args).toEqual({ conversationId: 'c2', workspaceId: 'w1' });
  });

  it('waits for each answer before the next question, and stops when the computer goes away', async () => {
    const desktop = new Desktop();
    desktop.answerAtOnce = false;
    const outbox = new MemoryOutbox();
    await outbox.add('i1', queuedQuestion({ conversationId: 'c1', text: 'One', workspaceId: null }, counter(), () => 1));
    await outbox.add('i2', queuedQuestion({ conversationId: 'c1', text: 'Two', workspaceId: null }, counter(), () => 2));
    const connection = new AbortController();
    const running = sender(desktop, outbox).run(connection.signal);
    await until(outbox, () => outbox.items[0].state === 'sent');
    expect(desktop.sent, 'the second waits for the first answer').toEqual(['One']);
    // The computer goes to sleep mid-answer: the sender stops, nothing fails.
    connection.abort();
    await running;
    expect(outbox.items.map(i => i.state)).toEqual(['sent', 'waiting']);

    // Back again (after a restart): the answer finished meanwhile; the second goes.
    desktop.finishAll();
    desktop.answerAtOnce = true;
    await sender(desktop, outbox).run(new AbortController().signal);
    expect(desktop.sent).toEqual(['One', 'Two']);
    expect(outbox.items.map(i => i.state)).toEqual(['answered', 'answered']);
  });

  it('a refused question fails with the reason and does not hold up the rest', async () => {
    const desktop = new Desktop();
    desktop.refused.add('code');
    const outbox = new MemoryOutbox();
    await outbox.add('i1', queuedQuestion({ conversationId: 'code', text: 'Change it', workspaceId: null }));
    await outbox.add('i2', queuedQuestion({ conversationId: 'c1', text: 'Fine', workspaceId: null }));
    await sender(desktop, outbox).run(new AbortController().signal);
    expect(outbox.items[0].state).toBe('failed');
    expect(itemWords(outbox.items[0] as QueuedItem).state).toBe('Not sent: Code mode works on your computer only. Continue this conversation there.');
    expect(outbox.items[1].state).toBe('answered');
  });

  it('an answer counts as finished only once the computer saved its end', () => {
    const conversation = (transcript: unknown, content = '') => ({
      id: 'c',
      title: '',
      updatedAt: '',
      messages: [{ id: 'a', role: 'assistant' as const, content, timestamp: '', transcript }],
    });
    expect(answerFinished(conversation({ status: 'running', blocks: [] }), 'a')).toBe(false);
    expect(answerFinished(conversation({ status: 'completed', blocks: [{ kind: 'text', id: 'm', text: 'Yes.' }] }), 'a')).toBe(true);
    expect(answerFinished(conversation({ status: 'error', error: 'It stopped.', blocks: [] }), 'a')).toBe(true);
    expect(answerFinished(conversation({ status: 'completed', blocks: [] }), 'other')).toBe(false);
    expect(answerFinished(null, 'a')).toBe(false);
  });
});

/** Resolves on the queue change after which `ready` holds (no polling: the queue says when it changed). */
function until(outbox: MemoryOutbox, ready: () => boolean): Promise<void> {
  return new Promise(resolve => {
    if (ready()) return resolve();
    let stop: (() => void) | null = null;
    void outbox.onChanged(() => {
      if (!ready()) return;
      stop?.();
      resolve();
    }).then(unlisten => {
      stop = unlisten;
    });
  });
}

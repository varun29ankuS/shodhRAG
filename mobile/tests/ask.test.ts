import { describe, expect, it } from 'vitest';
import { askOnce, AskStopped } from '../src/lib/ask';
import { createShim } from '../src/lib/invoke';
import { FakeLink } from './fakeLink';

const ids = () => {
  let n = 0;
  return () => `id-${++n}`;
};

const event = (sessionId: string, event: Record<string, unknown>) => ({ sessionId, event });

describe('the test Ask', () => {
  it('streams one answer and asks as the desktop window does', async () => {
    let link!: FakeLink;
    link = new FakeLink(async command => {
      if (command === 'agent_start') return 'session-1';
      if (command === 'agent_send') {
        // Events can arrive before agent_send returns the run's id.
        link.send('agent_event', event('session-1', { type: 'text_delta', runId: 'run-1', messageId: 'm0', delta: "I'll search the papers." }));
        link.send('agent_event', event('session-1', { type: 'text_interim', runId: 'run-1', messageId: 'm0', interim: true }));
        link.send('agent_event', event('session-2', { type: 'text_delta', runId: 'run-9', messageId: 'x', delta: 'another answer' }));
        queueMicrotask(() => {
          link.send('agent_event', event('session-1', { type: 'text_delta', runId: 'run-1', messageId: 'm1', delta: 'Self-RAG retrieves ' }));
          link.send('agent_event', event('session-1', { type: 'text_delta', runId: 'run-1', messageId: 'm1', delta: 'on demand [1].' }));
          link.send('agent_event', event('session-1', { type: 'run_finished', runId: 'run-1', status: 'completed', durationMs: 5, error: null }));
        });
        return 'run-1';
      }
      throw new Error(`unexpected ${command}`);
    });
    const seen: string[] = [];
    const answer = await askOnce(createShim(link), 'What does Self-RAG retrieve?', {
      onText: text => seen.push(text),
      newId: ids(),
      now: () => Date.UTC(2026, 10, 13, 10),
    });

    expect(answer).toEqual({ text: 'Self-RAG retrieves on demand [1].', status: 'completed', error: null });
    expect(seen.at(-1)).toBe('Self-RAG retrieves on demand [1].');
    expect(seen).not.toContain('another answer');
    expect(link.subscriptions).toEqual([['session-1']]);
    expect(link.calls.map(c => c.command)).toEqual(['agent_start', 'agent_send']);
    expect(link.calls[0].args).toEqual({ conversationId: 'id-1' });
    expect(link.calls[1].args).toEqual({
      sessionId: 'session-1',
      text: 'What does Self-RAG retrieve?',
      requestId: 'id-2',
      turn: {
        conversationId: 'id-1',
        question: { id: 'id-3', role: 'user', content: 'What does Self-RAG retrieve?', timestamp: '2026-11-13T10:00:00.000Z' },
        answer: {
          id: 'id-4',
          role: 'assistant',
          content: '',
          timestamp: '2026-11-13T10:00:00.000Z',
          transcript: { startedAtMs: Date.UTC(2026, 10, 13, 10) },
        },
        replaces: null,
      },
    });
  });

  it('a stopped question stops waiting and stops listening', async () => {
    const link = new FakeLink(async command => (command === 'agent_start' ? 'session-1' : 'run-1'));
    const controller = new AbortController();
    const seen: string[] = [];
    const asked = askOnce(createShim(link), 'Anything?', { onText: text => seen.push(text), signal: controller.signal, newId: ids() });
    await new Promise(resolve => setTimeout(resolve, 0));
    controller.abort();
    await expect(asked).rejects.toBeInstanceOf(AskStopped);
    link.send('agent_event', event('session-1', { type: 'text_delta', runId: 'run-1', messageId: 'm1', delta: 'late' }));
    expect(seen).toEqual([]);
  });
});

import { describe, expect, it } from 'vitest';
import { createShim, type LinkEvent } from '../src/lib/invoke';
import { FakeLink } from './fakeLink';

describe('the invoke shim', () => {
  it('invoke_shim_forwards_and_routes_events', async () => {
    const link = new FakeLink(async (command, args) => {
      if (command === 'agent_start') return 'session-1';
      if (command === 'agent_approve') throw { code: 'refused', message: 'Approve this on your computer.' };
      throw new Error(`unexpected ${command} ${JSON.stringify(args)}`);
    });
    const shim = createShim(link);

    // A call is forwarded with its arguments as the desktop window sends them,
    // and its value comes back.
    await expect(shim.invoke<string>('agent_start', { conversationId: 'conv-1' })).resolves.toBe('session-1');
    expect(link.calls).toEqual([{ command: 'agent_start', args: { conversationId: 'conv-1' } }]);
    // A call without arguments sends an empty object, as Tauri's invoke does.
    await expect(shim.invoke('agent_approve')).rejects.toEqual({ code: 'refused', message: 'Approve this on your computer.' });
    expect(link.calls[1]).toEqual({ command: 'agent_approve', args: {} });

    // Events reach the listeners of their name only, as Tauri events.
    const agent: LinkEvent<unknown>[] = [];
    const changed: LinkEvent<unknown>[] = [];
    const stopAgent = await shim.listen('agent_event', e => agent.push(e));
    await shim.listen('conversation_changed', e => changed.push(e));
    expect(link.attachments).toBe(1);
    link.send('agent_event', { sessionId: 'session-1', event: { type: 'text_delta', delta: 'Self-RAG ' } });
    link.send('conversation_changed', { id: 'conv-1' });
    link.send('phone_devices_changed', {});
    expect(agent.map(e => [e.event, e.payload])).toEqual([
      ['agent_event', { sessionId: 'session-1', event: { type: 'text_delta', delta: 'Self-RAG ' } }],
    ]);
    expect(changed.map(e => e.payload)).toEqual([{ id: 'conv-1' }]);
    expect(agent[0].id).not.toBe(changed[0].id);

    // A listener that stops receives nothing more; the others still do.
    stopAgent();
    link.send('agent_event', { sessionId: 'session-1', event: { type: 'run_finished' } });
    link.send('conversation_changed', { id: 'conv-2' });
    expect(agent).toHaveLength(1);
    expect(changed).toHaveLength(2);

    // Subscriptions go to the link.
    await shim.subscribe(['session-1']);
    expect(link.subscriptions).toEqual([['session-1']]);
  });

  it('a listener added while an event is delivered does not get that event', async () => {
    const link = new FakeLink(async () => null);
    const routed = createShim(link);
    const seen: string[] = [];
    await routed.listen('agent_event', () => {
      seen.push('first');
      void routed.listen('agent_event', () => seen.push('second'));
    });
    link.send('agent_event', {});
    await Promise.resolve();
    expect(seen).toEqual(['first']);
    link.send('agent_event', {});
    expect(seen).toEqual(['first', 'first', 'second']);
  });

  it('a link that cannot listen fails the listen, and the next one tries again', async () => {
    const link = new FakeLink(async () => null);
    let failures = 1;
    const original = link.onEvent.bind(link);
    link.onEvent = async handler => {
      if (failures-- > 0) throw new Error('not connected');
      return original(handler);
    };
    const shim = createShim(link);
    await expect(shim.listen('agent_event', () => {})).rejects.toThrow('not connected');
    const seen: unknown[] = [];
    await shim.listen('agent_event', e => seen.push(e.payload));
    link.send('agent_event', 1);
    expect(seen).toEqual([1]);
  });
});

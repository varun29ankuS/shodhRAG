/**
 * One question to the computer and its streamed answer: the Connected
 * screen's test Ask, which proves the whole path (the link, the computer's
 * agent, its events back). It asks as the desktop's window does: start the
 * conversation's session, then send the question with its turn, so the
 * computer saves the question and the answer whatever happens to the phone.
 */
import type { Shim } from './invoke';

/** `agent_event`'s payload: an event of one session. */
interface AgentEventPayload {
  sessionId: string;
  event: { type: string; runId?: string; messageId?: string; delta?: string; interim?: boolean; status?: string; error?: string | null };
}

export interface AskOptions {
  /** The answer so far, each time it grows. */
  onText: (text: string) => void;
  /** Stops waiting (the screen went away, or the connection ended). */
  signal?: AbortSignal;
  newId?: () => string;
  now?: () => number;
}

export interface Answer {
  text: string;
  status: 'completed' | 'aborted' | 'error';
  error: string | null;
}

export class AskStopped extends Error {}

/** Ask `question` in a new conversation and wait for the whole answer. */
export async function askOnce(shim: Shim, question: string, options: AskOptions): Promise<Answer> {
  const newId = options.newId ?? (() => crypto.randomUUID());
  const now = options.now ?? Date.now;
  const conversationId = newId();
  const sessionId = await shim.invoke<string>('agent_start', { conversationId });
  await shim.subscribe([sessionId]);

  // The answer's text by message, in order; a message the model marks as
  // its working note ("I'll search…") is not the answer.
  const order: string[] = [];
  const parts = new Map<string, string>();
  const interim = new Set<string>();
  const text = () => order.filter(id => !interim.has(id)).map(id => parts.get(id) ?? '').join('');

  // Events can arrive before agent_send returns the run's id: they wait here.
  let runId: string | null = null;
  const early: AgentEventPayload['event'][] = [];
  let finish: (answer: Answer) => void = () => {};
  let fail: (error: unknown) => void = () => {};
  const finished = new Promise<Answer>((resolve, reject) => {
    finish = resolve;
    fail = reject;
  });
  // Settled only through `await finished`; a stop before then is not unhandled.
  finished.catch(() => undefined);

  const apply = (event: AgentEventPayload['event']) => {
    if (event.runId !== runId) return;
    if (event.type === 'text_delta' && event.messageId && typeof event.delta === 'string') {
      if (!parts.has(event.messageId)) order.push(event.messageId);
      parts.set(event.messageId, (parts.get(event.messageId) ?? '') + event.delta);
      options.onText(text());
    } else if (event.type === 'text_interim' && event.messageId) {
      if (event.interim) interim.add(event.messageId);
      else interim.delete(event.messageId);
      options.onText(text());
    } else if (event.type === 'run_finished') {
      const status = event.status === 'completed' || event.status === 'aborted' ? event.status : 'error';
      finish({ text: text(), status, error: event.error ?? null });
    }
  };

  const unlisten = await shim.listen<AgentEventPayload>('agent_event', ({ payload }) => {
    if (payload.sessionId !== sessionId) return;
    if (runId === null) early.push(payload.event);
    else apply(payload.event);
  });
  const stop = () => fail(new AskStopped('The question was stopped.'));
  options.signal?.addEventListener('abort', stop, { once: true });

  try {
    if (options.signal?.aborted) throw new AskStopped('The question was stopped.');
    const requestId = newId();
    const timestamp = new Date(now()).toISOString();
    runId = await shim.invoke<string>('agent_send', {
      sessionId,
      text: question,
      requestId,
      turn: {
        conversationId,
        question: { id: newId(), role: 'user', content: question, timestamp },
        // The computer fills in the rest of the starting transcript.
        answer: { id: newId(), role: 'assistant', content: '', timestamp, transcript: { startedAtMs: now() } },
        replaces: null,
      },
    });
    for (const event of early.splice(0)) apply(event);
    return await finished;
  } finally {
    options.signal?.removeEventListener('abort', stop);
    unlisten();
  }
}

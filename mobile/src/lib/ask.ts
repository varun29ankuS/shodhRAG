/**
 * A question to the computer and its streamed answer. It asks as the
 * desktop's window does: start (or reuse) the conversation's session, then
 * send the question with its turn, so the computer saves the question and
 * the answer whatever happens to the phone (it may go to sleep mid-answer;
 * the answer is in the conversation when it comes back).
 */
import { foldAnswer, startingAnswer, type AnswerState } from './answer';
import type { HistoryTurn } from './desktop';
import type { Shim } from './invoke';
import type { Subscriptions } from './subscriptions';

/** `agent_event`'s payload: an event of one session. */
interface AgentEventPayload {
  sessionId: string;
  event: unknown;
}

export interface Question {
  conversationId: string;
  text: string;
  /** The conversation so far, for a session the computer started afresh. */
  history: HistoryTurn[];
  workspaceId: string | null;
}

export interface AskOptions {
  /** The answer so far, each time it changes. */
  onAnswer: (answer: AnswerState) => void;
  /** The session, once started (for Stop and approvals). */
  onSession?: (sessionId: string) => void;
  /** Stops waiting (the screen went away, or the connection ended). */
  signal?: AbortSignal;
  newId?: () => string;
  now?: () => number;
}

export class AskStopped extends Error {}

/** The subscription owner of the answer being read. */
export const ASK_OWNER = 'ask';

/** Ask `question` and wait for the whole answer. The answer's session stays subscribed. */
export async function ask(shim: Shim, subscriptions: Subscriptions, question: Question, options: AskOptions): Promise<AnswerState> {
  const newId = options.newId ?? (() => crypto.randomUUID());
  const now = options.now ?? Date.now;
  const stopped = () => new AskStopped('The question was stopped.');
  if (options.signal?.aborted) throw stopped();

  const sessionId = await shim.invoke<string>('agent_start', {
    conversationId: question.conversationId,
    ...(question.workspaceId ? { workspaceId: question.workspaceId } : {}),
  });
  options.onSession?.(sessionId);
  await subscriptions.set(ASK_OWNER, [sessionId]);

  let answer = startingAnswer();
  // Events can arrive before agent_send returns the run's id: they wait here.
  let runId: string | null = null;
  const early: unknown[] = [];
  let finish: (answer: AnswerState) => void = () => {};
  let fail: (error: unknown) => void = () => {};
  const finished = new Promise<AnswerState>((resolve, reject) => {
    finish = resolve;
    fail = reject;
  });
  // Settled only through `await finished`; a stop before then is not unhandled.
  finished.catch(() => undefined);

  const apply = (event: unknown) => {
    const next = foldAnswer(answer, event);
    if (next === answer) return;
    answer = next;
    options.onAnswer(answer);
    if (answer.status !== 'starting' && answer.status !== 'running') finish(answer);
  };

  const unlisten = await shim.listen<AgentEventPayload>('agent_event', ({ payload }) => {
    if (payload.sessionId !== sessionId) return;
    if (runId === null) early.push(payload.event);
    else apply(payload.event);
  });
  const stop = () => fail(stopped());
  options.signal?.addEventListener('abort', stop, { once: true });

  try {
    if (options.signal?.aborted) throw stopped();
    const timestamp = new Date(now()).toISOString();
    runId = await shim.invoke<string>('agent_send', {
      sessionId,
      text: question.text,
      requestId: newId(),
      history: question.history,
      turn: {
        conversationId: question.conversationId,
        question: { id: newId(), role: 'user', content: question.text, timestamp },
        // The computer fills in the rest of the starting transcript.
        answer: { id: newId(), role: 'assistant', content: '', timestamp, transcript: { startedAtMs: now() } },
        replaces: null,
      },
    });
    answer = { ...answer, runId };
    options.onAnswer(answer);
    for (const event of early.splice(0)) apply(event);
    return await finished;
  } finally {
    options.signal?.removeEventListener('abort', stop);
    unlisten();
  }
}

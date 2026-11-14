/**
 * A question to the computer and its streamed answer. It asks as the
 * desktop's window does: start (or reuse) the conversation's session, then
 * send the question with its turn, so the computer saves the question and
 * the answer whatever happens to the phone (it may go to sleep mid-answer;
 * the answer is in the conversation when it comes back).
 *
 * What the user typed is sent as their own words (`textOrigin: typed`), as
 * the desktop's composer sends it: a link in it is one they gave, which the
 * assistant may open. Text that is not theirs (read from a photo, shared from
 * another app) goes as attached context, which the computer fences and never
 * learns from.
 */
import { answerText, foldAnswer, savedAnswer, startingAnswer, type AnswerState } from './answer';
import type { HistoryTurn, SavedConversation } from './desktop';
import type { Shim } from './invoke';
import type { Subscriptions } from './subscriptions';

/** `agent_event`'s payload: an event of one session. */
interface AgentEventPayload {
  sessionId: string;
  event: unknown;
}

/**
 * Text the question is about, as the computer's `attachedContext` takes it:
 * read from a photo the user took (`photo`), or shared from another app
 * (`shared`).
 */
export interface Attached {
  origin: 'photo' | 'shared';
  text: string;
}

/** A passage the answer may cite under its own number (a photo, as "Your photo"). */
export interface CarriedPassage {
  n: number;
  file: string;
  path: string;
  page?: string | null;
  text: string;
  web?: boolean;
}

export interface Question {
  conversationId: string;
  text: string;
  /** The conversation so far, for a session the computer started afresh. */
  history: HistoryTurn[];
  workspaceId: string | null;
  attached?: Attached[];
  passages?: CarriedPassage[];
}

/** The ids a question is sent with: the same on every try, so a resend is the same request. */
export interface QuestionIds {
  requestId: string;
  questionId: string;
  answerId: string;
}

export interface AskOptions {
  /** The answer so far, each time it changes. */
  onAnswer: (answer: AnswerState) => void;
  /** The session, once started (for Stop and approvals). */
  onSession?: (sessionId: string) => void;
  /** The computer took the question (it is saved there from now on). */
  onSent?: (runId: string) => void;
  /** Stops waiting (the screen went away, or the connection ended). */
  signal?: AbortSignal;
  /** The ids to send with; new ones when absent. */
  ids?: QuestionIds;
  /** When the question was asked (its time in the conversation); now when absent. */
  askedAt?: number;
  /** Whose subscription the answer's session is (the Ask screen's, or the outbox's). */
  owner?: string;
  newId?: () => string;
  now?: () => number;
}

export class AskStopped extends Error {}

/** The subscription owner of the answer being read. */
export const ASK_OWNER = 'ask';

/** The conversation so far, as a session started afresh replays it. */
export function historyOf(conversation: SavedConversation | null | undefined): HistoryTurn[] {
  return (conversation?.messages ?? [])
    .filter((m): m is SavedConversation['messages'][number] & { role: 'user' | 'assistant' } => m.role === 'user' || m.role === 'assistant')
    .map(m => ({ role: m.role, content: m.role === 'assistant' ? answerText(savedAnswer(m.content, m.transcript)) : m.content }))
    .filter(t => t.content.trim() !== '');
}

/** Ask `question` and wait for the whole answer. The answer's session stays subscribed. */
export async function ask(shim: Shim, subscriptions: Subscriptions, question: Question, options: AskOptions): Promise<AnswerState> {
  const newId = options.newId ?? (() => crypto.randomUUID());
  const now = options.now ?? Date.now;
  const stopped = () => new AskStopped('The question was stopped.');
  if (options.signal?.aborted) throw stopped();
  const ids: QuestionIds = options.ids ?? { requestId: newId(), questionId: newId(), answerId: newId() };

  const sessionId = await shim.invoke<string>('agent_start', {
    conversationId: question.conversationId,
    ...(question.workspaceId ? { workspaceId: question.workspaceId } : {}),
  });
  options.onSession?.(sessionId);
  await subscriptions.set(options.owner ?? ASK_OWNER, [sessionId]);

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
    const askedAt = options.askedAt ?? now();
    const timestamp = new Date(askedAt).toISOString();
    runId = await shim.invoke<string>('agent_send', {
      sessionId,
      text: question.text,
      requestId: ids.requestId,
      history: question.history,
      textOrigin: 'typed',
      ...(question.attached && question.attached.length > 0 ? { attachedContext: question.attached } : {}),
      ...(question.passages && question.passages.length > 0 ? { scope: { passages: question.passages, ...(question.workspaceId ? { workspaceId: question.workspaceId } : {}) } } : {}),
      turn: {
        conversationId: question.conversationId,
        question: { id: ids.questionId, role: 'user', content: question.text, timestamp },
        // The computer fills in the rest of the starting transcript.
        answer: { id: ids.answerId, role: 'assistant', content: '', timestamp, transcript: { startedAtMs: askedAt } },
        replaces: null,
      },
    });
    answer = { ...answer, runId };
    options.onSent?.(runId);
    options.onAnswer(answer);
    for (const event of early.splice(0)) apply(event);
    return await finished;
  } finally {
    options.signal?.removeEventListener('abort', stop);
    unlisten();
  }
}

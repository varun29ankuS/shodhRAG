/**
 * What a stored message of the main conversation says when the conversation is replayed into a
 * fresh agent session. Pure module, unit-tested with Node (`app/tests/focusDrill.test.ts`).
 */

import { answerAsContext } from '../agent/grounding.ts';
import { summaryPrompt } from '../focus/summary.ts';
import type { SideSummaryRef } from '../focus/summary.ts';
import { composeQuestion } from './imageAttachments.ts';
import type { AttachedContext } from './imageAttachments.ts';

/** The parts of a chat message the replay reads. */
export interface ReplayedMessage {
  role: 'user' | 'assistant';
  content: string;
  /** An answer's agent transcript: its grounding report decides which citations travel. */
  transcript?: unknown;
  sideSummary?: SideSummaryRef;
  /** Text the user attached to a question (read from images): replayed after it, as sent. */
  attachedContext?: AttachedContext[];
}

/**
 * A side-discussion summary reads as one; an answer goes without the citation markers its
 * grounding check flagged (`answerAsContext`), so the next answer does not repeat them.
 */
export function replayedContent(message: ReplayedMessage): string {
  if (message.sideSummary) return summaryPrompt(message.sideSummary, message.content);
  if (message.role === 'assistant') return answerAsContext(message.content, message.transcript);
  return message.attachedContext && message.attachedContext.length > 0
    ? composeQuestion(message.content, message.attachedContext)
    : message.content;
}

/** The parts of a message `withContinuations` reads. */
interface ContinuedMessage {
  role: string;
  content: string;
  transcript?: { runId?: string; continuesRunId?: string } | null;
}

/**
 * Messages as the conversation reads them: an answer Shodh closed during and the answer
 * that continued it (Continue) are one answer, its text then the rest, in the continued
 * answer's place. The stopped answer stays on the screen; only what is replayed joins.
 */
export function withContinuations<M extends ContinuedMessage>(messages: readonly M[]): M[] {
  const out: M[] = [];
  for (const message of messages) {
    const continues = message.role === 'assistant' ? message.transcript?.continuesRunId : undefined;
    const previous = out[out.length - 1];
    if (continues && previous && previous.role === 'assistant' && previous.transcript?.runId === continues) {
      const before = previous.content.trimEnd();
      const rest = message.content.trimStart();
      out[out.length - 1] = { ...message, content: before && rest ? `${before} ${rest}` : before || rest };
      continue;
    }
    out.push(message);
  }
  return out;
}

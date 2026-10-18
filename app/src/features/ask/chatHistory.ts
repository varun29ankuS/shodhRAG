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

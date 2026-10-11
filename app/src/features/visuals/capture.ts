/**
 * What the gallery records from answers: the capture request of one
 * finished answer (main or side), and the one-time backfill of answers
 * saved before the gallery existed.
 *
 * Pure module (type-only imports apart from thread readers), unit-tested
 * with Node (`app/tests/visualGallery.test.ts`).
 */

import type { Conversation } from '../../hooks/useConversations';
import { readThreads, threadsFromMetadata } from '../focus/threadStore.ts';
import { capturable, extractVisualBlocks } from './extract.ts';
import type { ExtractedVisual } from './extract.ts';
import type { VisualOrigin } from './model.ts';

/** A block as `visuals_capture` takes it. */
export interface CaptureBlock {
  kind: ExtractedVisual['kind'];
  title: string;
  source: string;
}

export interface CaptureBatch {
  origin: VisualOrigin;
  blocks: CaptureBlock[];
}

/** The blocks of one answer worth recording, or null when it has none. */
export function captureBatch(origin: VisualOrigin, answer: string): CaptureBatch | null {
  const blocks = capturable(extractVisualBlocks(answer));
  return blocks.length > 0 ? { origin, blocks } : null;
}

/** Answers carried by one backfill call (the backend accepts up to 2,000). */
export const BACKFILL_CHUNK = 500;

/**
 * Capture batches for every saved answer and side answer of `conversations`.
 * Recording them again is harmless (the backend deduplicates per answer).
 */
export function backfillBatches(conversations: readonly Pick<Conversation, 'id' | 'messages' | 'focusThreads'>[]): CaptureBatch[] {
  const out: CaptureBatch[] = [];
  const push = (origin: VisualOrigin, text: string) => {
    const batch = captureBatch(origin, text);
    if (batch) out.push(batch);
  };
  for (const conversation of conversations) {
    for (const message of conversation.messages) {
      if (message.role === 'assistant' && message.content) {
        push({ conversationId: conversation.id, messageId: message.id, threadId: null, turnId: null }, message.content);
      }
      for (const thread of threadsFromMetadata(message.metadata)) {
        for (const turn of thread.turns) {
          if (turn.role !== 'assistant' || !turn.content) continue;
          push({ conversationId: conversation.id, messageId: message.id, threadId: thread.id, turnId: turn.id }, turn.content);
        }
      }
    }
    for (const thread of readThreads(conversation.focusThreads)) {
      for (const turn of thread.turns) {
        if (turn.role !== 'assistant' || !turn.content) continue;
        push({ conversationId: conversation.id, messageId: null, threadId: thread.id, turnId: turn.id }, turn.content);
      }
    }
  }
  return out;
}

/** `items` in chunks of `size`. */
export function chunks<T>(items: readonly T[], size = BACKFILL_CHUNK): T[][] {
  const out: T[][] = [];
  for (let i = 0; i < items.length; i += size) out.push(items.slice(i, i + size));
  return out;
}

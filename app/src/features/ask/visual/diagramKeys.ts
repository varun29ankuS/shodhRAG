/**
 * The diagrams a conversation holds, as the diagram ledger keys them
 * (`sourceKey` of the source the answer draws): in its answers, and in the
 * answers of its side threads (kept on its messages and on itself). Used
 * to forget a deleted conversation's diagram versions, notes and lists.
 *
 * Pure module, unit-tested with Node (`app/tests/diagramKeys.test.ts`).
 */

import { extractVisualBlocks } from '../../visuals/extract.ts';
import { readThreads, threadsFromMetadata } from '../../focus/threadStore.ts';
import { sourceKey } from './mermaidRetry.ts';

/** What of a conversation holds diagrams; `Conversation` satisfies it. */
export interface DiagramHolder {
  messages: readonly { content: string; metadata?: unknown }[];
  focusThreads?: unknown;
}

/** The ledger keys of the diagrams in `markdown`. */
export function diagramKeysIn(markdown: string): string[] {
  return extractVisualBlocks(markdown)
    .filter(block => block.kind === 'mermaid')
    .map(block => sourceKey(block.source));
}

/** The ledger keys of every diagram a conversation holds. */
export function conversationDiagramKeys(conversation: DiagramHolder): Set<string> {
  const keys = new Set<string>();
  const add = (text: string) => {
    if (!text.includes('```') && !text.includes('~~~')) return;
    for (const key of diagramKeysIn(text)) keys.add(key);
  };
  const threads = [...readThreads(conversation.focusThreads)];
  for (const message of conversation.messages) {
    add(message.content);
    threads.push(...threadsFromMetadata(message.metadata));
  }
  for (const thread of threads) for (const turn of thread.turns) add(turn.content);
  return keys;
}

/**
 * The keys to forget when `deleted` goes: its diagrams, except those the
 * same source also draws in a conversation that stays (their versions are
 * still shown there).
 */
export function diagramKeysToForget(deleted: DiagramHolder, remaining: readonly DiagramHolder[]): string[] {
  const gone = conversationDiagramKeys(deleted);
  for (const other of remaining) {
    if (gone.size === 0) break;
    for (const key of conversationDiagramKeys(other)) gone.delete(key);
  }
  return [...gone];
}

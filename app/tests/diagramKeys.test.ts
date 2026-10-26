/**
 * Deleting a conversation forgets its diagrams' versions, notes and lists:
 * the keys of every diagram it holds (answers and side-thread answers),
 * except those another conversation still draws.
 *   node --experimental-strip-types --test app/tests/diagramKeys.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { conversationDiagramKeys, diagramKeysIn, diagramKeysToForget } from '../src/features/ask/visual/diagramKeys.ts';
import { diagramFenceSource } from '../src/features/ask/visual/mathText.ts';
import { EMPTY_RECORD, LEDGER_STORAGE_KEY, createRepairLedger, sourceKey } from '../src/features/ask/visual/mermaidRetry.ts';
import type { LedgerStorage } from '../src/features/ask/visual/mermaidRetry.ts';

const PRIVATE = 'flowchart TD\n  A[Salary review] --> B[Offer]';
const SHARED = 'flowchart LR\n  E[Encoder] --> D[Decoder]';
const IN_THREAD = 'sequenceDiagram\n  Alice->>Bob: hello';

function fence(source: string, lang = 'mermaid'): string {
  return `\`\`\`${lang}\n${source}\n\`\`\``;
}

/** The key the answer's diagram block uses for a fence (as MessageContentRenderer and MermaidBlock compute it). */
function rendererKey(lang: string, body: string): string {
  return sourceKey(diagramFenceSource(lang, body).trim());
}

function thread(id: string, content: string) {
  return {
    id,
    anchor: { conversationId: 'c1', parentMessageId: 'm2', target: { kind: 'mermaid', label: 'Diagram', source: PRIVATE } },
    turns: [
      { id: `${id}-q`, role: 'user', content: 'Explain it', timestamp: '2026-10-27T10:00:00Z' },
      { id: `${id}-a`, role: 'assistant', content, timestamp: '2026-10-27T10:00:05Z' },
    ],
    createdAt: '2026-10-27T10:00:00Z',
    updatedAt: '2026-10-27T10:00:05Z',
  };
}

const deleted = {
  messages: [
    { content: 'Draw it' },
    {
      content: `Here:\n\n${fence(PRIVATE)}\n\nand\n\n${fence(SHARED)}`,
      metadata: { focusThreads: [thread('t1', `In order:\n\n${fence(IN_THREAD)}`)] },
    },
  ],
  focusThreads: [thread('t2', fence('A --> B   ', 'flowchart'))],
};
const remaining = [{ messages: [{ content: fence(SHARED) }] }];

test('diagramKeysIn: the keys the answer uses, also for a headerless fence with trailing spaces', () => {
  assert.deepEqual(diagramKeysIn(fence(PRIVATE)), [rendererKey('mermaid', PRIVATE)]);
  assert.deepEqual(diagramKeysIn(fence('  A --> B  \n  B --> C ', 'flowchart')), [rendererKey('flowchart', '  A --> B  \n  B --> C ')]);
  assert.deepEqual(diagramKeysIn('No diagrams. ```js\nx\n```'), []);
});

test('conversationDiagramKeys: answers, side threads on messages, and threads on the conversation', () => {
  const keys = conversationDiagramKeys(deleted);
  assert.ok(keys.has(rendererKey('mermaid', PRIVATE)));
  assert.ok(keys.has(rendererKey('mermaid', SHARED)));
  assert.ok(keys.has(rendererKey('mermaid', IN_THREAD)), 'a diagram in a side answer');
  assert.ok(keys.has(rendererKey('flowchart', 'A --> B   ')), 'a diagram in a thread kept on the conversation');
  assert.equal(keys.size, 4);
});

test('diagramKeysToForget: the deleted conversation\'s diagrams, except one another conversation still draws', () => {
  const forget = diagramKeysToForget(deleted, remaining);
  assert.ok(forget.includes(rendererKey('mermaid', PRIVATE)));
  assert.ok(!forget.includes(rendererKey('mermaid', SHARED)), 'still shown elsewhere');
  assert.equal(forget.length, 3);
});

test('deleting a conversation removes its diagram versions and notes from storage', () => {
  const values = new Map<string, string>();
  const storage: LedgerStorage = { getItem: k => values.get(k) ?? null, setItem: (k, v) => { values.set(k, v); } };
  const ledger = createRepairLedger(storage);
  const privateKey = rendererKey('mermaid', PRIVATE);
  const sharedKey = rendererKey('mermaid', SHARED);
  ledger.begin(privateKey, 'adjust');
  ledger.succeed(privateKey, 'flowchart LR\n  A[Salary review] --> B[Offer]', 'mention the bonus');
  ledger.begin(sharedKey, 'adjust');
  ledger.succeed(sharedKey, 'flowchart RL\n  E[Encoder] --> D[Decoder]', 'Arrows the other way');

  ledger.forget(diagramKeysToForget(deleted, remaining));
  const stored = values.get(LEDGER_STORAGE_KEY) ?? '';
  assert.ok(!stored.includes('Salary') && !stored.includes('mention the bonus'));
  assert.equal(createRepairLedger(storage).get(privateKey), EMPTY_RECORD, 'gone after a restart too');
  assert.equal(createRepairLedger(storage).get(sharedKey).versions.length, 1, 'kept for the conversation that stays');
});

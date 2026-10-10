/**
 * The one model correction a diagram may get: the request, reading the
 * reply, the offer rules and the ledger that caps it at one attempt.
 *   node --experimental-strip-types --test app/tests/mermaidRetry.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  LEDGER_STORAGE_KEY,
  MAX_PARSE_ERROR_CHARS,
  MAX_REPAIR_SOURCE_CHARS,
  composeMermaidFixRequest,
  createRepairLedger,
  extractMermaidReply,
  parseErrorText,
  repairOffer,
  sourceKey,
} from '../src/features/ask/visual/mermaidRetry.ts';
import type { LedgerStorage } from '../src/features/ask/visual/mermaidRetry.ts';

class MemoryStorage implements LedgerStorage {
  values = new Map<string, string>();
  failWrites = false;
  getItem(key: string) {
    return this.values.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    if (this.failWrites) throw new Error('QuotaExceededError');
    this.values.set(key, value);
  }
}

const BROKEN = 'flowchart TD\n  A[Start (init)] --> B';
const PARSE_ERROR = "Parse error on line 2:\n...  A[Start (init)] --> B\n-----------^\nExpecting 'SQE', got 'PS'";

test('sourceKey: stable, and different for different sources', () => {
  assert.equal(sourceKey(BROKEN), sourceKey(BROKEN));
  assert.notEqual(sourceKey(BROKEN), sourceKey(`${BROKEN} `));
  assert.match(sourceKey(BROKEN), /^mmd-[0-9a-z]+-[0-9a-z]+$/);
});

test('parseErrorText: first line for display, whole message capped', () => {
  const text = parseErrorText(PARSE_ERROR);
  assert.equal(text.short, 'Parse error on line 2:');
  assert.ok(text.full.includes("Expecting 'SQE'"));
  assert.equal(parseErrorText('x'.repeat(5_000)).full.length, MAX_PARSE_ERROR_CHARS);
  assert.equal(parseErrorText('  \n ').short, 'The diagram could not be drawn.');
});

test('the request carries the source and the parse error and asks for one block', () => {
  const request = composeMermaidFixRequest(BROKEN, PARSE_ERROR);
  assert.ok(request.includes('A[Start (init)] --> B'));
  assert.ok(request.includes("Expecting 'SQE', got 'PS'"));
  assert.ok(request.includes('```mermaid'));
  assert.ok(/exactly one ```mermaid code block/.test(request));
  assert.ok(/do not use tools/i.test(request));
});

test('extractMermaidReply: one diagram, from a fence or a bare reply', () => {
  const fixed = 'flowchart TD\n  A["Start (init)"] --> B';
  assert.equal(extractMermaidReply(`Here it is:\n\n\`\`\`mermaid\n${fixed}\n\`\`\`\n`), fixed);
  assert.equal(extractMermaidReply(`\`\`\`\n${fixed}\n\`\`\``), fixed, 'an unlabelled fence holding a diagram');
  assert.equal(extractMermaidReply(fixed), fixed, 'a bare diagram');
  assert.equal(extractMermaidReply(`~~~mermaid\r\n${fixed}\r\n~~~`), fixed);
});

test('extractMermaidReply: nothing usable is null', () => {
  assert.equal(extractMermaidReply('I cannot fix this diagram.'), null);
  assert.equal(extractMermaidReply('```mermaid\nflowchart TD\n  A-->B\n```\n```mermaid\ngraph LR\n  C-->D\n```'), null, 'two diagrams');
  assert.equal(extractMermaidReply('```python\nprint(1)\n```'), null);
  assert.equal(extractMermaidReply('```mermaid\nflowchart TD\n  A-->B'), null, 'unterminated fence');
  assert.equal(extractMermaidReply('```mermaid\n\n```'), null, 'empty fence');
});

test('repairOffer: automatic only on the latest answer, never twice, never for huge sources', () => {
  const none = { status: 'none' } as const;
  assert.equal(repairOffer({ eligible: true, latest: true, attempt: none, sourceChars: 100 }), 'auto');
  assert.equal(repairOffer({ eligible: true, latest: false, attempt: none, sourceChars: 100 }), 'button');
  assert.equal(repairOffer({ eligible: false, latest: true, attempt: none, sourceChars: 100 }), 'none');
  assert.equal(repairOffer({ eligible: true, latest: true, attempt: { status: 'pending' }, sourceChars: 100 }), 'none');
  assert.equal(repairOffer({ eligible: true, latest: true, attempt: { status: 'failed', message: 'x' }, sourceChars: 100 }), 'none');
  assert.equal(repairOffer({ eligible: true, latest: true, attempt: none, sourceChars: MAX_REPAIR_SOURCE_CHARS + 1 }), 'none');
});

test('ledger: one attempt per source, across remounts and restarts', () => {
  const storage = new MemoryStorage();
  const key = sourceKey(BROKEN);
  const ledger = createRepairLedger(storage);
  assert.equal(ledger.get(key).status, 'none');
  assert.equal(ledger.begin(key), true);
  assert.equal(ledger.begin(key), false, 'a second attempt is refused while one runs');
  ledger.succeed(key, 'flowchart TD\n  A --> B');
  assert.deepEqual(ledger.get(key), { status: 'fixed', source: 'flowchart TD\n  A --> B' });
  assert.equal(ledger.begin(key), false, 'nor after it succeeded');

  const reopened = createRepairLedger(storage);
  assert.deepEqual(reopened.get(key), { status: 'fixed', source: 'flowchart TD\n  A --> B' });
  assert.equal(reopened.begin(key), false, 'the cap survives a restart');
});

test('ledger: an attempt cut short by a restart counts as failed', () => {
  const storage = new MemoryStorage();
  const ledger = createRepairLedger(storage);
  ledger.begin('k');
  const reopened = createRepairLedger(storage);
  assert.equal(reopened.get('k').status, 'failed');
  assert.equal(reopened.begin('k'), false);
});

test('ledger: a run that never started is released; outcomes only apply to a pending attempt', () => {
  const ledger = createRepairLedger(null);
  ledger.begin('k');
  ledger.release('k');
  assert.equal(ledger.get('k').status, 'none');
  assert.equal(ledger.begin('k'), true, 'released: the one attempt is still available');
  ledger.fail('k', 'Its correction does not draw either: Parse error on line 1:\nmore');
  assert.deepEqual(ledger.get('k'), { status: 'failed', message: 'Its correction does not draw either: Parse error on line 1:' });
  ledger.succeed('k', 'graph TD\n  A-->B');
  ledger.release('k');
  assert.equal(ledger.get('k').status, 'failed', 'a settled attempt does not change');
});

test('ledger: oldest entries are forgotten past the cap; listeners hear changes', () => {
  const storage = new MemoryStorage();
  const ledger = createRepairLedger(storage, 2);
  let heard = 0;
  const unsubscribe = ledger.subscribe(() => { heard += 1; });
  ledger.begin('a');
  ledger.begin('b');
  ledger.begin('c');
  assert.equal(ledger.get('a').status, 'none');
  assert.equal(ledger.get('c').status, 'pending');
  assert.equal(heard, 3);
  unsubscribe();
  ledger.fail('c', 'x');
  assert.equal(heard, 3);
  assert.equal((JSON.parse(storage.getItem(LEDGER_STORAGE_KEY) ?? '[]') as unknown[]).length, 2);
});

test('ledger: storage failures and corrupt records do not break the cap', () => {
  const storage = new MemoryStorage();
  storage.values.set(LEDGER_STORAGE_KEY, '{not json');
  const ledger = createRepairLedger(storage);
  storage.failWrites = true;
  assert.equal(ledger.begin('k'), true);
  assert.equal(ledger.begin('k'), false, 'held in memory when storage refuses writes');

  storage.failWrites = false;
  storage.values.set(LEDGER_STORAGE_KEY, JSON.stringify([['ok', { status: 'fixed', source: 'graph TD' }], ['bad', { status: 'weird' }], [7, {}], 'x']));
  const reread = createRepairLedger(storage);
  assert.equal(reread.get('ok').status, 'fixed');
  assert.equal(reread.get('bad').status, 'none');
});

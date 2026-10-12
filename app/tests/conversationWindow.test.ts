/**
 * Windowed conversation tests (estimates, following output, kept messages,
 * find over unmounted messages). Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/conversationWindow.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  MESSAGE_GAP_PX,
  STICK_TO_BOTTOM_PX,
  countOccurrences,
  distanceFromBottom,
  estimateMessageHeight,
  findInMessages,
  foldCase,
  followsOutput,
  mountedIndexes,
  occurrenceNumber,
  occurrenceOffsets,
  searchableText,
  startCursor,
  stepCursor,
  totalOccurrences,
} from '../src/features/ask/conversationWindow.ts';

test('a question is estimated as a short bubble, an answer by its lines', () => {
  const question = estimateMessageHeight({ role: 'user', content: 'What is RRF?' });
  assert.ok(question >= 40 && question <= 100, String(question));
  const short = estimateMessageHeight({ role: 'assistant', content: 'One line.' });
  const long = estimateMessageHeight({ role: 'assistant', content: Array.from({ length: 40 }, () => 'x'.repeat(160)).join('\n') });
  assert.ok(long > short * 5, `${long} vs ${short}`);
  // 40 lines that wrap twice at the column's width.
  assert.ok(long >= 40 * 2 * 28, String(long));
});

test('estimates count a chart as a drawn block, not as its JSON source', () => {
  const json = JSON.stringify({ type: 'bar', data: Array.from({ length: 50 }, (_, i) => ({ x: i, y: i })) }, null, 2);
  const chart = estimateMessageHeight({ role: 'assistant', content: `Intro\n\n\`\`\`chart\n${json}\n\`\`\`` });
  const asCode = estimateMessageHeight({ role: 'assistant', content: `Intro\n\n\`\`\`json\n${json}\n\`\`\`` });
  assert.ok(chart < asCode, `${chart} vs ${asCode}`);
  assert.ok(chart >= 340, String(chart));
});

test('estimates include the gap after the message', () => {
  assert.ok(estimateMessageHeight({ role: 'user', content: '' }) > MESSAGE_GAP_PX);
});

test('the view follows output only near the bottom', () => {
  assert.equal(distanceFromBottom(5000, 4000, 1000), 0);
  assert.equal(distanceFromBottom(5000, 3000, 1000), 1000);
  assert.equal(followsOutput(5000, 4000, 1000), true);
  assert.equal(followsOutput(5000, 4000 - STICK_TO_BOTTOM_PX + 1, 1000), true);
  assert.equal(followsOutput(5000, 4000 - STICK_TO_BOTTOM_PX, 1000), false);
  // Overscrolled (elastic) positions read as at the bottom.
  assert.equal(distanceFromBottom(5000, 4100, 1000), 0);
});

test('kept messages are mounted with the visible range, sorted and unique', () => {
  assert.deepEqual(mountedIndexes([10, 11, 12], [99, 11, null, undefined, 3], 100), [3, 10, 11, 12, 99]);
  // Out-of-range keeps (a message that was removed) are ignored.
  assert.deepEqual(mountedIndexes([0, 1], [5, -1], 3), [0, 1]);
  assert.deepEqual(mountedIndexes([], [], 0), []);
});

test('searchable text drops markdown, citations and visual sources but keeps code', () => {
  const content = [
    '## Recall at k',
    'HNSW keeps **high recall** [1][2] and [a link](https://example.com) too.',
    '| Method | Recall |',
    '| --- | --- |',
    '| HNSW | 0.98 |',
    '```chart',
    '{"type":"bar","title":"secret chart json"}',
    '```',
    '```ts',
    'const recall = 0.98;',
    '```',
    '- `inline code` item',
  ].join('\n');
  const text = searchableText(content);
  assert.ok(text.includes('Recall at k'));
  assert.ok(text.includes('HNSW keeps high recall and a link too.'), text);
  assert.ok(text.includes('HNSW 0.98'));
  assert.ok(!text.includes('secret chart json'));
  assert.ok(text.includes('const recall = 0.98;'));
  assert.ok(text.includes('inline code item'));
  assert.ok(!text.includes('https://example.com'));
  assert.ok(!text.includes('[1]'));
});

test('occurrences are counted case-insensitively and without overlap', () => {
  assert.equal(countOccurrences('Recall, recall, RECALL', 'recall'), 3);
  assert.equal(countOccurrences('aaaa', 'aa'), 2);
  assert.equal(countOccurrences('anything', ''), 0);
  assert.deepEqual(occurrenceOffsets('Ab ab', 'AB'), [[0, 2], [3, 5]]);
});

test('case folding keeps offsets aligned when a character lengthens', () => {
  const text = 'İstanbul recall';
  assert.equal(foldCase(text).length, text.length);
  const [[start, end]] = occurrenceOffsets(text, 'RECALL');
  assert.equal(text.slice(start, end), 'recall');
});

test('find lists matching messages in order with their counts', () => {
  const matches = findInMessages(['no', 'recall here', 'Recall and recall', 'none'], '  recall ');
  assert.deepEqual(matches, [{ index: 1, count: 1 }, { index: 2, count: 2 }]);
  assert.equal(totalOccurrences(matches), 3);
  assert.deepEqual(findInMessages(['x'], '   '), []);
});

test('stepping walks every occurrence and wraps both ways', () => {
  const matches = [{ index: 4, count: 2 }, { index: 9, count: 1 }];
  let cursor = { match: 0, occurrence: 0 };
  const seen: number[] = [];
  for (let i = 0; i < 4; i++) {
    seen.push(occurrenceNumber(matches, cursor));
    cursor = stepCursor(matches, cursor, 1);
  }
  assert.deepEqual(seen, [1, 2, 3, 1]);
  cursor = stepCursor(matches, { match: 0, occurrence: 0 }, -1);
  assert.deepEqual(cursor, { match: 1, occurrence: 0 });
  cursor = stepCursor(matches, cursor, -1);
  assert.deepEqual(cursor, { match: 0, occurrence: 1 });
  assert.deepEqual(stepCursor([], { match: 3, occurrence: 2 }, 1), { match: 0, occurrence: 0 });
});

test('a new search starts at the first match from the view onwards', () => {
  const matches = [{ index: 2, count: 1 }, { index: 30, count: 1 }, { index: 80, count: 1 }];
  assert.deepEqual(startCursor(matches, 0), { match: 0, occurrence: 0 });
  assert.deepEqual(startCursor(matches, 31), { match: 2, occurrence: 0 });
  // Past every match: the last one.
  assert.deepEqual(startCursor(matches, 90), { match: 2, occurrence: 0 });
});

/**
 * "Go deeper" marks: reading prepared questions, prose marks and the
 * per-answer budget (src/features/ask/visual/marks.ts).
 *   node --experimental-strip-types --test app/tests/visualMarks.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_MARKS,
  blockMarks,
  findTextMarks,
  markBudget,
  placeTextMarks,
  readDeeper,
  stripTextMarks,
} from '../src/features/ask/visual/marks.ts';
import { parseDiagram } from '../src/features/ask/visual/diagramSpec.ts';
import { layoutDiagram } from '../src/features/ask/visual/diagramLayout.ts';
import { diagramSvg } from '../src/features/ask/visual/diagramSvg.ts';

const fence = (lang: string, body: unknown) => `\`\`\`${lang}\n${JSON.stringify(body)}\n\`\`\``;

test('a prepared question is flattened, and empty or overlong ones are dropped', () => {
  assert.equal(readDeeper('  Why does   attention scale?  '), 'Why does attention scale?');
  assert.equal(readDeeper(''), null);
  assert.equal(readDeeper('   '), null);
  assert.equal(readDeeper(42), null);
  assert.equal(readDeeper('x'.repeat(160)), 'x'.repeat(160));
  assert.equal(readDeeper('x'.repeat(161)), null);
});

test('prose marks are found with their shown text and question', () => {
  const text = 'It uses [[self-attention|Why does self-attention cost grow with length?]] over tokens.';
  const marks = findTextMarks(text);
  assert.equal(marks.length, 1);
  assert.equal(marks[0].shown, 'self-attention');
  assert.equal(marks[0].question, 'Why does self-attention cost grow with length?');
  assert.equal(text.slice(marks[0].start, marks[0].end), '[[self-attention|Why does self-attention cost grow with length?]]');
});

test('an unfinished mark (still streaming) is not a mark', () => {
  assert.deepEqual(findTextMarks('It uses [[self-attention|Why does'), []);
  assert.equal(stripTextMarks('It uses [[self-attention|Why does'), 'It uses [[self-attention|Why does');
});

test('a mark whose question is too long keeps its shown text but has no question', () => {
  const marks = findTextMarks(`[[term|${'q'.repeat(161)}]]`);
  assert.equal(marks.length, 1, 'still a mark, so its syntax never shows');
  assert.equal(marks[0].question, null);
  assert.equal(stripTextMarks(`A [[term|${'q'.repeat(161)}]].`), 'A term.');
  const ok = findTextMarks(`[[term|${'q'.repeat(160)}]]`);
  assert.equal(ok[0].question, 'q'.repeat(160));
});

test('placing marks skips inline code and keeps only the allowed number', () => {
  const text = 'Use `[[a|qa?]]`, then [[b|qb?]], [[c|qc?]] and [[d|]].';
  const placed = placeTextMarks(text, 1, i => `<${i}>`);
  assert.equal(placed.text, 'Use `[[a|qa?]]`, then <0>, c and d.');
  assert.deepEqual(placed.marks, [{ shown: 'b', question: 'qb?' }]);
});

test('stripping marks leaves the shown text', () => {
  assert.equal(stripTextMarks('Uses [[RoPE|What is rotary position embedding?]] and [[ALiBi|How does ALiBi bias attention?]].'), 'Uses RoPE and ALiBi.');
});

test('marks inside code do not count and are not stripped', () => {
  const content = 'See `[[a|b?]]` here.\n\n```text\n[[c|d?]]\n```\n\nThen [[e|f?]].';
  const budget = markBudget(content);
  assert.equal(budget.text, 1);
  assert.equal(stripTextMarks('See `[[a|b?]]` here.'), 'See `[[a|b?]]` here.');
});

test('block marks are read from each visual kind', () => {
  assert.deepEqual(
    blockMarks('diagram', JSON.stringify({ layout: 'flow', nodes: [{ id: 'a', label: 'Attention', deeper: 'Why quadratic?' }, { id: 'b', label: 'FFN' }] })),
    [{ label: 'Attention', question: 'Why quadratic?' }],
  );
  assert.deepEqual(
    blockMarks('chart', JSON.stringify({ type: 'line', xKey: 'year', series: [{ key: 's', label: 'Score', deeper: 'What drove the 2024 jump?' }], data: [{ year: 2023, s: 1 }] })),
    [{ label: 'Score', question: 'What drove the 2024 jump?' }],
  );
  assert.deepEqual(
    blockMarks('chart', JSON.stringify({ type: 'bar', data: { labels: ['a'], datasets: [{ label: 'Cost', data: [1], deeper: 'Why so high?' }] } })),
    [{ label: 'Cost', question: 'Why so high?' }],
  );
  assert.deepEqual(
    blockMarks('plot', JSON.stringify({ params: [{ name: 'v0', label: 'v0 (m/s)', deeper: 'What sets the launch speed?' }, { name: 'g' }] })),
    [{ label: 'v0 (m/s)', question: 'What sets the launch speed?' }],
  );
  assert.deepEqual(
    blockMarks('simulation', JSON.stringify({ params: [{ name: 'k', deeper: 'What is a spring constant?' }] })),
    [{ label: 'k', question: 'What is a spring constant?' }],
  );
  assert.deepEqual(
    blockMarks('derivation', JSON.stringify({ steps: [{ latex: 'a=b' }, { latex: 'b=c', deeper: 'Why can we substitute here?' }] })),
    [{ label: 'Step 2', question: 'Why can we substitute here?' }],
  );
  assert.deepEqual(
    blockMarks('symbols', JSON.stringify([{ symbol: '\\Phi_q', meaning: 'feature map', deeper: 'What is a feature map?' }])),
    [{ label: '\\Phi_q', question: 'What is a feature map?' }],
  );
  assert.deepEqual(blockMarks('diagram', '{not json'), []);
  assert.deepEqual(blockMarks('python', '{"deeper": "x"}'), []);
});

test('the budget allows four marks per answer in document order', () => {
  const diagram = { layout: 'flow', nodes: [
    { id: 'a', label: 'A', deeper: 'qa?' }, { id: 'b', label: 'B', deeper: 'qb?' }, { id: 'c', label: 'C', deeper: 'qc?' },
  ] };
  const content = `First [[x|qx?]] then [[y|qy?]].\n\n${fence('diagram', diagram)}\n\nLast [[z|qz?]].`;
  const budget = markBudget(content);
  assert.equal(MAX_MARKS, 4);
  assert.equal(budget.text, 2, 'the two prose marks before the diagram; the last one is over budget');
  assert.equal(budget.blocks.get(JSON.stringify(diagram)), 2, 'the diagram gets the remaining two');
});

test('an answer without marks has an empty budget', () => {
  const budget = markBudget('Plain text.\n\n```chart\n{"type":"bar"}\n```');
  assert.equal(budget.text, 0);
  assert.equal(budget.blocks.get('{"type":"bar"}') ?? 0, 0);
});

test('repeated identical blocks get no marks, so the 4-per-answer cap holds', () => {
  const diagram = { layout: 'flow', nodes: [
    { id: 'a', label: 'A', deeper: 'qa?' }, { id: 'b', label: 'B', deeper: 'qb?' }, { id: 'c', label: 'C', deeper: 'qc?' },
  ] };
  const diagramBody = JSON.stringify(diagram);
  const content = `First [[x|qx?]].\n\n${fence('diagram', diagram)}\n\nMiddle.\n\n${fence('diagram', diagram)}`;
  const budget = markBudget(content);
  assert.equal(budget.text, 1, 'prose mark gets the first allowance');
  assert.equal(budget.blocks.get(diagramBody), 0, 'repeated identical blocks get 0');
  // Total marks across all blocks plus text should never exceed MAX_MARKS.
  let total = budget.text;
  for (const allow of budget.blocks.values()) {
    total += allow;
  }
  assert(total <= MAX_MARKS, `total marks ${total} must not exceed MAX_MARKS ${MAX_MARKS}`);
});

test('diagram nodes keep a usable prepared question and drop an unusable one', () => {
  const r = parseDiagram(JSON.stringify({ layout: 'flow', nodes: [
    { id: 'a', label: 'Attention', deeper: ' Why is it quadratic? ' },
    { id: 'b', label: 'FFN', deeper: 'x'.repeat(161) },
    { id: 'c', label: 'Norm' },
  ] }), { citations: null, codeMode: false });
  assert.ok(r.ok);
  if (!r.ok) return;
  assert.equal(r.spec.nodes[0].deeper, 'Why is it quadratic?');
  assert.equal(r.spec.nodes[1].deeper, null);
  assert.equal(r.spec.nodes[2].deeper, null);
});

test('marked nodes are drawn with the glow and say so to screen readers', () => {
  const r = parseDiagram(JSON.stringify({ layout: 'flow', nodes: [
    { id: 'a', label: 'Attention', deeper: 'Why quadratic?' }, { id: 'b', label: 'FFN' },
  ], edges: [{ from: 'a', to: 'b' }] }), { citations: null, codeMode: false });
  assert.ok(r.ok);
  if (!r.ok) return;
  const svg = diagramSvg(r.spec, layoutDiagram(r.spec), { interactive: true, idPrefix: 't', marked: new Set([0]) });
  assert.match(svg, /data-node="0"[^>]*data-deeper="true"/);
  assert.match(svg, /aria-label="Attention, go deeper available"/);
  assert.doesNotMatch(svg, /data-node="1"[^>]*data-deeper/);
  const plain = diagramSvg(r.spec, layoutDiagram(r.spec), { interactive: true, idPrefix: 't' });
  assert.doesNotMatch(plain, /data-deeper/);
});

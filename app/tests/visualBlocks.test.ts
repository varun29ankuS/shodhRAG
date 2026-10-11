/**
 * Visual answer blocks: chart parsing and math preprocessing.
 *   node --experimental-strip-types --test app/tests/visualBlocks.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseChartBlock } from '../src/features/ask/visual/chartSpec.ts';
import { escapeCurrency, isMermaidLanguage, mermaidSource, normalizeMathDelimiters, protectMath } from '../src/features/ask/visual/mathText.ts';

test('chart: preferred rows + xKey + series shape', () => {
  const r = parseChartBlock(JSON.stringify({
    type: 'bar', title: 'Recall@10', xKey: 'method',
    series: [{ key: 'recall', label: 'Recall@10' }],
    data: [{ method: 'HNSW', recall: 0.98 }, { method: 'IVF-PQ', recall: '0.91' }],
  }));
  assert.ok(r.ok);
  if (!r.ok) return;
  assert.deepEqual(r.chart.data.labels, ['HNSW', 'IVF-PQ']);
  assert.deepEqual(r.chart.data.datasets, [{ label: 'Recall@10', data: [0.98, 0.91] }]);
});

test('chart: legacy labels + datasets shape still works', () => {
  const r = parseChartBlock('{"type":"line","data":{"labels":["a","b"],"datasets":[{"label":"x","data":[1,"n/a"]}]}}');
  assert.ok(r.ok);
  if (r.ok) assert.deepEqual(r.chart.data.datasets[0].data, [1, null]);
});

test('chart: clear errors for bad input', () => {
  assert.equal(parseChartBlock('not json').ok, false);
  assert.equal(parseChartBlock('{"type":"radar3d","xKey":"a","series":[{"key":"b"}],"data":[{"a":1}]}').ok, false);
  assert.equal(parseChartBlock('{"type":"bar","xKey":"a","series":[],"data":[{"a":1}]}').ok, false);
});

test('chart: oversized data is capped', () => {
  const data = Array.from({ length: 2000 }, (_, i) => ({ x: i, y: i }));
  const r = parseChartBlock(JSON.stringify({ type: 'line', xKey: 'x', series: [{ key: 'y' }], data }));
  assert.ok(r.ok);
  if (r.ok) assert.equal(r.chart.data.labels.length, 500);
});

test('math: \\( \\) and \\[ \\] become dollars', () => {
  assert.equal(normalizeMathDelimiters('a \\(x^2\\) b \\[\\sum_i x_i\\]'), 'a $x^2$ b $$\\sum_i x_i$$');
});

test('math: an equation alone on its line becomes display math', () => {
  assert.equal(normalizeMathDelimiters('Intro\n$$ f(x) = \\sum_q \\Phi_q $$\nAfter'), 'Intro\n$$\nf(x) = \\sum_q \\Phi_q\n$$\nAfter');
  assert.equal(normalizeMathDelimiters('inline $$a+b$$ stays'), 'inline $$a+b$$ stays');
});

test('math: currency is escaped, math is not', () => {
  assert.equal(escapeCurrency('costs $5 and $1,200'), 'costs \\$5 and \\$1,200');
  assert.equal(escapeCurrency('$x$ and $$y$$'), '$x$ and $$y$$');
});

test('math: segments survive the citation rewrite', () => {
  const { text, restore } = protectMath('state $S_t = S_{t-1}[1]$ grows [2] and $$W[3]$$');
  assert.ok(!text.includes('[1]') && !text.includes('[3]'));
  assert.ok(text.includes('[2]'));
  assert.equal(restore(text), 'state $S_t = S_{t-1}[1]$ grows [2] and $$W[3]$$');
});

test('mermaid: languages and headers', () => {
  assert.ok(isMermaidLanguage('mermaid') && isMermaidLanguage('flowchart') && isMermaidLanguage('sequenceDiagram'));
  assert.ok(!isMermaidLanguage('python'));
  assert.equal(mermaidSource('flowchart', 'A-->B'), 'flowchart TD\nA-->B');
  assert.equal(mermaidSource('mermaid', 'graph LR\nA-->B'), 'graph LR\nA-->B');
  assert.equal(mermaidSource('sequence', 'A->>B: hi'), 'sequenceDiagram\nA->>B: hi');
});

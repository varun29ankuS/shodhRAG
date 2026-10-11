/**
 * Cross-paper comparison: table rows, chart data, facets and grouping.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/researchComparison.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  chartNumber,
  columnChart,
  columnLabel,
  comparisonRows,
  groupByTable,
  provenanceLabel,
  sortFacet,
  valueLabel,
} from '../src/features/research/comparison.ts';
import type { Comparison, ComparisonCell, ResultRecord } from '../src/features/research/types.ts';

function cell(filePath: string, value: string, overrides: Partial<ComparisonCell> = {}): ComparisonCell {
  return {
    resultId: `${filePath}:${value}`,
    value,
    valueText: value,
    unit: null,
    filePath,
    fileName: filePath.split('/').pop() ?? filePath,
    page: 6,
    region: { page: 6, x0: 300, y0: 500, x1: 320, y1: 510 },
    setting: null,
    ...overrides,
  };
}

const comparison: Comparison = {
  columns: [
    { key: 'sift1m|recall@10', dataset: 'SIFT1M', metric: 'recall@10' },
    { key: 'gist1m|recall@10', dataset: 'GIST1M', metric: 'recall@10' },
  ],
  rows: [
    { method: 'HNSW', methodId: 'method:hnsw', cells: { 'sift1m|recall@10': [cell('a/p1.pdf', '0.95'), cell('a/p2.pdf', '0.97')] } },
    { method: 'IVF-PQ', methodId: 'method:ivf-pq', cells: { 'sift1m|recall@10': [cell('a/p1.pdf', '0.81'), cell('a/p1.pdf', '0.84', { setting: 'nprobe=64' })], 'gist1m|recall@10': [cell('a/p2.pdf', 'n/a')] } },
  ],
  papers: [
    { filePath: 'a/p1.pdf', fileName: 'p1.pdf' },
    { filePath: 'a/p2.pdf', fileName: 'p2.pdf' },
  ],
  notes: ['2 papers report recall@10 on SIFT1M'],
  pendingReview: 1,
  truncated: false,
};

test('rows have a cell for every column, empty where nothing was reported', () => {
  const rows = comparisonRows(comparison);
  assert.equal(rows.length, 2);
  assert.deepEqual(rows[0].cells.map(c => c.values.length), [2, 0]);
  assert.deepEqual(rows[1].cells.map(c => c.values.length), [2, 1]);
  assert.equal(columnLabel(comparison.columns[0]), 'SIFT1M · recall@10');
  assert.equal(columnLabel({ dataset: '', metric: '' }), 'Value');
});

test('a column charts methods by paper, first value per paper, skipping non-numbers', () => {
  const chart = columnChart(comparison, 'sift1m|recall@10');
  assert.ok(chart);
  assert.deepEqual(chart.spec.series, [
    { key: 'p1', label: 'p1.pdf' },
    { key: 'p2', label: 'p2.pdf' },
  ]);
  assert.deepEqual(chart.spec.data, [
    { method: 'HNSW', p1: 0.95, p2: 0.97 },
    { method: 'IVF-PQ', p1: 0.81, p2: null },
  ]);
  assert.equal(chart.multiple, 1);
  assert.equal(chart.spec.title, 'SIFT1M · recall@10');
  // Only a non-number in the column: nothing to chart.
  assert.equal(columnChart(comparison, 'gist1m|recall@10'), null);
  assert.equal(columnChart(comparison, 'missing'), null);
});

test('values parse as printed numbers only', () => {
  assert.equal(chartNumber('1,234.5'), 1234.5);
  assert.equal(chartNumber('−0.68'), -0.68);
  assert.equal(chartNumber('95.3±0.2'), null);
  assert.equal(chartNumber('n/a'), null);
  assert.equal(valueLabel({ valueText: '445.47 mJ', value: '445.47', unit: 'mJ' }), '445.47 mJ');
  assert.equal(valueLabel({ valueText: '78.0', value: '78.0', unit: '%' }), '78.0 %');
});

test('facets sort by use, results group by table', () => {
  assert.deepEqual(
    sortFacet([
      { id: 'b', label: 'B', count: 1 },
      { id: 'a', label: 'A', count: 3 },
      { id: 'c', label: 'C', count: 1 },
    ]).map(f => f.id),
    ['a', 'b', 'c'],
  );
  const base: Omit<ResultRecord, 'id' | 'method' | 'page' | 'tableCaption' | 'region'> = {
    dataset: 'Books', metric: 'Ppl.', methodId: 'm', datasetId: 'd', metricId: 'k', value: '1', valueText: '1', unit: null, setting: null,
    filePath: 'p.pdf', fileName: 'p.pdf', extractor: 'rule', confidence: 0.9, status: 'accepted',
  };
  const groups = groupByTable([
    { ...base, id: '2', method: 'B', page: 11, tableCaption: null, region: { page: 11, x0: 0, y0: 600, x1: 1, y1: 610 } },
    { ...base, id: '1', method: 'A', page: 11, tableCaption: null, region: { page: 11, x0: 0, y0: 650, x1: 1, y1: 660 } },
    { ...base, id: '3', method: 'C', page: 3, tableCaption: 'Table 1: Main', region: null },
  ]);
  assert.deepEqual(groups.map(g => g.caption), ['Table 1: Main', 'Table on page 11']);
  assert.deepEqual(groups[1].results.map(r => r.id), ['1', '2']);
  assert.equal(provenanceLabel({ extractor: 'llm', confidence: 0.734 }), 'model-assisted · 73%');
});

/**
 * Citation graph view logic: filters, capping with clusters, the list view.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/graphModel.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  DEFAULT_FILTERS,
  graphList,
  methodNeighbourhood,
  nodeRadius,
  progressLabel,
  restrictGraph,
  shapeGraph,
  shortLabel,
  yearBounds,
} from '../src/features/research/graphModel.ts';
import type { GraphViewData, ViewNode } from '../src/features/research/graphTypes.ts';

function node(id: string, year: number | null, inLibrary: boolean, libraryCiters = 0, methods: string[] = []): ViewNode {
  return { id, label: id, year, inLibrary, filePath: inLibrary ? `C:/p/${id}.pdf` : null, libraryCiters, citedByCount: null, methods };
}

const data: GraphViewData = {
  nodes: [
    node('fwp', 2021, true, 1),
    node('delta', 2024, true, 1, ['method:deltanet']),
    node('neg', 2024, true, 0, ['method:deltanet']),
    node('attention', 2017, false, 2),
    node('linear', 2020, false, 3),
    node('lstm', 1997, false, 1),
  ],
  edges: [
    ['fwp', 'attention'],
    ['fwp', 'linear'],
    ['fwp', 'lstm'],
    ['delta', 'fwp'],
    ['delta', 'attention'],
    ['delta', 'linear'],
    ['neg', 'delta'],
    ['neg', 'linear'],
  ],
  methods: [{ id: 'method:deltanet', label: 'DeltaNet' }],
  omitted: 0,
};

test('everything is drawn under the cap', () => {
  const g = shapeGraph(data, DEFAULT_FILTERS);
  assert.equal(g.nodes.length, 6);
  assert.equal(g.edges.length, 8);
  assert.equal(g.clustered, 0);
  assert.equal(g.nodes.filter(n => n.kind === 'library').length, 3);
});

test('past the cap, the least cited works fold into clusters per citing library paper', () => {
  const g = shapeGraph(data, DEFAULT_FILTERS, 4);
  const drawnPapers = g.nodes.filter(n => n.kind !== 'cluster').map(n => n.id);
  assert.deepEqual(drawnPapers, ['fwp', 'delta', 'neg', 'linear']);
  const clusters = g.nodes.filter(n => n.kind === 'cluster');
  assert.deepEqual(clusters.map(c => [c.parent, c.label]), [
    ['delta', '+1 reference'],
    ['fwp', '+2 references'],
  ]);
  assert.equal(g.clustered, 2);
  assert.ok(g.edges.some(e => e.source === 'fwp' && e.target === 'cluster:fwp'));
  // Library papers are drawn even when they alone exceed the cap.
  assert.equal(shapeGraph(data, DEFAULT_FILTERS, 1).nodes.filter(n => n.kind === 'library').length, 3);
});

test('filters: library only, years and method neighbourhood', () => {
  const lib = shapeGraph(data, { ...DEFAULT_FILTERS, libraryOnly: true });
  assert.deepEqual(lib.nodes.map(n => n.id), ['fwp', 'delta', 'neg']);
  assert.equal(lib.edges.length, 2);
  const recent = shapeGraph(data, { ...DEFAULT_FILTERS, yearFrom: 2020, yearTo: 2022 });
  assert.deepEqual(recent.nodes.map(n => n.id).sort(), ['fwp', 'linear']);
  const keep = methodNeighbourhood(data, 'method:deltanet');
  assert.deepEqual([...(keep ?? [])].sort(), ['attention', 'delta', 'fwp', 'linear', 'neg']);
  assert.equal(methodNeighbourhood(data, null), null);
  const byMethod = shapeGraph(data, { ...DEFAULT_FILTERS, method: 'method:deltanet' });
  assert.ok(!byMethod.nodes.some(n => n.id === 'lstm'));
});

test('the list shows the same papers with counts, library first', () => {
  const rows = graphList(data, DEFAULT_FILTERS);
  assert.equal(rows.length, 6);
  assert.deepEqual(rows.slice(0, 3).map(r => r.node.id), ['delta', 'fwp', 'neg']);
  const linear = rows.find(r => r.node.id === 'linear');
  assert.equal(linear?.citedBy, 3);
  const fwp = rows.find(r => r.node.id === 'fwp');
  assert.equal(fwp?.cites, 3);
  assert.equal(graphList(data, { ...DEFAULT_FILTERS, libraryOnly: true }).length, 3);
});

test('helpers', () => {
  assert.deepEqual(yearBounds(data.nodes), { min: 1997, max: 2024 });
  assert.equal(yearBounds([]), null);
  assert.ok(nodeRadius({ id: 'a', label: 'a', kind: 'library', year: null, weight: 0 }) > nodeRadius({ id: 'b', label: 'b', kind: 'external', year: null, weight: 0 }));
  assert.equal(progressLabel({ stage: 'scanning', done: 0, total: 3, file: 'a.pdf' }), 'Reading a.pdf (1 of 3)');
  assert.equal(shortLabel('abcdef', 4), 'abc…');
});

test('a workspace graph keeps its library papers and what they cite, nothing else from the library', () => {
  const kept = restrictGraph(data, n => n.id === 'delta');
  assert.deepEqual(kept.nodes.map(n => n.id).sort(), ['attention', 'delta', 'linear']);
  // delta cites fwp, a library paper outside the workspace: left out, with the edge.
  assert.deepEqual(kept.edges, [['delta', 'attention'], ['delta', 'linear']]);
  assert.equal(restrictGraph(data, () => false).nodes.length, 0);
  assert.equal(kept.methods.length, 1);
});

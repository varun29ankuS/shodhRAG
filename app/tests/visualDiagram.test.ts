/**
 * Diagrams from structure: schema checks, feedback-loop labels, layout
 * determinism and the SVG forms (src/features/ask/visual/diagram*.ts).
 *   node --experimental-strip-types --test app/tests/visualDiagram.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { cleanPath, describeDiagram, describeNode, feedbackLoops, parseDiagram } from '../src/features/ask/visual/diagramSpec.ts';
import type { DiagramContext, DiagramSpec } from '../src/features/ask/visual/diagramSpec.ts';
import { layoutDiagram, roundedPath, wrapLabel } from '../src/features/ask/visual/diagramLayout.ts';
import { diagramSvg } from '../src/features/ask/visual/diagramSvg.ts';
import { sanitizeSvg } from '../src/features/ask/visual/svgSanitize.ts';

const research: DiagramContext = { citations: new Set([1, 2, 3]), codeMode: false };
const code: DiagramContext = { citations: new Set(), codeMode: true };

function spec(source: unknown, context = research): DiagramSpec {
  const r = parseDiagram(JSON.stringify(source), context);
  assert.ok(r.ok, r.ok ? '' : r.errors.join('\n'));
  return r.ok ? r.spec : (null as never);
}

function errors(source: unknown, context = research): string[] {
  const r = parseDiagram(typeof source === 'string' ? source : JSON.stringify(source), context);
  assert.ok(!r.ok, 'expected the diagram to be refused');
  return r.ok ? [] : r.errors;
}

const FLOW = {
  layout: 'flow',
  title: 'Indexing',
  nodes: [
    { id: 'parse', label: 'Parse PDF', kind: 'step', cite: 1 },
    { id: 'chunk', label: 'Split into chunks' },
    { id: 'embed', label: 'Embed chunks', emphasis: true, cite: 2 },
    { id: 'store', label: 'Vector store', kind: 'store' },
  ],
  edges: [
    { from: 'parse', to: 'chunk', label: 'text' },
    { from: 'chunk', to: 'embed' },
    { from: 'embed', to: 'store', label: 'vectors' },
  ],
};

test('diagram: a valid flow is read with its citations and emphasis', () => {
  const s = spec(FLOW);
  assert.equal(s.layout, 'flow');
  assert.equal(s.nodes.length, 4);
  assert.equal(s.nodes[0].cite, 1);
  assert.equal(s.nodes[2].emphasis, true);
  assert.equal(s.edges[2].label, 'vectors');
});

test('diagram: edges must join declared nodes and ids must be unique', () => {
  const bad = errors({ ...FLOW, edges: [{ from: 'parse', to: 'nowhere' }] });
  assert.ok(bad.some(e => e.includes('"nowhere" is not a node id')), bad.join('\n'));
  const twice = errors({ ...FLOW, nodes: [...FLOW.nodes, { id: 'parse', label: 'Again' }] });
  assert.ok(twice.some(e => e.includes('used twice')));
  const badId = errors({ ...FLOW, nodes: [{ id: '1st node', label: 'x' }], edges: [] });
  assert.ok(badId.some(e => e.includes('must start with a letter')));
  assert.ok(errors('not json').some(e => e.includes('not valid JSON')));
  assert.ok(errors({ layout: 'mindmap', nodes: [] }).some(e => e.includes('"layout" must be one of')));
});

test('diagram: citations must be sources of the answer', () => {
  const unknown = errors({ ...FLOW, nodes: [{ id: 'a', label: 'A', cite: 7 }], edges: [] });
  assert.ok(unknown.some(e => e.includes('cites [7]')));
  // Text without sources cannot cite at all.
  const none = errors({ ...FLOW, nodes: [{ id: 'a', label: 'A', cite: 1 }], edges: [] }, { citations: null, codeMode: false });
  assert.ok(none.some(e => e.includes('cites [1]')));
  assert.equal(spec({ ...FLOW, nodes: [{ id: 'a', label: 'A', cite: '[3]' }], edges: [] }).nodes[0].cite, 3);
});

test('diagram: file paths are Code mode only, relative and inside the folder', () => {
  const nodes = [
    { id: 'api', label: 'API', path: 'src/api/mod.rs' },
    { id: 'old', label: 'Old cache', path: 'src/cache.rs', change: 'removed' },
  ];
  const r = parseDiagram(JSON.stringify({ layout: 'architecture', nodes, edges: [] }), code);
  assert.ok(r.ok);
  // Only existing parts are checked on disk: a removed node's file is gone.
  assert.deepEqual(r.ok ? r.paths : [], ['src/api/mod.rs']);
  assert.ok(errors({ layout: 'flow', nodes: [nodes[0]], edges: [] }).some(e => e.includes('only Code mode')));
  for (const path of ['../secret', '/etc/passwd', 'C:/Windows', 'src/../../x']) {
    assert.ok(errors({ layout: 'flow', nodes: [{ id: 'a', label: 'A', path }], edges: [] }, code).some(e => e.includes('relative to the code folder')), path);
  }
  assert.equal(cleanPath('.\\src\\lib.rs'), 'src/lib.rs');
  assert.equal(cleanPath('src/'), 'src');
});

test('diagram: budgets and per-layout rules are enforced', () => {
  const many = Array.from({ length: 13 }, (_, i) => ({ id: `n${i}`, label: `Node ${i}` }));
  assert.ok(errors({ layout: 'flow', nodes: many, edges: [] }).some(e => e.includes('at most 12 nodes')));
  const emphasised = FLOW.nodes.map(n => ({ ...n, emphasis: true }));
  assert.ok(errors({ ...FLOW, nodes: emphasised }).some(e => e.includes('At most 2 nodes')));
  assert.ok(errors({ layout: 'timeline', nodes: [{ id: 'a', label: 'A' }] }).some(e => e.includes('"at"')));
  assert.ok(errors({ layout: 'layers', nodes: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }], edges: [{ from: 'a', to: 'b' }] }).some(e => e.includes('no edges')));
  assert.ok(errors({ layout: 'quadrant', nodes: [{ id: 'a', label: 'A', x: 2, y: 0.5 }] }).some(e => e.includes('"x" must be')));
  assert.ok(errors({ layout: 'quadrant', nodes: [{ id: 'a', label: 'A', x: 0.2, y: 0.5 }] }).some(e => e.includes('"axes"')));
  assert.ok(errors({ layout: 'loop', nodes: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }], edges: [{ from: 'a', to: 'b' }] }).some(e => e.includes('"sign"')));
  assert.ok(errors({ layout: 'loop', nodes: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }], edges: [{ from: 'a', to: 'b', sign: '+' }] }).some(e => e.includes('at least one cycle')));
});

test('loops: each cycle is listed once and labelled R (even minus links) or B (odd)', () => {
  const s = spec({
    layout: 'loop',
    nodes: [
      { id: 'users', label: 'Users' },
      { id: 'content', label: 'Content' },
      { id: 'quality', label: 'Quality' },
      { id: 'load', label: 'Load' },
    ],
    edges: [
      { from: 'users', to: 'content', sign: '+' },
      { from: 'content', to: 'users', sign: '+' },
      { from: 'users', to: 'load', sign: '+' },
      { from: 'load', to: 'quality', sign: '-' },
      { from: 'quality', to: 'users', sign: '+' },
      { from: 'quality', to: 'quality', sign: '-' },
    ],
  });
  const loops = feedbackLoops(s);
  assert.deepEqual(
    loops.map(l => `${l.polarity}:${l.nodes.join('>')}`),
    ['R:users>content', 'B:users>load>quality', 'B:quality'],
  );
  // Two minus links make a reinforcing loop.
  const twoMinus = spec({
    layout: 'loop',
    nodes: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }],
    edges: [{ from: 'a', to: 'b', sign: '-' }, { from: 'b', to: 'a', sign: '−' }],
  });
  assert.deepEqual(feedbackLoops(twoMinus).map(l => l.polarity), ['R']);
  // Parallel links with different signs are different loops.
  const parallel = spec({
    layout: 'loop',
    nodes: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }],
    edges: [{ from: 'a', to: 'b', sign: '+' }, { from: 'a', to: 'b', sign: '-' }, { from: 'b', to: 'a', sign: '+' }],
  });
  assert.deepEqual(feedbackLoops(parallel).map(l => l.polarity), ['R', 'B']);
  assert.ok(describeDiagram(s).includes('Loop R1: Users → Content → Users'));
});

const SAMPLES: unknown[] = [
  FLOW,
  {
    layout: 'architecture',
    nodes: [
      { id: 'ui', label: 'React UI', group: 'App' },
      { id: 'cmd', label: 'Tauri commands', group: 'App', emphasis: true },
      { id: 'rag', label: 'RAG engine', group: 'Core' },
      { id: 'db', label: 'LanceDB', kind: 'store', group: 'Core', change: 'added' },
      { id: 'old', label: 'Legacy index', group: 'Core', change: 'removed' },
    ],
    edges: [
      { from: 'ui', to: 'cmd', label: 'invoke' },
      { from: 'cmd', to: 'rag' },
      { from: 'rag', to: 'db', change: 'added' },
      { from: 'rag', to: 'old', change: 'removed' },
      { from: 'db', to: 'rag', kind: 'async', label: 'events' },
    ],
  },
  {
    layout: 'sequence',
    nodes: [{ id: 'u', label: 'User' }, { id: 'a', label: 'App' }, { id: 'm', label: 'Model' }],
    edges: [
      { from: 'u', to: 'a', label: 'ask' },
      { from: 'a', to: 'm', label: 'prompt' },
      { from: 'm', to: 'a', label: 'tokens', kind: 'return' },
      { from: 'a', to: 'a', label: 'check' },
    ],
  },
  { layout: 'layers', nodes: [{ id: 'ui', label: 'Interface' }, { id: 'core', label: 'Core', emphasis: true }, { id: 'os', label: 'Operating system' }] },
  { layout: 'timeline', nodes: [{ id: 'a', label: 'Spike', at: '2026-09' }, { id: 'b', label: 'Beta', at: '2026-10' }, { id: 'c', label: 'Release', at: '2026-12' }] },
  {
    layout: 'quadrant',
    axes: { x: ['cheap', 'costly'], y: ['low value', 'high value'] },
    nodes: [{ id: 'a', label: 'Cache', x: 0.2, y: 0.8 }, { id: 'b', label: 'Rewrite', x: 0.9, y: 0.4 }],
  },
  {
    layout: 'loop',
    nodes: [{ id: 'a', label: 'Adoption' }, { id: 'b', label: 'Feedback' }, { id: 'c', label: 'Quality' }],
    edges: [{ from: 'a', to: 'b', sign: '+' }, { from: 'b', to: 'c', sign: '+' }, { from: 'c', to: 'a', sign: '+' }],
  },
];

test('layout: every layout is deterministic, on the grid and inside its canvas', () => {
  for (const sample of SAMPLES) {
    const s = spec(sample);
    const first = layoutDiagram(s);
    const again = layoutDiagram(spec(sample));
    assert.deepEqual(again, first, s.layout);
    assert.equal(first.width % 4, 0);
    assert.equal(first.height % 4, 0);
    for (const b of first.boxes) {
      assert.ok(b.x >= 0 && b.y >= 0 && b.x + b.w <= first.width && b.y + b.h <= first.height, `${s.layout}: ${JSON.stringify(b)}`);
    }
    assert.equal(first.boxes.length, s.nodes.length);
    assert.equal(first.wires.length, s.edges.length);
    const svg = diagramSvg(s, first, { interactive: true, idPrefix: 'd1' });
    assert.equal(svg, diagramSvg(s, layoutDiagram(s), { interactive: true, idPrefix: 'd1' }));
  }
});

test('layout: flow goes down in layers, boxes of one layer do not overlap', () => {
  const d = layoutDiagram(spec(FLOW));
  const ys = d.boxes.map(b => b.y);
  assert.ok(ys[0] < ys[1] && ys[1] < ys[2] && ys[2] < ys[3]);
  const arch = layoutDiagram(spec(SAMPLES[1]));
  const [ui, cmd, rag] = arch.boxes;
  assert.ok(ui.x < cmd.x && cmd.x < rag.x, 'architecture runs left to right');
  assert.deepEqual(arch.frames.map(f => f.label), ['App', 'Core']);
  for (let i = 0; i < arch.boxes.length; i++) {
    for (let j = i + 1; j < arch.boxes.length; j++) {
      const a = arch.boxes[i];
      const b = arch.boxes[j];
      const apart = a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y;
      assert.ok(apart, `boxes ${i} and ${j} overlap`);
    }
  }
});

test('svg: themed interactive nodes, an export the sketch sanitizer keeps, escaped text', () => {
  const s = spec({ ...FLOW, title: 'A <b> & "c"', nodes: [...FLOW.nodes.slice(0, 3), { id: 'store', label: '<script>x</script>' }] });
  const d = layoutDiagram(s);
  const live = diagramSvg(s, d, { interactive: true, idPrefix: 'p' });
  assert.ok(live.includes('data-node="0"') && live.includes('role="button"') && live.includes('tabindex="0"'));
  assert.ok(live.includes('fill-shodh-accent-soft'), 'the emphasised node takes the accent');
  assert.equal((live.match(/fill-shodh-accent-soft/g) ?? []).length, 1);
  assert.ok(!live.includes('<script>'));
  assert.ok(live.includes('&lt;script&gt;'));
  const exported = diagramSvg(s, d, { interactive: false, idPrefix: 'p' });
  assert.ok(!exported.includes('class='));
  const clean = sanitizeSvg(exported, { idPrefix: 'x' });
  assert.ok(clean.ok, clean.ok ? '' : clean.error);
  assert.ok(clean.ok && clean.svg.includes('<path'));
  assert.ok(clean.ok && clean.removed.length === 0, clean.ok ? clean.removed.join(',') : '');
});

test('delta: added, removed and changed parts carry a badge and a line style', () => {
  const s = spec(SAMPLES[1]);
  const svg = diagramSvg(s, layoutDiagram(s), { interactive: true, idPrefix: 'q' });
  assert.ok(svg.includes('>+</text>') && svg.includes('>−</text>'));
  assert.ok(svg.includes('fill-shodh-success-soft'));
  assert.ok(svg.includes('stroke-dasharray="5 4"'), 'removed parts are dashed');
  assert.ok(svg.includes('text-decoration="line-through"'));
});

test('helpers: labels wrap to two lines and corners are rounded', () => {
  assert.deepEqual(wrapLabel('Short'), ['Short']);
  const long = wrapLabel('A rather long component name that keeps going on and on');
  assert.equal(long.length, 2);
  assert.ok(long[1].endsWith('…'));
  assert.equal(roundedPath([[0, 0], [0, 40], [40, 40]]), 'M0 0 L0 32 Q0 40 8 40 L40 40');
  const s = spec(FLOW);
  assert.ok(describeNode(s, 'embed').includes('Comes from: ← Split into chunks'));
  assert.ok(describeNode(s, 'embed').includes('Source: [2]'));
});

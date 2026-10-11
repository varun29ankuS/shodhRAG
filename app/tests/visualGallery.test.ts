/**
 * The gallery of generated visuals: block extraction and title inference per
 * kind, capture dedupe (the same normalisation as the backend's content
 * hash), backfill batches, gallery filter/sort, record targets, and refine
 * requests and responses.
 *   node --experimental-strip-types --test app/tests/visualGallery.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import {
  capturable,
  extractVisualBlocks,
  markdownTableRows,
  MAX_BLOCKS_PER_ANSWER,
  normalizeSource,
  visualKey,
} from '../src/features/visuals/extract.ts';
import { backfillBatches, captureBatch, chunks } from '../src/features/visuals/capture.ts';
import {
  applyChange,
  cardFromDetail,
  filterVisuals,
  kindCounts,
  paramsFor,
  paramValuesOf,
  recordTarget,
  sortVisuals,
} from '../src/features/visuals/model.ts';
import type { VisualSummary } from '../src/features/visuals/model.ts';
import { composeRefineRequest, parseRefineResponse } from '../src/features/visuals/refine.ts';

const PLOT = JSON.stringify({ title: 'Trajectory', x: { min: 0, max: 40 }, y: { min: 0, max: 20 }, params: [{ name: 'v0', min: 1, max: 30, value: 20 }], items: [{ type: 'function', expr: 'x' }] });
const SIM = JSON.stringify({ title: 'Pendulum', state: { th: '0.3', w: '0' }, derivatives: { th: 'w', w: '-g*sin(th)' }, view: { x: [-1, 1], y: [-1, 1] }, draw: [{ type: 'circle', at: ['sin(th)', '-cos(th)'] }] });
const CHART = JSON.stringify({ type: 'bar', title: 'Revenue by quarter', xKey: 'q', series: [{ key: 'r', label: 'Revenue' }], data: [{ q: 'Q1', r: 1 }, { q: 'Q2', r: 2 }] });
const SVG = '<svg viewBox="0 0 10 10"><title>Free-body diagram</title><line x1="0" y1="0" x2="5" y2="5"/></svg>';

test('every kind is found, in order, with the renderer’s rules', () => {
  const answer = [
    '## Login',
    '```mermaid',
    'graph TD',
    '  A[Start] --> B[Server]',
    '```',
    '```flowchart',
    'X[Open] --> Y[Close]',
    '```',
    '```chart',
    CHART,
    '```',
    '```svg',
    SVG,
    '```',
    '```plot',
    PLOT,
    '```',
    '```simulation',
    SIM,
    '```',
    'Energy is \\[E = mc^2\\] in general.',
    '',
    '$$',
    'F = m a',
    '$$',
    '',
    '| Quarter | Revenue |',
    '|---|--:|',
    '| Q1 | 10 |',
    '',
    '```python',
    'print("$$x$$")',
    '```',
  ].join('\n');
  const blocks = extractVisualBlocks(answer);
  assert.deepEqual(blocks.map(b => b.kind), ['mermaid', 'mermaid', 'chart', 'svg', 'plot', 'simulation', 'equation', 'table']);
  // A bare ```flowchart gets the header the renderer adds.
  assert.equal(blocks[1].source, 'flowchart TD\nX[Open] --> Y[Close]');
  // \[…\] inside a sentence is inline math in the renderer: only the $$ block is a display equation.
  assert.equal(blocks[6].source, 'F = m a');
  assert.equal(blocks[7].source, '| Quarter | Revenue |\n|---|--:|\n| Q1 | 10 |');
  assert.ok(blocks.every(b => !b.problem));
});

test('a display equation on its own line, written either way, is found', () => {
  const blocks = extractVisualBlocks('Intro\n\n\\[ a^2 + b^2 = c^2 \\]\n\nand\n\n$$x = 1$$\n');
  assert.deepEqual(blocks.map(b => [b.kind, b.source]), [['equation', 'a^2 + b^2 = c^2'], ['equation', 'x = 1']]);
});

test('math delimiters inside code fences are left as written', () => {
  const plot = JSON.stringify({ title: 'Angle \\(\\theta\\)', x: { min: 0, max: 1 }, y: { min: 0, max: 1 }, items: [{ type: 'function', expr: 'x' }] });
  const [block] = extractVisualBlocks(`\`\`\`plot\n${plot}\n\`\`\``);
  assert.equal(block.source, plot);
});

test('titles: own title, then caption, then heading, then first label, then the noun', () => {
  const own = extractVisualBlocks(`# Section\n\`\`\`plot\n${PLOT}\n\`\`\``)[0];
  assert.equal(own.title, 'Trajectory');
  const svgOwn = extractVisualBlocks(`\`\`\`svg\n${SVG}\n\`\`\``)[0];
  assert.equal(svgOwn.title, 'Free-body diagram');
  const mermaidOwn = extractVisualBlocks('```mermaid\npie\n  title Market share\n  "A": 1\n```')[0];
  assert.equal(mermaidOwn.title, 'Market share');

  const captionAbove = extractVisualBlocks('## Results\n\n**Figure 2: Request flow**\n\n```mermaid\ngraph TD\nA-->B\n```')[0];
  assert.equal(captionAbove.title, 'Request flow');
  const colon = extractVisualBlocks('The steps are as follows:\n```mermaid\ngraph TD\nA-->B\n```')[0];
  assert.equal(colon.title, 'The steps are as follows');
  const captionBelow = extractVisualBlocks('| a | b |\n|---|---|\n| 1 | 2 |\n\n*Table 1. Sample sizes*')[0];
  assert.equal(captionBelow.title, 'Sample sizes');

  const heading = extractVisualBlocks('## Energy of a moving body\n\nIt is\n\n$$\nE_k = \\tfrac12 m v^2\n$$')[0];
  assert.equal(heading.title, 'Energy of a moving body');

  const firstLabel = extractVisualBlocks('Some prose.\n\n```mermaid\ngraph TD\n  A[Receive order] --> B[Ship]\n```')[0];
  assert.equal(firstLabel.title, 'Receive order');
  const tableLabel = extractVisualBlocks('Text.\n\n| Name | Age |\n|---|---|\n| A | 3 |')[0];
  assert.equal(tableLabel.title, 'Table: Name, Age');
  const equationLabel = extractVisualBlocks('Text.\n\n$$\nx = y\n$$')[0];
  assert.equal(equationLabel.title, 'Equation x = y');
  const noun = extractVisualBlocks('```svg\n<svg viewBox="0 0 1 1"><rect width="1" height="1"/></svg>\n```')[0];
  assert.equal(noun.title, 'Sketch');
});

test('blocks the renderer would not draw carry a problem and are never captured', () => {
  const blocks = extractVisualBlocks('```chart\n{not json\n```\n```plot\n{"x": 1}\n```\n```svg\n<svg><script>x</script>\n```');
  assert.equal(blocks.length, 3);
  assert.ok(blocks.every(b => typeof b.problem === 'string' && b.problem.length > 0));
  assert.deepEqual(capturable(blocks), []);
  // An unclosed fence (cut-off answer) is not a block, and nothing after it is either.
  assert.deepEqual(extractVisualBlocks('$$\nx\n$$\n```mermaid\ngraph TD\nA-->B').map(b => b.kind), ['equation']);
});

test('capture dedupe uses the backend’s normalisation and hash input', () => {
  assert.equal(normalizeSource('\r\n\ngraph TD  \r\n  A-->B\t\r\n\n'), 'graph TD\n  A-->B');
  assert.equal(visualKey('mermaid', 'graph TD\n  A-->B'), visualKey('mermaid', '\ngraph TD \r\n  A-->B\n\n'));
  assert.notEqual(visualKey('mermaid', 'graph TD'), visualKey('svg', 'graph TD'));
  // The backend's content hash is SHA-256 of this key; same fixed vector as
  // shodh_rag::visuals::tests::content_hash_is_sha256_of_kind_and_source.
  const hash = createHash('sha256').update(visualKey('equation', '\nE = mc^2 \r\n')).digest('hex');
  assert.equal(hash, '1a32250b01d47db2a264b337686ee04f5d98c857e235eff0441bee49fe124dfb');

  const answer = '$$\nE = mc^2\n$$\n\nAgain:\n\n$$\nE = mc^2   \n$$\n\n```mermaid\ngraph TD\nA-->B\n```';
  const kept = capturable(extractVisualBlocks(answer));
  assert.deepEqual(kept.map(b => b.kind), ['equation', 'mermaid']);
  const many = Array.from({ length: MAX_BLOCKS_PER_ANSWER + 5 }, (_, i) => `$$\nx_${i}\n$$`).join('\n\n');
  assert.equal(capturable(extractVisualBlocks(many)).length, MAX_BLOCKS_PER_ANSWER);
  const origin = { conversationId: 'c1', messageId: 'm1', threadId: null, turnId: null };
  assert.equal(captureBatch(origin, 'No visuals here.'), null);
  assert.equal(captureBatch(origin, answer)?.blocks.length, 2);
});

test('backfill covers answers, side answers on messages and on the conversation', () => {
  const sideTurn = (id: string, content: string) => ({ id, role: 'assistant', content, timestamp: '2026-10-01T00:00:00Z' });
  const thread = (id: string, parentMessageId: string | null, turns: unknown[]) => ({
    id,
    anchor: { conversationId: 'c1', parentMessageId, target: { kind: 'equation', label: 'Equation', tex: 'x' } },
    turns,
    createdAt: '2026-10-01T00:00:00Z',
    updatedAt: '2026-10-01T00:00:00Z',
  });
  const conversations = [{
    id: 'c1',
    messages: [
      { id: 'u1', role: 'user' as const, content: '$$\nignored\n$$', timestamp: '' },
      {
        id: 'm1', role: 'assistant' as const, content: '```mermaid\ngraph TD\nA-->B\n```', timestamp: '',
        metadata: { focusThreads: [thread('t1', 'm1', [{ id: 'q', role: 'user', content: 'why', timestamp: '' }, sideTurn('a1', '$$\ny = 2\n$$')])] },
      },
      { id: 'm2', role: 'assistant' as const, content: 'Plain text.', timestamp: '' },
    ],
    focusThreads: [thread('t2', null, [sideTurn('a2', '| a | b |\n|---|---|\n| 1 | 2 |')])],
  }];
  const batches = backfillBatches(conversations);
  assert.deepEqual(batches.map(b => [b.origin.messageId, b.origin.threadId, b.origin.turnId, b.blocks[0].kind]), [
    ['m1', null, null, 'mermaid'],
    ['m1', 't1', 'a1', 'equation'],
    [null, 't2', 'a2', 'table'],
  ]);
  assert.deepEqual(chunks([1, 2, 3, 4, 5], 2), [[1, 2], [3, 4], [5]]);
});

function card(id: string, patch: Partial<VisualSummary> = {}): VisualSummary {
  return {
    id, rootId: id, parentId: null, version: 1, conversationId: 'c1', messageId: 'm1', threadId: null, turnId: null,
    kind: 'mermaid', title: id, source: 'graph TD', params: {}, contentHash: '', pinned: false, note: '', instruction: null,
    createdBy: 'capture', createdAt: '2026-10-01T00:00:00.000Z', updatedAt: '2026-10-01T00:00:00.000Z', versionCount: 1, firstCreatedAt: '2026-10-01T00:00:00.000Z',
    ...patch,
  };
}

test('gallery: pinned first, then most recently changed; kind filter and counts', () => {
  const items = [
    card('old', { updatedAt: '2026-10-01T00:00:00.000Z' }),
    card('new', { updatedAt: '2026-10-03T00:00:00.000Z', kind: 'equation' }),
    card('pinned', { updatedAt: '2026-09-01T00:00:00.000Z', pinned: true }),
  ];
  assert.deepEqual(sortVisuals(items).map(v => v.id), ['pinned', 'new', 'old']);
  assert.deepEqual(filterVisuals(items, 'mermaid').map(v => v.id), ['pinned', 'old']);
  assert.deepEqual(filterVisuals(items, 'all').map(v => v.id), ['pinned', 'new', 'old']);
  assert.deepEqual(kindCounts(items), { mermaid: 2, equation: 1 });

  const unpinned = applyChange(items, { type: 'pinned', rootId: 'pinned', pinned: false, at: '2026-10-04T00:00:00.000Z' });
  assert.deepEqual(unpinned.map(v => v.id), ['pinned', 'new', 'old']);
  assert.equal(unpinned[0].pinned, false);
  const pinnedOld = applyChange(unpinned, { type: 'pinned', rootId: 'old', pinned: true, at: '2026-10-05T00:00:00.000Z' });
  assert.equal(pinnedOld[0].id, 'old');
  assert.deepEqual(applyChange(pinnedOld, { type: 'removed', rootId: 'new' }).map(v => v.id), ['old', 'pinned']);
  const renamed = applyChange(items, { type: 'renamed', rootId: 'old', title: 'Flow', at: '2026-10-06T00:00:00.000Z' });
  assert.equal(renamed[1].title, 'Flow');

  // A refined version replaces its chain's card.
  const detail = {
    visual: { ...card('v2'), rootId: 'old', version: 2, parentId: 'old', createdAt: '2026-10-07T00:00:00.000Z', updatedAt: '2026-10-07T00:00:00.000Z' },
    versions: [
      { id: 'old', version: 1, createdBy: 'capture' as const, instruction: null, createdAt: '2026-10-01T00:00:00.000Z' },
      { id: 'v2', version: 2, createdBy: 'user' as const, instruction: 'bigger', createdAt: '2026-10-07T00:00:00.000Z' },
    ],
  };
  const replaced = applyChange(items, { type: 'replaced', card: cardFromDetail(detail) });
  assert.equal(replaced.length, 3);
  const chain = replaced.find(v => v.rootId === 'old');
  assert.equal(chain?.id, 'v2');
  assert.equal(chain?.versionCount, 2);
  assert.equal(chain?.firstCreatedAt, '2026-10-01T00:00:00.000Z');
});

test('records draw as focus targets with their slider positions', () => {
  const plot = recordTarget({ kind: 'plot', title: 'Trajectory', source: PLOT, params: { values: [{ name: 'v0', value: 12 }, { name: 'bad name', value: 1 }] } });
  assert.deepEqual(plot && plot.kind === 'plot' ? plot.values : null, [{ name: 'v0', value: 12 }]);
  const table = recordTarget({ kind: 'table', title: 'Sizes', source: '| a | b \\| c |\n|---|---|\n| **1** | 2 |', params: {} });
  assert.deepEqual(table && table.kind === 'table' ? table.rows : null, [['a', 'b | c'], ['1', '2']]);
  assert.equal(recordTarget({ kind: 'table', title: 'x', source: 'not a table', params: {} }), null);
  const eq = recordTarget({ kind: 'equation', title: 'Energy', source: 'E = mc^2', params: {} });
  assert.deepEqual(eq, { kind: 'equation', label: 'Energy', tex: 'E = mc^2' });
  assert.deepEqual(paramValuesOf(paramsFor([{ name: 'L', value: 2 }])), [{ name: 'L', value: 2 }]);
  assert.deepEqual(paramsFor([]), {});
  assert.deepEqual(markdownTableRows('| x |\n|---|'), [['x']]);
});

test('refine request carries the spec, sliders and the one-block rule', () => {
  const request = composeRefineRequest({ kind: 'simulation', title: 'Pendulum', source: SIM, values: [{ name: 'L', value: 1.5 }], instruction: 'make the pendulum longer' });
  assert.ok(request.includes('Change requested: make the pendulum longer'));
  assert.ok(request.includes('```simulation\n'));
  assert.ok(request.includes(SIM));
  assert.ok(request.includes('L = 1.5'));
  assert.ok(request.includes('exactly one ```simulation code block'));
  const eq = composeRefineRequest({ kind: 'equation', title: 'E', source: 'E = mc^2', values: [], instruction: 'add units' });
  assert.ok(eq.includes('$$\nE = mc^2\n$$'));
  assert.ok(eq.includes('one display equation'));
  assert.ok(!eq.includes('sliders'));
});

test('refine response: exactly one block of the same kind, else a clear message', () => {
  const previous = 'graph TD\nA-->B';
  const ok = parseRefineResponse('```mermaid\ngraph TD\nA-->B\nB-->C\n```\nAdded step C.', 'mermaid', previous);
  assert.deepEqual(ok, { ok: true, source: 'graph TD\nA-->B\nB-->C', note: 'Added step C.' });

  const none = parseRefineResponse('I cannot do that.', 'mermaid', previous);
  assert.equal(none.ok, false);
  assert.match(none.ok ? '' : none.message, /did not contain a revised diagram/);

  const wrongKind = parseRefineResponse('$$\nx\n$$', 'mermaid', previous);
  assert.match(wrongKind.ok ? '' : wrongKind.message, /contained an equation, not a diagram/);

  const two = parseRefineResponse('```mermaid\ngraph TD\nA-->C\n```\n```mermaid\ngraph TD\nA-->D\n```', 'mermaid', previous);
  assert.match(two.ok ? '' : two.message, /contained 2 diagrams; a refinement must return exactly one/);

  const broken = parseRefineResponse('```plot\n{"x": 1}\n```', 'plot', PLOT);
  assert.match(broken.ok ? '' : broken.message, /cannot be drawn/);

  const same = parseRefineResponse('```mermaid\ngraph TD \nA-->B\n```', 'mermaid', previous);
  assert.match(same.ok ? '' : same.message, /unchanged/);

  const table = parseRefineResponse('| a | b |\n|---|---|\n| 1 | 3 |', 'table', '| a | b |\n|---|---|\n| 1 | 2 |');
  assert.equal(table.ok, true);
});

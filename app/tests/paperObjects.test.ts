/**
 * Figures from the PDF: the paper_objects answer, ```figure blocks, how a
 * block finds its figure, the crop rectangle on the page, and the focus
 * target of a figure.
 *   node --experimental-strip-types --test app/tests/paperObjects.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseFigureBlock, readBox, readPaperParts, resolveFigure } from '../src/features/research/paperObjects.ts';
import { boxToRect, cropPlan, rectToBox } from '../src/features/research/snippetGeometry.ts';
import { equationTarget, figureTarget, paperOf } from '../src/features/focus/targets.ts';
import { readTarget } from '../src/features/focus/threadStore.ts';
import { buildContextBlock } from '../src/features/focus/contextBlock.ts';

const PARTS = {
  filePath: 'C:\\Papers\\delta.pdf',
  fileName: 'delta.pdf',
  pages: [{ number: 3, width: 612, height: 792 }],
  figures: [
    {
      id: 'fig-2',
      number: '2',
      label: 'Figure 2',
      caption: 'Figure 2: Throughput of the chunkwise form.',
      page: 3,
      bbox: { x0: 69, y0: 397, x1: 543, y1: 603 },
      captionBbox: { x0: 72, y0: 400, x1: 540, y1: 412 },
      regionFound: true,
      mentions: ['As Figure 2 shows, the chunkwise form is faster.'],
    },
    { id: 'broken', page: 0, bbox: { x0: 0, y0: 0, x1: 1, y1: 1 } },
  ],
  equations: [
    { id: 'eq-1', number: '1', text: 'St = St−1 + βt…', latex: 'S_t = S_{t-1}', origin: 'source', sourceFile: 'C:/Papers/delta.tex', page: 4, bbox: { x0: 150, y0: 600, x1: 460, y1: 620 } },
    { id: 'eq-p5-1', number: null, text: 'ψ', latex: '\\psi', origin: 'weird', page: null, bbox: null },
  ],
  texSource: 'C:/Papers/delta.tex',
};

test('the paper_objects answer is read and bad entries are dropped', () => {
  const parts = readPaperParts(PARTS);
  assert.ok(parts);
  if (!parts) return;
  assert.equal(parts.figures.length, 1);
  assert.equal(parts.figures[0].mentions.length, 1);
  assert.equal(parts.equations.length, 2);
  assert.equal(parts.equations[0].origin, 'source');
  assert.equal(parts.equations[1].origin, 'reconstructed');
  assert.equal(readPaperParts({}), null);
  assert.deepEqual(readBox([10, 50, 5, 20]), { x0: 5, y0: 20, x1: 10, y1: 50 });
  assert.equal(readBox([1, 1, 1.5, 9]), null);
  assert.equal(readBox({ x0: 'a' }), null);
});

test('a figure block names the paper and the figure id, or a page and box', () => {
  const byId = parseFigureBlock('{"paper": "C:/Papers/delta.pdf", "figureId": "FIG-2", "page": 3, "bbox": [69, 397, 543, 603], "caption": "Figure 2: Throughput."}');
  assert.ok(byId.ok);
  if (!byId.ok) return;
  assert.equal(byId.spec.figureId, 'fig-2');
  assert.deepEqual(byId.spec.bbox, { x0: 69, y0: 397, x1: 543, y1: 603 });
  const byPlace = parseFigureBlock('{"paper": "delta.pdf", "page": 3, "bbox": {"x0": 1, "y0": 2, "x1": 300, "y1": 200}}');
  assert.ok(byPlace.ok && byPlace.spec.figureId === null);
  assert.equal(parseFigureBlock('{"figureId": "fig-2"}').ok, false);
  assert.equal(parseFigureBlock('{"paper": "a.pdf"}').ok, false);
  assert.equal(parseFigureBlock('{"paper": "a.pdf", "figureId": "../../etc"}').ok, false);
  assert.equal(parseFigureBlock('not json').ok, false);
});

test('a block shows the paper’s current figure, or the place it carries when the id is gone', () => {
  const parts = readPaperParts(PARTS);
  if (!parts) throw new Error('fixture');
  const spec = parseFigureBlock('{"paper": "delta.pdf", "figureId": "fig-2", "page": 3, "bbox": [1, 1, 50, 50], "caption": "old"}');
  if (!spec.ok) throw new Error('fixture');
  const current = resolveFigure(spec.spec, parts.figures);
  assert.deepEqual(current?.bbox, { x0: 69, y0: 397, x1: 543, y1: 603 });
  assert.equal(current?.label, 'Figure 2');
  const stale = parseFigureBlock('{"paper": "delta.pdf", "figureId": "fig-9", "page": 3, "bbox": [1, 1, 50, 50], "caption": "Figure 9: Gone."}');
  if (!stale.ok) throw new Error('fixture');
  const fallback = resolveFigure(stale.spec, parts.figures);
  assert.deepEqual(fallback?.bbox, { x0: 1, y0: 1, x1: 50, y1: 50 });
  assert.equal(fallback?.label, 'Figure 9');
  const idOnly = parseFigureBlock('{"paper": "delta.pdf", "figureId": "fig-9"}');
  if (!idOnly.ok) throw new Error('fixture');
  assert.equal(resolveFigure(idOnly.spec, parts.figures), null);
});

test('the crop rectangle of a figure follows the page view box, also with an offset origin', () => {
  const box = { x0: 69, y0: 397, x1: 543, y1: 603 };
  const plain = boxToRect(box, [0, 0, 612, 792]);
  assert.deepEqual(plain, { x: 69, y: 189, width: 474, height: 206 });
  // A cropped page (view box not at the origin): the rectangle is relative to the view box.
  const shifted = boxToRect(box, [36, 18, 576, 774]);
  assert.deepEqual(shifted, { x: 33, y: 171, width: 474, height: 206 });
  assert.deepEqual(rectToBox(shifted, [36, 18, 576, 774]), box);
  // Drawn sharp but within the canvas limits.
  const plan = cropPlan(plain, [0, 0, 612, 792], [1, 0, 0, -1, 0, 792], { targetScale: 4, maxSide: 1200, maxPixels: 1_000_000 });
  assert.ok(plan);
  if (!plan) return;
  assert.ok(plan.width <= 1200 && plan.height <= 1200 && plan.width * plan.height <= 1_000_000);
  assert.ok(Math.abs(plan.width / plan.height - 474 / 206) < 0.02);
});

test('a figure opens with its caption and the text near it, and shows its box in the paper', () => {
  const target = figureTarget({
    filePath: 'C:\\Papers\\delta.pdf',
    page: 3,
    bbox: { x0: 69, y0: 397, x1: 543, y1: 603 },
    figureId: 'fig-2',
    caption: 'Figure 2: Throughput of the chunkwise form.',
    mentions: ['As Figure 2 shows, the chunkwise form is faster.'],
  });
  assert.ok(target && target.kind === 'figure');
  if (!target || target.kind !== 'figure') return;
  assert.equal(target.label, 'Figure 2 · delta.pdf');
  assert.equal(target.fileName, 'delta.pdf');
  assert.deepEqual(readTarget(JSON.parse(JSON.stringify(target))), target);
  assert.deepEqual(paperOf(target)?.regions, [{ page: 3, x0: 69, y0: 397, x1: 543, y1: 603 }]);
  const block = buildContextBlock(target, { selection: 'chunkwise' });
  assert.match(block, /figure from delta\.pdf page 3, caption/);
  assert.match(block, /text that refers to it/);
  assert.match(block, /selected text/);
  assert.equal(figureTarget({ filePath: 'a.pdf', page: 0, bbox: { x0: 0, y0: 0, x1: 1, y1: 1 }, figureId: null, caption: '' }), null);
  assert.equal(readTarget({ ...target, bbox: { x0: 5, y0: 5, x1: 1, y1: 1 } }), null);
});

test('an equation keeps the meanings of the symbols it contains', () => {
  const target = equationTarget('y = \\Phi_q x', [
    { symbol: '\\Phi_q', meaning: 'feature map', definedAt: { paper: 'delta.pdf', page: 3 } },
    { symbol: '\\gamma', meaning: 'unused', definedAt: null },
  ]);
  assert.ok(target.kind === 'equation');
  if (target.kind !== 'equation') return;
  assert.deepEqual(target.symbols?.map(s => s.symbol), ['\\Phi_q']);
  assert.deepEqual(readTarget(JSON.parse(JSON.stringify(target))), target);
  assert.match(buildContextBlock(target), /\\Phi_q: feature map \(defined in delta\.pdf, page 3\)/);
  assert.equal('symbols' in equationTarget('x = 1'), false);
});

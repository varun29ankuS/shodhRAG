/**
 * Pan, zoom, pinch and fit of the citation graph drawing.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/graphViewport.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  GRAPH_MAX_ZOOM,
  GRAPH_MIN_ZOOM,
  IDENTITY,
  clampGraphView,
  clientToViewBox,
  fitGraph,
  isDrag,
  nodeBounds,
  pinchGraph,
  toGraph,
  toScreen,
  zoomGraphAt,
  zoomGraphStep,
} from '../src/features/research/graphViewport.ts';

const viewport = { width: 880, height: 520 };

function close(actual: number, expected: number, message?: string) {
  assert.ok(Math.abs(actual - expected) < 1e-6, `${message ?? ''} ${actual} ≈ ${expected}`);
}

test('bounds cover every node with its radius and skip invalid points', () => {
  assert.equal(nodeBounds([]), null);
  assert.deepEqual(
    nodeBounds([{ x: 10, y: 20 }, { x: 100, y: -5 }, { x: Number.NaN, y: 0 }], i => (i === 0 ? 5 : 2)),
    { minX: 5, minY: -7, maxX: 102, maxY: 25 },
  );
});

test('fitting centres the nodes and never enlarges past 2x', () => {
  const bounds = { minX: 0, minY: 0, maxX: 1600, maxY: 400 };
  const view = fitGraph(bounds, viewport);
  const centre = toScreen(view, { x: 800, y: 200 });
  close(centre.x, 440);
  close(centre.y, 260);
  // Width decides: (880 - 64) / 1600.
  close(view.scale, 816 / 1600);
  const tiny = fitGraph({ minX: 0, minY: 0, maxX: 10, maxY: 10 }, viewport);
  assert.equal(tiny.scale, 2);
  assert.deepEqual(fitGraph(null, viewport), IDENTITY);
});

test('zoom keeps the point under the cursor fixed and is clamped', () => {
  const view = { scale: 1, x: 30, y: -10 };
  const cursor = { x: 200, y: 150 };
  const under = toGraph(view, cursor);
  const zoomed = zoomGraphAt(view, 2.5, cursor);
  const after = toScreen(zoomed, under);
  close(after.x, cursor.x);
  close(after.y, cursor.y);
  assert.equal(zoomGraphAt(view, 1000, cursor).scale, GRAPH_MAX_ZOOM);
  assert.equal(zoomGraphAt(view, 0.0001, cursor).scale, GRAPH_MIN_ZOOM);
  const stepped = zoomGraphStep(IDENTITY, 1, viewport);
  assert.ok(stepped.scale > 1);
  const back = zoomGraphStep(stepped, -1, viewport);
  close(back.scale, 1);
  close(back.x, 0);
});

test('panning can never push the whole graph out of view', () => {
  const bounds = { minX: 0, minY: 0, maxX: 400, maxY: 300 };
  const far = clampGraphView({ scale: 1, x: 5000, y: -5000 }, bounds, viewport, 48);
  // The left edge of the nodes may sit at most 48 units from the right edge.
  close(0 * far.scale + far.x, viewport.width - 48);
  close(300 * far.scale + far.y, 48);
  const inside = { scale: 1, x: 100, y: 50 };
  assert.deepEqual(clampGraphView(inside, bounds, viewport, 48), inside);
  assert.equal(clampGraphView({ scale: 99, x: 0, y: 0 }, null, viewport).scale, GRAPH_MAX_ZOOM);
});

test('a pinch scales by finger spread around their midpoint', () => {
  const start = { scale: 1, x: 0, y: 0 };
  const view = pinchGraph(start, [{ x: 100, y: 100 }, { x: 200, y: 100 }], [{ x: 50, y: 100 }, { x: 250, y: 100 }]);
  close(view.scale, 2);
  // The graph point under the starting midpoint (150, 100) stays under the new midpoint.
  const p = toScreen(view, { x: 150, y: 100 });
  close(p.x, 150);
  close(p.y, 100);
  const moved = pinchGraph(start, [{ x: 0, y: 0 }, { x: 10, y: 0 }], [{ x: 20, y: 30 }, { x: 30, y: 30 }]);
  close(moved.scale, 1);
  close(moved.x, 20);
  close(moved.y, 30);
});

test('presses become drags past the threshold; client pixels map to the viewBox', () => {
  assert.equal(isDrag({ x: 0, y: 0 }, { x: 2, y: 2 }), false);
  assert.equal(isDrag({ x: 0, y: 0 }, { x: 4, y: 0 }), true);
  const p = clientToViewBox({ x: 300, y: 160 }, { left: 100, top: 60, width: 440, height: 260 }, viewport);
  close(p.x, 400);
  close(p.y, 200);
});

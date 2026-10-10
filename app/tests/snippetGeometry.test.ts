/**
 * Snippet geometry: text inside a rectangle, selection/marquee → rectangle,
 * crop math for a sharp render, and rectangle ↔ region conversion.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/snippetGeometry.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  boxToRect,
  cropPlan,
  cssToSnippetRect,
  insideBox,
  invertTransform,
  rectToBox,
  rectToRegion,
  regionToRect,
  textBoxes,
  textInRect,
  textItemBox,
} from '../src/features/research/snippetGeometry.ts';
import type { TextBox } from '../src/features/research/snippetGeometry.ts';

const LETTER = [0, 0, 612, 792];

function item(str: string, x: number, y: number, width: number, height = 10) {
  return { str, transform: [height, 0, 0, height, x, y], width, height };
}

test('a pdf.js text item box spans its width along the run and its height above the baseline', () => {
  assert.deepEqual(textItemBox(item('Hello', 72, 700, 30)), { x0: 72, y0: 700, x1: 102, y1: 710 });
  // Text rotated 90° counter-clockwise: the run goes up the page.
  const rotated = textItemBox({ str: 'Up', transform: [0, 10, -10, 0, 50, 100], width: 20, height: 10 });
  assert.deepEqual(rotated, { x0: 40, y0: 100, x1: 50, y1: 120 });
  assert.equal(textItemBox({ str: 'x', transform: [1, 0, 0, 1, NaN, 0], width: 1, height: 1 }), null);
  // Marked-content markers have no `str` and are skipped.
  assert.equal(textBoxes([{ type: 'beginMarkedContent' }, item('a', 1, 1, 5)]).length, 1);
});

test('an item counts when at least half of its area is inside; zero-area items by their centre', () => {
  const rect = { x0: 0, y0: 0, x1: 100, y1: 100 };
  assert.equal(insideBox({ x0: 50, y0: 10, x1: 150, y1: 20 }, rect), true); // exactly half
  assert.equal(insideBox({ x0: 51, y0: 10, x1: 151, y1: 20 }, rect), false); // just under half
  assert.equal(insideBox({ x0: 10, y0: 10, x1: 10, y1: 20 }, rect), true);
  assert.equal(insideBox({ x0: 110, y0: 10, x1: 110, y1: 20 }, rect), false);
});

test('text in a rectangle reads lines top to bottom, items left to right', () => {
  const items: TextBox[] = textBoxes([
    item('world', 140, 700, 40),
    item('Hello', 100, 700, 35),
    item('second   line', 100, 686, 70),
    item('outside', 400, 700, 40),
    // Superscript on the first line, slightly raised: still the same line.
    item('2', 182, 704, 4, 6),
    item('   ', 120, 672, 10),
    item('below', 100, 600, 30),
  ]);
  // Rectangle from (90, 80) top-left, 120 x 40: covers y 672..712 in PDF space.
  const text = textInRect(items, { x: 90, y: 80, width: 120, height: 40 }, LETTER);
  assert.equal(text, 'Hello world 2\nsecond line');
  assert.equal(textInRect(items, { x: 300, y: 300, width: 10, height: 10 }, LETTER), '');
});

test('a view box with an origin offset shifts the rectangle', () => {
  const view = [10, 20, 622, 812];
  assert.deepEqual(rectToBox({ x: 5, y: 12, width: 100, height: 50 }, view), { x0: 15, x1: 115, y1: 800, y0: 750 });
  assert.deepEqual(boxToRect({ x0: 115, y0: 800, x1: 15, y1: 750 }, view), { x: 5, y: 12, width: 100, height: 50 });
});

test('rectangles and indexer regions convert both ways', () => {
  const rect = { x: 72, y: 92, width: 228, height: 100 };
  const region = rectToRegion(rect, 3, LETTER);
  assert.deepEqual(region, { page: 3, x0: 72, y0: 600, x1: 300, y1: 700 });
  assert.deepEqual(regionToRect(region, LETTER), rect);
});

test('selection rects in CSS pixels become one rectangle in PDF points, clipped to the page', () => {
  // Scale 1.5, no rotation: x' = 1.5x, y' = 1.5(792 - y).
  const transform = [1.5, 0, 0, -1.5, 0, 1188];
  const rect = cssToSnippetRect(
    [
      { left: 108, top: 138, width: 150, height: 15 },
      { left: 108, top: 156, width: 300, height: 15 },
    ],
    transform,
    LETTER,
  );
  assert.deepEqual(rect, { x: 72, y: 92, width: 200, height: 22 });
  // Dragged past the page edge: clipped.
  const clipped = cssToSnippetRect([{ left: -30, top: -30, width: 90, height: 90 }], transform, LETTER);
  assert.deepEqual(clipped, { x: 0, y: 0, width: 40, height: 40 });
  // Too small, or nothing usable.
  assert.equal(cssToSnippetRect([{ left: 10, top: 10, width: 3, height: 3 }], transform, LETTER), null);
  assert.equal(cssToSnippetRect([], transform, LETTER), null);
  assert.equal(invertTransform([0, 0, 0, 0, 1, 1]), null);
});

test('crop math renders at the target scale and stays within the canvas caps', () => {
  const unit = [1, 0, 0, -1, 0, 792];
  const plan = cropPlan({ x: 72, y: 92, width: 200, height: 100 }, LETTER, unit);
  assert.ok(plan);
  assert.equal(plan.scale, 4);
  assert.equal(plan.width, 800);
  assert.equal(plan.height, 400);
  // The rectangle's top-left corner lands on the canvas origin.
  assert.equal(plan.offsetX, -72 * 4);
  assert.equal(plan.offsetY, -92 * 4);

  // A whole page at scale 4 would be 2448 x 3168 = 7.8M px: under both caps.
  const page = cropPlan({ x: 0, y: 0, width: 612, height: 792 }, LETTER, unit);
  assert.equal(page?.scale, 4);

  // A huge region is limited by the longest side.
  const big = cropPlan({ x: 0, y: 0, width: 2000, height: 100 }, [0, 0, 2000, 792], unit, { targetScale: 4, maxSide: 4096, maxPixels: 16_000_000 });
  assert.ok(big && big.width <= 4096 && big.height <= 4096);
  assert.ok(Math.abs(big.scale - 4096 / 2000) < 1e-9);

  // And by the pixel budget.
  const square = cropPlan({ x: 0, y: 0, width: 1500, height: 1500 }, [0, 0, 1500, 1500], [1, 0, 0, -1, 0, 1500], { targetScale: 4, maxSide: 100_000, maxPixels: 16_000_000 });
  assert.ok(square && square.width * square.height <= 16_000_000 + 2 * 4000 + 1);

  // Rotated page (90°): the crop follows the rotated viewport.
  const rotated = cropPlan({ x: 0, y: 0, width: 100, height: 50 }, LETTER, [0, 1, 1, 0, 0, 0]);
  assert.ok(rotated);
  assert.equal(rotated.width, 200); // the 50-point side runs along x after rotation
  assert.equal(rotated.height, 400);
  assert.equal(cropPlan({ x: 0, y: 0, width: 0, height: 10 }, LETTER, unit), null);
});

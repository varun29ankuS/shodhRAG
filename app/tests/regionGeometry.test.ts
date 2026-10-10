/**
 * Layout-box highlighting: parsing the boxes the indexer stores and mapping
 * them onto a rendered page. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/regionGeometry.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { firstRegionPage, parseRegions, regionToCssRect, regionsOnPages } from '../src/features/ask/viewer/regionGeometry.ts';
import { passagesFromDetail } from '../src/features/agent/reducer.ts';
import { toSearchHits } from '../src/features/ask/searchResults.ts';

test('regions parse from chunk metadata JSON and step details, dropping invalid boxes', () => {
  const stored = '[{"page":3,"x0":72,"y0":400,"x1":300,"y1":700},{"page":0,"x0":1,"y0":1,"x1":2,"y1":2},{"page":4,"x0":5,"y0":9,"x1":5,"y1":20}]';
  assert.deepEqual(parseRegions(stored), [{ page: 3, x0: 72, y0: 400, x1: 300, y1: 700 }]);
  assert.deepEqual(parseRegions([{ page: 2, x0: 10, y0: 20, x1: 30, y1: 40 }]), [{ page: 2, x0: 10, y0: 20, x1: 30, y1: 40 }]);
  assert.equal(parseRegions('not json'), null);
  assert.equal(parseRegions('[]'), null);
  assert.equal(parseRegions(null), null);
  assert.equal(parseRegions([{ page: 1, x0: 'a', y0: 0, x1: 1, y1: 1 }]), null);
});

test('regions are limited to the cited pages and the first page leads', () => {
  const regions = [
    { page: 6, x0: 0, y0: 0, x1: 10, y1: 10 },
    { page: 5, x0: 0, y0: 0, x1: 10, y1: 10 },
    { page: 9, x0: 0, y0: 0, x1: 10, y1: 10 },
  ];
  const onCited = regionsOnPages(regions, { start: 5, end: 6 });
  assert.deepEqual(onCited.map(r => r.page), [6, 5]);
  assert.equal(firstRegionPage(onCited), 5);
  assert.equal(regionsOnPages(regions, null).length, 3);
  assert.equal(firstRegionPage([]), null);
});

test('a box in PDF points maps to CSS pixels through the viewport transform', () => {
  // pdf.js viewport for a 612 x 792 page at scale 1.5, no rotation:
  // x' = 1.5 x, y' = 1.5 (792 - y).
  const transform = [1.5, 0, 0, -1.5, 0, 1188];
  const rect = regionToCssRect({ page: 1, x0: 72, y0: 600, x1: 300, y1: 700 }, transform);
  assert.deepEqual(rect, { left: 108, top: 138, width: 342, height: 150 });

  // Rotated 90°: pdf.js maps (x, y) to (s*y, s*x) after the view box shift.
  const rotated = [0, 1, 1, 0, 0, 0];
  assert.deepEqual(regionToCssRect({ page: 1, x0: 10, y0: 20, x1: 30, y1: 60 }, rotated), {
    left: 20,
    top: 10,
    width: 40,
    height: 20,
  });
});

test('passages from a step detail carry section and regions; search hits read them from metadata', () => {
  const [passage] = passagesFromDetail({
    passages: [
      {
        n: 1,
        file: 'deltanet.pdf',
        path: 'c:/papers/deltanet.pdf',
        page: '5-6',
        heading: '3.2 Chunkwise form',
        section: '3 Method > 3.2 Chunkwise form',
        score: 0.7,
        text: 'The chunkwise form',
        regions: [{ page: 5, x0: 72, y0: 400, x1: 300, y1: 700 }],
      },
    ],
  });
  assert.equal(passage.section, '3 Method > 3.2 Chunkwise form');
  assert.deepEqual(passage.regions, [{ page: 5, x0: 72, y0: 400, x1: 300, y1: 700 }]);

  const [legacy] = passagesFromDetail({ passages: [{ n: 2, path: 'c:/a.pdf', text: 'x' }] });
  assert.equal(legacy.regions, null);

  const [hit] = toSearchHits([
    {
      sourceFile: 'c:/papers/deltanet.pdf',
      text: 'The chunkwise form',
      pageNumber: 5,
      citation: { title: 'DeltaNet', pageNumbers: '5-6' },
      metadata: { section_path: '3 Method', bboxes: '[{"page":5,"x0":72,"y0":400,"x1":300,"y1":700}]' },
    },
  ]);
  assert.deepEqual(hit.page, { start: 5, end: 6 });
  assert.equal(hit.section, '3 Method');
  assert.equal(hit.regions?.length, 1);
});

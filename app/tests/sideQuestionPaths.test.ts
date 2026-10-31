/**
 * Side questions never carry a local path: every object a side question can be about
 * names its file, and a local image's address is left out. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/sideQuestionPaths.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { buildContextBlock, contextLabel, publicAddress, targetDigest } from '../src/features/focus/contextBlock.ts';
import type { FocusTarget } from '../src/features/focus/focusTypes.ts';

const WIN = 'C:\\Users\\alice-private\\Papers\\delta.pdf';
const POSIX = '/Users/alice-private/Papers/delta.pdf';
const PATHISH = /alice-private|[A-Za-z]:[\\/]|\/Users\/|\/home\//;

function targets(path: string): FocusTarget[] {
  return [
    {
      kind: 'source',
      label: 'Passage 3',
      hit: {
        number: 3,
        sourceFile: path,
        fileName: '',
        title: '',
        text: 'Notice is sixty days.',
        snippet: 'Notice is sixty days.',
        score: 1,
        page: { start: 2, end: 2 },
        lineRange: null,
        url: null,
      },
    },
    { kind: 'snippet', label: 'Snippet', snippetId: 's1', filePath: path, fileName: '', page: 2, rect: { x: 1, y: 1, width: 10, height: 10 }, text: 'A region.' },
    {
      kind: 'figure',
      label: 'Figure 2',
      filePath: path,
      fileName: '',
      page: 3,
      bbox: { x0: 1, y0: 1, x1: 9, y1: 9 },
      figureId: 'fig-2',
      caption: 'Figure 2: Throughput.',
      nearby: 'As Figure 2 shows.',
    },
    {
      kind: 'selection',
      label: 'Selection',
      text: 'sixty days',
      paragraph: 'Notice is sixty days.',
      origin: 'document',
      document: { sourceFile: path, fileName: '', page: 2 },
    },
    { kind: 'image', label: 'Image', src: `asset://localhost/${encodeURIComponent(path)}`, alt: 'A chart' },
    { kind: 'image', label: 'Image', src: `http://asset.localhost/${path.replace(/\\/g, '/')}`, alt: 'A chart' },
    { kind: 'image', label: 'Image', src: `file:///${path.replace(/\\/g, '/')}`, alt: 'A chart' },
  ] as FocusTarget[];
}

test('no side-question context holds a local path, whatever the platform', () => {
  for (const path of [WIN, POSIX]) {
    for (const target of targets(path)) {
      for (const text of [buildContextBlock(target), targetDigest(target), contextLabel(target)]) {
        assert.doesNotMatch(text, PATHISH, `${target.kind}: ${text}`);
      }
    }
  }
  // The file is still named.
  assert.match(buildContextBlock(targets(WIN)[1]), /delta\.pdf/);
});

test('only a public web address of an image is kept', () => {
  assert.equal(publicAddress('https://example.org/figure.png'), 'https://example.org/figure.png');
  for (const local of ['asset://localhost/C%3A%2Fx.png', 'http://asset.localhost/C:/x.png', 'file:///C:/x.png', 'http://localhost:1420/x.png', 'data:image/png;base64,AAA', null, '']) {
    assert.equal(publicAddress(local), null, String(local));
  }
});

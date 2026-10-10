/**
 * Export → PDF: the document sent to the print view (sources, numbering,
 * titles, file names) and the print layout rules (hidden controls, no page
 * breaks inside figures, equations or tables).
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/printModel.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  PRINT_HIDDEN_SELECTORS,
  PRINT_UNBREAKABLE_SELECTORS,
  answerTitle,
  buildPrintDocument,
  hitsFromSources,
  printFileName,
  printSources,
  printStylesheet,
  sourceAnchor,
  sourceLine,
  stripLeadingTitle,
  unresolvedCitations,
  visualPrintMarkdown,
} from '../src/features/print/printModel.ts';
import { writesPdfFiles } from '../src/features/print/printPlatform.ts';
import type { SearchHit } from '../src/features/ask/types.ts';

function hit(number: number, over: Partial<SearchHit> = {}): SearchHit {
  return {
    number,
    sourceFile: 'C:/docs/lease.pdf',
    fileName: 'lease.pdf',
    title: 'Lease',
    text: 'text',
    snippet: 'text',
    score: 1,
    page: { start: 4, end: 4 },
    lineRange: null,
    url: null,
    ...over,
  };
}

const hits = [
  hit(1),
  hit(2, { sourceFile: 'https://blog.example/r', fileName: 'Rust blog', url: 'https://blog.example/r', page: null }),
  hit(3, { sourceFile: 'C:/papers/deltanet.pdf', fileName: 'deltanet.pdf', page: { start: 5, end: 6 } }),
  hit(4, { sourceFile: 'C:/notes/plain.txt', fileName: 'plain.txt', page: null, section: '2 Method' }),
  hit(9, { fileName: 'unused.pdf' }),
];

test('sources list the cited passages once, in number order, with file and page or URL', () => {
  const markdown = 'Notice is 60 days [3][1]. Async closures shipped [2]. Again [1]. Method [4]. Unknown [7].';
  const sources = printSources(markdown, hits);
  assert.deepEqual(sources.map(s => s.n), [1, 2, 3, 4], 'uncited [9] left out, [1] once');
  assert.deepEqual(sources[0], { n: 1, title: 'lease.pdf', location: 'p. 4', url: null });
  assert.deepEqual(sources[1], { n: 2, title: 'Rust blog', location: null, url: 'https://blog.example/r' });
  assert.equal(sources[2].location, 'pp. 5–6');
  assert.equal(sources[3].location, '2 Method', 'unpaged files fall back to the section');
  assert.equal(sourceLine(sources[0]), 'lease.pdf, p. 4');
  assert.deepEqual(unresolvedCitations(markdown, sources), [7]);
  // Citations inside code are not sources.
  assert.deepEqual(printSources('```\nx[1]\n```\nNothing cited.', hits), []);
});

test('the printed renderer resolves exactly the listed numbers and links them to the list', () => {
  const doc = buildPrintDocument({ title: 'Notice periods', createdAt: '2026-10-04T10:00:00Z', markdown: 'A [1] and B [2].', hits });
  const resolved = hitsFromSources(doc.sources);
  assert.deepEqual(resolved.map(h => h.number), [1, 2]);
  assert.equal(resolved[1].url, 'https://blog.example/r');
  assert.equal(sourceAnchor(2), 'print-source-2');
  assert.equal(doc.createdAt, '2026-10-04T10:00:00.000Z');
  assert.equal(doc.subtitle, null);
});

test('titles come from the question or the answer, and a repeated heading is not printed twice', () => {
  assert.equal(answerTitle('# Notice periods\n\nThe notice is 60 days [1].'), 'Notice periods');
  assert.equal(answerTitle('The notice period is 60 days [1]. It renews yearly.'), 'The notice period is 60 days.');
  assert.equal(answerTitle('```chart\n{}\n```\n\nRevenue grew.'), 'Revenue grew.');
  assert.equal(answerTitle(''), 'Shodh answer');
  assert.ok(answerTitle('x'.repeat(200)).endsWith('…'));
  assert.equal(stripLeadingTitle('# Notice periods\nBody', 'notice periods'), 'Body');
  assert.equal(stripLeadingTitle('# Other\nBody', 'Notice periods'), '# Other\nBody');
  const doc = buildPrintDocument({ title: 'Notice periods', markdown: '# Notice periods\nBody [1]', hits });
  assert.equal(doc.markdown, 'Body [1]');
  assert.equal(doc.sources.length, 1);
});

test('file names are safe and end in .pdf', () => {
  assert.equal(printFileName('Q3: results / summary?'), 'Q3 results summary.pdf');
  assert.equal(printFileName('   '), 'Shodh export.pdf');
  assert.equal(printFileName('notes...'), 'notes.pdf');
});

test('gallery visuals print as the block the renderer draws', () => {
  assert.equal(visualPrintMarkdown({ kind: 'mermaid', source: 'graph TD; A-->B' }), '```mermaid\ngraph TD; A-->B\n```');
  assert.equal(visualPrintMarkdown({ kind: 'equation', source: 'E = mc^2', note: 'Energy.' }), '$$\nE = mc^2\n$$\n\nEnergy.');
  assert.equal(visualPrintMarkdown({ kind: 'table', source: '| a |\n|---|\n| 1 |' }), '| a |\n|---|\n| 1 |');
});

test('the print stylesheet hides controls and keeps figures, equations and tables whole', () => {
  const css = printStylesheet();
  for (const selector of ['button', 'input', '[role="slider"]', '[role="toolbar"]', '[data-print-hide]']) {
    assert.ok((PRINT_HIDDEN_SELECTORS as readonly string[]).includes(selector), selector);
    assert.ok(css.includes(`.print-view ${selector}`), selector);
  }
  assert.match(css, /\{ display: none !important; \}/);
  for (const selector of ['figure', 'table', 'svg', '.katex-display', 'pre']) {
    assert.ok((PRINT_UNBREAKABLE_SELECTORS as readonly string[]).includes(selector), selector);
  }
  assert.match(css, /break-inside: avoid/);
  assert.match(css, /overflow: visible !important/);
});

test('only Windows writes PDF files directly', () => {
  assert.equal(writesPdfFiles('Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Edg/129.0'), true);
  assert.equal(writesPdfFiles('Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15'), false);
  assert.equal(writesPdfFiles('Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15'), false);
});

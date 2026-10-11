/**
 * Symbol meanings: ```symbols blocks, matching symbols in LaTeX, the KaTeX
 * annotation (trust locked to one attribute) and removing it again.
 *   node --experimental-strip-types --test app/tests/visualSymbols.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import katex from 'katex';
import { unified } from 'unified';
import remarkParse from 'remark-parse';
import remarkMath from 'remark-math';
import remarkRehype from 'remark-rehype';
import rehypeKatex from 'rehype-katex';
import rehypeFocusEquations, { FOCUS_EQUATION_TAG } from '../src/features/focus/rehypeFocusEquations.ts';
import rehypeSymbols from '../src/features/ask/visual/rehypeSymbols.ts';
import {
  annotateTex,
  katexTrust,
  parseSymbolsBlock,
  readSymbolNotes,
  stripSymbolWrappers,
  symbolTokens,
  symbolsInMessage,
  type SymbolNote,
} from '../src/features/ask/visual/symbols.ts';

const note = (symbol: string, meaning = `meaning of ${symbol}`): SymbolNote => ({ symbol, meaning, definedAt: null });

test('a symbols block is a JSON list of symbol and meaning, both key spellings of definedAt accepted', () => {
  const r = parseSymbolsBlock(
    JSON.stringify([
      { symbol: '\\Phi_q', meaning: 'query feature map', defined_at: { paper: 'delta.pdf', page: 3 } },
      { symbol: '\\beta_t', meaning: 'writing strength', definedAt: { paper: 'delta.pdf' } },
      { symbol: 'x' },
      { symbol: '\\beta_t', meaning: 'duplicate' },
      'junk',
    ]),
  );
  assert.ok(r.ok);
  if (!r.ok) return;
  assert.deepEqual(r.symbols.map(s => s.symbol), ['\\Phi_q', '\\beta_t']);
  assert.deepEqual(r.symbols[0].definedAt, { paper: 'delta.pdf', page: 3 });
  assert.deepEqual(r.symbols[1].definedAt, { paper: 'delta.pdf', page: null });
  assert.equal(parseSymbolsBlock('{').ok, false);
  assert.equal(parseSymbolsBlock('{"a": 1}').ok, false);
  assert.equal(parseSymbolsBlock('[{"symbol": "x"}]').ok, false);
  assert.equal(readSymbolNotes({ symbols: [{ symbol: 'k', meaning: 'key' }] }).length, 1);
});

test('symbols of a message come from all of its symbols blocks', () => {
  const content = [
    '$$y = \\Phi_q x$$',
    '```symbols',
    '[{"symbol": "\\\\Phi_q", "meaning": "feature map"}]',
    '```',
    'More text.',
    '```symbols',
    '[{"symbol": "x", "meaning": "input"}, {"symbol": "\\\\Phi_q", "meaning": "ignored, first wins"}]',
    '```',
  ].join('\n');
  const symbols = symbolsInMessage(content);
  assert.deepEqual(symbols.map(s => [s.symbol, s.meaning]), [['\\Phi_q', 'feature map'], ['x', 'input']]);
});

test('spellings of one symbol compare equal', () => {
  assert.deepEqual(symbolTokens('\\Phi_{q}'), symbolTokens('\\Phi_q'));
  assert.deepEqual(symbolTokens('\\mathbf{x}'), symbolTokens('\\mathbf x'));
  assert.notDeepEqual(symbolTokens('\\Phi_q'), symbolTokens('\\Phi_k'));
});

test('occurrences are wrapped at token boundaries, longest symbol first', () => {
  const symbols = [note('\\Phi'), note('\\Phi_q'), note('x')];
  const r = annotateTex('y = \\Phi_{q} x + \\Phi(x)', symbols);
  assert.equal(r.tex, 'y = {\\htmlData{shodh-sym=1}{\\Phi_{q}}} {\\htmlData{shodh-sym=2}{x}} + {\\htmlData{shodh-sym=0}{\\Phi}}({\\htmlData{shodh-sym=2}{x}})');
  assert.deepEqual(r.matched, [0, 1, 2]);
  assert.equal(stripSymbolWrappers(r.tex), 'y = \\Phi_{q} x + \\Phi(x)');
});

test('subscripts of other symbols, styled letters, names and structure are left alone', () => {
  const q = annotateTex('\\Phi_q + q', [note('q')]);
  assert.equal(q.tex, '\\Phi_q + {\\htmlData{shodh-sym=0}{q}}');
  // A bold x is a different symbol from x.
  assert.equal(annotateTex('\\mathbf{x} + x', [note('x')]).tex, '\\mathbf{x} + {\\htmlData{shodh-sym=0}{x}}');
  assert.equal(annotateTex('\\mathbf{x}', [note('\\mathbf x')]).tex, '{\\htmlData{shodh-sym=0}{\\mathbf{x}}}');
  // Inside \text and environment names nothing matches.
  assert.deepEqual(annotateTex('\\text{a} + \\operatorname{a}(t)', [note('a')]).matched, []);
  assert.deepEqual(annotateTex('\\begin{aligned} a &= b \\\\ c &= d \\end{aligned}', [note('a &= b'), note('aligned')]).matched, []);
  assert.equal(annotateTex('\\begin{aligned} a &= b \\end{aligned}', [note('a')]).tex, '\\begin{aligned} {\\htmlData{shodh-sym=0}{a}} &= b \\end{aligned}');
  // Already annotated LaTeX is never annotated twice.
  assert.deepEqual(annotateTex('{\\htmlData{shodh-sym=0}{x}}', [note('x')]).matched, []);
});

test('a bare superscript argument stays legal LaTeX after wrapping', () => {
  const r = annotateTex('x^\\alpha', [note('\\alpha')]);
  // \alpha after ^ is the exponent of x, part of another symbol: not wrapped.
  assert.deepEqual(r.matched, []);
  const s = annotateTex('e^{-\\lambda t}', [note('\\lambda')]);
  assert.equal(s.tex, 'e^{-{\\htmlData{shodh-sym=0}{\\lambda}} t}');
  assert.doesNotThrow(() => katex.renderToString(s.tex, { throwOnError: true, trust: katexTrust }));
});

test('KaTeX trusts exactly the symbol attribute and nothing else', () => {
  const html = katex.renderToString(annotateTex('\\Phi_q x', [note('\\Phi_q')]).tex, { throwOnError: true, trust: katexTrust });
  assert.match(html, /data-shodh-sym="0"/);
  assert.equal(katexTrust({ command: '\\htmlData', attributes: { 'data-shodh-sym': '3' } }), true);
  assert.equal(katexTrust({ command: '\\htmlData', attributes: { 'data-shodh-sym': '3', onclick: 'x' } }), false);
  assert.equal(katexTrust({ command: '\\htmlData', attributes: { 'data-shodh-sym': 'javascript:1' } }), false);
  assert.equal(katexTrust({ command: '\\href', attributes: {} }), false);
  assert.equal(katexTrust({ command: '\\includegraphics' }), false);
  const hostile = katex.renderToString('\\href{javascript:alert(1)}{x} \\htmlData{foo=bar}{y}', { throwOnError: false, trust: katexTrust });
  // Refused commands are shown as red source text, never as a link or an attribute.
  assert.doesNotMatch(hostile, /href="|data-foo=/);
});

interface Node {
  type: string;
  tagName?: string;
  properties?: Record<string, unknown>;
  children?: Node[];
}

function collect(node: Node, test: (n: Node) => boolean, out: Node[] = []): Node[] {
  if (test(node)) out.push(node);
  for (const child of node.children ?? []) collect(child, test, out);
  return out;
}

test('in an answer, marked symbols reach the HTML while the equation keeps its own LaTeX', async () => {
  const symbols = [note('\\Phi_q'), note('x')];
  const annotate = (tex: string) => annotateTex(tex, symbols).tex;
  const processor = unified()
    .use(remarkParse)
    .use(remarkMath)
    .use(remarkRehype)
    .use(rehypeSymbols, { annotate })
    .use(rehypeKatex, { trust: katexTrust })
    .use(rehypeFocusEquations);
  const tree = (await processor.run(processor.parse('Inline $x$ and\n\n$$\ny = \\Phi_q x\n$$\n'))) as unknown as Node;
  const marked = collect(tree, n => n.type === 'element' && n.properties?.dataShodhSym !== undefined);
  assert.equal(marked.length, 3);
  const [equation] = collect(tree, n => n.type === 'element' && n.tagName === FOCUS_EQUATION_TAG);
  assert.equal(equation.properties?.dataTex, 'y = \\Phi_q x');
});

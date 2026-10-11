/**
 * Focus pop-out: display equations are wrapped with their TeX source.
 *   node --experimental-strip-types --test app/tests/focusEquations.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { unified } from 'unified';
import remarkParse from 'remark-parse';
import remarkMath from 'remark-math';
import remarkRehype from 'remark-rehype';
import rehypeKatex from 'rehype-katex';
import rehypeFocusEquations, { FOCUS_EQUATION_TAG, annotationTex } from '../src/features/focus/rehypeFocusEquations.ts';

interface Node {
  type: string;
  tagName?: string;
  properties?: Record<string, unknown>;
  children?: Node[];
}

function collect(node: Node, tag: string, out: Node[] = []): Node[] {
  if (node.type === 'element' && node.tagName === tag) out.push(node);
  for (const child of node.children ?? []) collect(child, tag, out);
  return out;
}

async function render(markdown: string): Promise<Node> {
  const processor = unified().use(remarkParse).use(remarkMath).use(remarkRehype).use(rehypeKatex).use(rehypeFocusEquations);
  return (await processor.run(processor.parse(markdown))) as unknown as Node;
}

test('display math from the real pipeline is wrapped with its TeX', async () => {
  const tree = await render('Intro\n\n$$\nE = mc^2\n$$\n\nand inline $a+b$.');
  const wrapped = collect(tree, FOCUS_EQUATION_TAG);
  assert.equal(wrapped.length, 1);
  assert.equal(wrapped[0].properties?.dataTex, 'E = mc^2');
});

test('inline math is left alone and the plugin is idempotent', async () => {
  const tree = await render('Only inline $x^2$ here.');
  assert.equal(collect(tree, FOCUS_EQUATION_TAG).length, 0);
  const twice = await render('$$\n\\frac{1}{2}\n$$');
  rehypeFocusEquations()(twice as never);
  assert.equal(collect(twice, FOCUS_EQUATION_TAG).length, 1);
});

test('annotation lookup', () => {
  const node = {
    type: 'element',
    tagName: 'span',
    children: [{ type: 'element', tagName: 'annotation', properties: { encoding: 'application/x-tex' }, children: [{ type: 'text', value: ' x ' }] }],
  };
  assert.equal(annotationTex(node), 'x');
  assert.equal(annotationTex({ type: 'element', tagName: 'span', children: [] }), null);
});

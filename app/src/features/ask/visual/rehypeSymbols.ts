/**
 * rehype plugin, run before rehype-katex: rewrites the LaTeX of every math
 * element (`code.language-math`, inline and display) through `annotate`, so
 * the symbols of the answer's ```symbols blocks come out of KaTeX as
 * elements carrying `data-shodh-sym`.
 *
 * Pure module (no runtime imports); `annotate` is supplied by the caller.
 */

interface HastNode {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
}

export interface RehypeSymbolsOptions {
  annotate: (tex: string, display: boolean) => string;
}

function classList(node: HastNode): string[] {
  const value = node.properties?.className;
  if (Array.isArray(value)) return value.map(String);
  if (typeof value === 'string') return value.split(/\s+/);
  return [];
}

function textOf(node: HastNode): string {
  if (node.type === 'text') return node.value ?? '';
  return (node.children ?? []).map(textOf).join('');
}

function visit(node: HastNode, annotate: RehypeSymbolsOptions['annotate']): void {
  if (node.type === 'element' && node.tagName === 'code') {
    const classes = classList(node);
    if (classes.includes('language-math')) {
      const tex = textOf(node);
      const next = annotate(tex, classes.includes('math-display'));
      if (next !== tex) node.children = [{ type: 'text', value: next }];
      return;
    }
  }
  for (const child of node.children ?? []) visit(child, annotate);
}

export default function rehypeSymbols(options: RehypeSymbolsOptions) {
  return (tree: HastNode) => {
    if (options?.annotate) visit(tree, options.annotate);
  };
}

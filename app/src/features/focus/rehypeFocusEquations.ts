/**
 * rehype plugin, run after rehype-katex: wraps every display equation
 * (`span.katex-display`) in a `focus-equation` element that carries the
 * LaTeX source (from KaTeX's MathML annotation) as `data-tex`, so the
 * renderer can give the equation a focus affordance and re-render it large.
 *
 * Pure module (no runtime imports).
 */

interface HastNode {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
}

export const FOCUS_EQUATION_TAG = 'focus-equation';

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

/** The TeX source KaTeX stored in its MathML `annotation`, if present. */
export function annotationTex(node: HastNode): string | null {
  if (node.type === 'element' && node.tagName === 'annotation' && node.properties?.encoding === 'application/x-tex') {
    return textOf(node).trim();
  }
  for (const child of node.children ?? []) {
    const found = annotationTex(child);
    if (found !== null) return found;
  }
  return null;
}

function wrap(parent: HastNode): void {
  const children = parent.children;
  if (!children) return;
  for (let i = 0; i < children.length; i++) {
    const child = children[i];
    if (child.type !== 'element') continue;
    if (child.tagName === FOCUS_EQUATION_TAG) continue;
    if (child.tagName === 'span' && classList(child).includes('katex-display')) {
      const tex = annotationTex(child);
      if (tex) {
        children[i] = { type: 'element', tagName: FOCUS_EQUATION_TAG, properties: { dataTex: tex }, children: [child] };
      }
      continue;
    }
    wrap(child);
  }
}

export default function rehypeFocusEquations() {
  return (tree: HastNode) => {
    wrap(tree);
  };
}

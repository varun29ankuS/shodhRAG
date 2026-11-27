/**
 * The phone's UI text is short, as the desktop's is (app/tests/copyLength.test.ts): a label
 * or one short line. This fails on any text of 110 characters or more drawn in the phone's
 * JSX, so paragraphs cannot creep back, and on a composer placeholder too long for the field
 * on a phone (it was cut off: "Connect to your computer t…").
 */
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

const MAX_UI_TEXT = 110;
/** The longest placeholder the ask field shows whole on a 360dp-wide phone. */
const MAX_PLACEHOLDER = 24;

const root = resolve(process.cwd(), 'src');
function tsxFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) out.push(...tsxFiles(path));
    else if (name.endsWith('.tsx')) out.push(path);
  }
  return out;
}

const isProse = (value: string) => /\s\w+\s\w+\s/.test(value);

function paragraphs(file: string, text: string): string[] {
  const sf = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const out: string[] = [];
  const visit = (node: ts.Node, inJsx: boolean) => {
    const jsx = inJsx || ts.isJsxElement(node) || ts.isJsxSelfClosingElement(node) || ts.isJsxFragment(node);
    let value: string | null = null;
    if (node.kind === ts.SyntaxKind.JsxText) value = node.getText(sf).replace(/\s+/g, ' ').trim();
    else if (jsx && (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node))) value = node.text;
    if (value && value.length >= MAX_UI_TEXT && isProse(value)) out.push(`${file}: ${value.slice(0, 60)}…`);
    ts.forEachChild(node, child => visit(child, jsx));
  };
  visit(sf, false);
  return out;
}

describe('copy', () => {
  it('has no paragraph of UI text', () => {
    const found = tsxFiles(root).flatMap(file => paragraphs(relative(root, file), readFileSync(file, 'utf8')));
    expect(found).toEqual([]);
    const long = 'This sentence is long enough to count as a paragraph of user interface text, which is what this check exists to catch.';
    expect(paragraphs('x.tsx', `<p>${long}</p>`)).toHaveLength(1);
    expect(paragraphs('x.tsx', '<p>One short line.</p>')).toHaveLength(0);
  });

  it('keeps the ask fields\' placeholders whole on a phone', () => {
    for (const file of ['screens/Home.tsx', 'screens/Ask.tsx']) {
      const text = readFileSync(join(root, file), 'utf8');
      const line = text.split('\n').find(l => /'Not connected'/.test(l) && /'Sent when it wakes'/.test(l)) ?? '';
      const words = [...line.matchAll(/'([^']+)'/g)].map(m => m[1]);
      expect(words.length, file).toBeGreaterThanOrEqual(3);
      for (const w of words) expect(w.length, `${file}: "${w}"`).toBeLessThanOrEqual(MAX_PLACEHOLDER);
    }
  });

  it('says connected once: the bar\'s status, not a line under the greeting too', () => {
    const home = readFileSync(join(root, 'screens/Home.tsx'), 'utf8');
    expect(home).toMatch(/\{status\.state !== 'connected' && \(\s*<p className=\{`line/);
  });
});

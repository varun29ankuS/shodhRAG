/**
 * UI text is short: a label of a few words or one short line (UI audit rule, 2d). A
 * paragraph's detail lives behind an info button (`InfoPopover`) when it matters (privacy,
 * cost, what leaves the computer), or is gone. This fails on any text of
 * `MAX_UI_TEXT` characters or more drawn in JSX outside an info button, so paragraphs
 * cannot creep back. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/copyLength.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { relative } from 'node:path';
import ts from 'typescript';
import { files, srcDir } from './jsxScan.ts';

/** The length from which UI text is a paragraph. */
const MAX_UI_TEXT = 110;

/** Words, not a class list or a code sample: at least three words separated by spaces. */
const isProse = (value: string) => /\s\w+\s\w+\s/.test(value) && !/^[\w-]+(\s[\w-:[\]/.%]+)+$/.test(value);

/** Paragraph-length text drawn in `text`'s JSX outside an info button. */
function paragraphs(file: string, text: string): string[] {
  const sf = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const out: string[] = [];
  const visit = (node: ts.Node, inJsx: boolean, inInfo: boolean) => {
    const jsx = inJsx || ts.isJsxElement(node) || ts.isJsxSelfClosingElement(node) || ts.isJsxFragment(node);
    const info = inInfo || (ts.isJsxElement(node) && node.openingElement.tagName.getText(sf) === 'InfoPopover');
    let value: string | null = null;
    if (node.kind === ts.SyntaxKind.JsxText) value = node.getText(sf).replace(/\s+/g, ' ').trim();
    else if (jsx && (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node))) value = node.text;
    if (value && !info && value.length >= MAX_UI_TEXT && isProse(value)) {
      out.push(`${file}:${sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1}: ${value.slice(0, 60)}…`);
    }
    ts.forEachChild(node, child => visit(child, jsx, info));
  };
  visit(sf, false, false);
  return out;
}

test('no paragraph of UI text outside an info button', () => {
  const found = files(srcDir, /\.tsx$/).flatMap(file =>
    paragraphs(relative(srcDir, file).replace(/\\/g, '/'), readFileSync(file, 'utf8')),
  );
  assert.deepEqual(found, []);
});

test('the check finds a paragraph in text, a prop or a branch, and lets one behind an info button be', () => {
  const long = 'This sentence is long enough to count as a paragraph of user interface text, which is the thing the check exists to catch.';
  assert.ok(long.length >= MAX_UI_TEXT);
  assert.equal(paragraphs('x.tsx', `<p>${long}</p>`).length, 1);
  assert.equal(paragraphs('x.tsx', `<Row detail="${long}" />`).length, 1);
  assert.equal(paragraphs('x.tsx', `<p>{on ? '${long}' : 'Off'}</p>`).length, 1);
  assert.equal(paragraphs('x.tsx', `<p>Short. <InfoPopover label="About" title="About">${long}</InfoPopover></p>`).length, 0);
  assert.equal(paragraphs('x.tsx', `<p>One short line.</p>`).length, 0);
});

/**
 * The conversation's widths (features/ask/wideBlock.ts) and the stylesheet rules that draw
 * them. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/wideBlock.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  COLUMN_GUTTER_PX,
  TEXT_COLUMN_PX,
  WIDE_BLOCK_CLASS,
  WIDE_BLOCK_HOST_CLASS,
  WIDE_BLOCK_MAX_PX,
  textWidth,
  wideBlockOverhang,
  wideBlockWidth,
} from '../src/features/ask/wideBlock.ts';
import { HTML_PREVIEW_HEIGHT_CLASS } from '../src/features/ask/htmlPreview.ts';

const css = readFileSync(new URL('../src/index.css', import.meta.url), 'utf8');
const source = (path: string) => readFileSync(new URL(`../src/${path}`, import.meta.url), 'utf8');

test('a wide block is as wide as the conversation allows, up to its maximum', () => {
  // A wide window: the full 1100, reaching past the text equally on both sides.
  assert.equal(wideBlockWidth(1600), WIDE_BLOCK_MAX_PX);
  assert.equal(textWidth(1600), TEXT_COLUMN_PX - COLUMN_GUTTER_PX * 2);
  assert.equal(wideBlockOverhang(1600), (WIDE_BLOCK_MAX_PX - (TEXT_COLUMN_PX - COLUMN_GUTTER_PX * 2)) / 2);
  // In between: the conversation's width less the gutters.
  assert.equal(wideBlockWidth(900), 900 - COLUMN_GUTTER_PX * 2);
  assert.ok(wideBlockOverhang(900) > 0);
});

test('a wide block is never narrower than the text, and never wider than the conversation', () => {
  for (const host of [0, 40, 320, 500, TEXT_COLUMN_PX, TEXT_COLUMN_PX + 1, 900, 1156, 1157, 2400]) {
    assert.ok(wideBlockWidth(host) >= textWidth(host), `at ${host}`);
    assert.ok(wideBlockWidth(host) <= Math.max(0, host - COLUMN_GUTTER_PX * 2), `at ${host}`);
    assert.ok(wideBlockOverhang(host) >= 0, `at ${host}`);
  }
  // At the column's own width and below, it is exactly the text's width.
  assert.equal(wideBlockWidth(TEXT_COLUMN_PX), textWidth(TEXT_COLUMN_PX));
  assert.equal(wideBlockWidth(500), textWidth(500));
});

test('the stylesheet draws the same numbers', () => {
  const gutters = COLUMN_GUTTER_PX * 2;
  const width = `min(${WIDE_BLOCK_MAX_PX}px, calc(100cqw - ${gutters}px))`;
  assert.ok(css.includes(`.${WIDE_BLOCK_HOST_CLASS} {\n  container: ${WIDE_BLOCK_HOST_CLASS} / inline-size;`), 'the scroller is the size container');
  assert.ok(css.includes(`@container ${WIDE_BLOCK_HOST_CLASS} (min-width: 0px) {\n  .${WIDE_BLOCK_CLASS} {\n    width: ${width};\n    margin-inline: calc((100% - ${width}) / 2);`), 'the wide block rule');
});

test('the conversation hosts wide blocks, and a previewed page is one', () => {
  assert.match(source('features/ask/AskView.tsx'), /'flex-1 overflow-y-auto scrollbar-thin', WIDE_BLOCK_HOST_CLASS/);
  assert.match(source('features/ask/ConversationLog.tsx'), /maxWidth: TEXT_COLUMN_PX/);
  assert.match(source('features/ask/MessageContentRenderer.tsx'), /isHtmlLanguage\(lang\) && WIDE_BLOCK_CLASS/);
});

test('a previewed page is no longer held to 360px', () => {
  const block = source('features/ask/HtmlPreviewBlock.tsx');
  assert.doesNotMatch(block, /h-\[360px\]/);
  assert.match(block, /HTML_PREVIEW_HEIGHT_CLASS/);
  assert.ok(css.includes(`.${HTML_PREVIEW_HEIGHT_CLASS} {\n  height: clamp(360px, 60vh, 720px);`));
});

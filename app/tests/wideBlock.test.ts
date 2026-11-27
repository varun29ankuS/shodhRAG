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
  PLAN_DOCK_INSET_PX,
  PLAN_DOCK_PX,
  TEXT_COLUMN_PX,
  WIDE_BLOCK_CLASS,
  WIDE_BLOCK_HOST_CLASS,
  WIDE_BLOCK_MAX_PX,
  planDocks,
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

test('the text reads in an 800px column; wide blocks reach 1100px', () => {
  assert.equal(TEXT_COLUMN_PX, 800);
  assert.equal(WIDE_BLOCK_MAX_PX, 1100);
});

test('the conversation hosts wide blocks: an answer\'s code, tables, charts and diagrams', () => {
  const ask = source('features/ask/AskView.tsx');
  assert.match(ask, /'flex-1 overflow-y-auto scrollbar-thin', WIDE_BLOCK_HOST_CLASS/);
  // The answer's text is drawn wide; a notice or a question is not.
  assert.match(ask, /onOpenArtifact=\{onOpenArtifact\}\s+wide\s+\/>/);
  assert.doesNotMatch(source('features/ask/UserText.tsx'), /\bwide\b/);
  assert.doesNotMatch(source('features/focus/SideThread.tsx'), /<MessageContentRenderer[^>]*\bwide\b/);
  // An agent answer's own text, not a working note, an earlier draft or the floating chat's.
  assert.match(source('features/agent/Transcript.tsx'), /compact=\{compact \|\| superseded \|\| note\}\s+wide=\{!\(compact \|\| superseded \|\| note\)\}/);
  assert.match(source('features/ask/ConversationLog.tsx'), /maxWidth: TEXT_COLUMN_PX/);

  const renderer = source('features/ask/MessageContentRenderer.tsx');
  // Code (every language, a previewed page with it).
  assert.match(renderer, /'my-4 rounded-xl overflow-hidden border border-shodh-border bg-shodh-surface', wide && WIDE_BLOCK_CLASS/);
  // Tables, in the text and as artifacts; charts as artifacts.
  assert.match(renderer, /<FocusFrame noun="table" getTarget=\{tableFromElement\} doubleClick=\{false\} placement="above" className=\{cn\('my-4', wide && WIDE_BLOCK_CLASS\)\}>/);
  assert.match(renderer, /noun="chart".*className=\{cn\(wide && WIDE_BLOCK_CLASS\)\}>/);
  assert.match(renderer, /key=\{artifact\.id\} noun="table"[^>]*className=\{cn\(wide && WIDE_BLOCK_CLASS\)\}>/);
  // Charts and diagrams in the text; math, derivations and symbol tables read with it.
  const wideVisuals = /const WIDE_VISUAL_LANGUAGES = new Set\(\[([^\]]*)\]\)/.exec(renderer);
  assert.ok(wideVisuals, 'the wide visuals are listed');
  assert.deepEqual([...wideVisuals[1].matchAll(/'([^']+)'/g)].map(m => m[1]).sort(), ['chart', 'diagram', 'figure', 'plot', 'simulation', 'svg']);
  assert.match(renderer, /if \(WIDE_VISUAL_LANGUAGES\.has\(lang\) \|\| isMermaidLanguage\(lang\)\) return wide \? <div className=\{WIDE_BLOCK_CLASS\}>/);
  // The memos that draw blocks change with it.
  assert.match(renderer, /\}\), \[isDark, theme, print, wide\]\);/);
  assert.match(renderer, /\}, \[codeComponents, processLatest, wide\]\);/);
});

test('every conversation reads in the same column', () => {
  const connector = source('features/connectors/ConnectorConversationView.tsx');
  assert.doesNotMatch(connector, /max-w-\[700px\]/);
  assert.equal(connector.match(/style=\{\{ maxWidth: TEXT_COLUMN_PX \}\}/g)?.length, 3);
});

test('the plan docks beside the conversation only where it covers none of it', () => {
  // The docked plan is the width planDocks counts with.
  assert.match(source('features/agent/PlanPanel.tsx'), new RegExp(`variant === 'docked' \\? 'w-\\[${PLAN_DOCK_PX}px\\]'`));
  for (let host = 0; host <= 3000; host += 4) {
    if (!planDocks(host)) continue;
    // Room right of the widest block for the plan and its inset.
    const right = (host - wideBlockWidth(host)) / 2;
    assert.ok(right >= PLAN_DOCK_PX + PLAN_DOCK_INSET_PX, `at ${host}`);
  }
  // Clear of the rail in the gutter, and docked in a wide enough window.
  assert.ok(PLAN_DOCK_INSET_PX > COLUMN_GUTTER_PX);
  assert.ok(!planDocks(1440 - 248));
  assert.ok(planDocks(2400));
  const ask = source('features/ask/AskView.tsx');
  assert.match(ask, /const planDocked = planDocks\(hostBox\.width\);/);
  assert.match(ask, /inlinePlan=\{!planDocked \|\| message\.transcript !== latest\}/);
  assert.match(ask, /\{planDocked && latest\?\.plan && \(/);
});

test('a previewed page is no longer held to 360px', () => {
  const block = source('features/ask/HtmlPreviewBlock.tsx');
  assert.doesNotMatch(block, /h-\[360px\]/);
  assert.match(block, /HTML_PREVIEW_HEIGHT_CLASS/);
  assert.ok(css.includes(`.${HTML_PREVIEW_HEIGHT_CLASS} {\n  height: clamp(360px, 60vh, 720px);`));
});

test('a table\'s buttons sit in a row above it, never over a header cell', () => {
  const renderer = source('features/ask/MessageContentRenderer.tsx');
  const tables = renderer.match(/<FocusFrame[^>]*noun="table"[^>]*>/g) ?? [];
  assert.equal(tables.length, 2);
  for (const frame of tables) assert.match(frame, /placement="above"/);
  const frame = source('features/focus/FocusFrame.tsx');
  assert.match(frame, /placement === 'over' \? 'absolute top-2 right-2 z-10 inline-flex' : 'flex justify-end mb-1\.5'/);
  assert.match(frame, /\{placement === 'above' && children\}/);
});

test('a code block paints no band behind its lines', () => {
  const view = source('components/CodeHighlightView.tsx');
  assert.match(view, /\[CODE_SELECTOR\]: \{ \.\.\.theme\[CODE_SELECTOR\], background: 'transparent' \}/);
  assert.match(view, /style: dark \? DARK_THEME : LIGHT_THEME,/);
});

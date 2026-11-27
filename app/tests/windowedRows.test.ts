/**
 * The app's long lists are windowed (hooks/useWindowedList.ts, lib/windowedRows.ts): only
 * the rows in view are drawn, the tab stop and focused rows stay drawn, and a row brought
 * into view by the keyboard or the assistant is drawn before it is focused or found.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/windowedRows.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { drawnIndexes, gridColumns, gridRows, rowOfCard } from '../src/lib/windowedRows.ts';
import { source, withoutComments } from './jsxScan.ts';

test('the drawn rows: the visible range and the kept ones, once each, in order', () => {
  assert.deepEqual(drawnIndexes([4, 5, 6], [], 100), [4, 5, 6]);
  assert.deepEqual(drawnIndexes([4, 5, 6], [90, 5, null, undefined], 100), [4, 5, 6, 90]);
  // A kept row that is gone (a filter narrowed the list) is not drawn.
  assert.deepEqual(drawnIndexes([0, 1], [-1, 7], 3), [0, 1]);
  assert.deepEqual(drawnIndexes([], [], 0), []);
});

test('a grid has as many columns as cards of the least width fit, at least one', () => {
  // auto-fill, minmax(280px, 1fr) with a 12px gap.
  assert.equal(gridColumns(0, 280, 12), 1);
  assert.equal(gridColumns(279, 280, 12), 1);
  assert.equal(gridColumns(280, 280, 12), 1);
  assert.equal(gridColumns(571, 280, 12), 1);
  assert.equal(gridColumns(572, 280, 12), 2);
  assert.equal(gridColumns(864, 280, 12), 3);
});

test('a grid\'s cards fill rows in order; each card is in one row', () => {
  assert.deepEqual(gridRows(0, 3), []);
  assert.deepEqual(gridRows(7, 3), [{ first: 0, last: 2 }, { first: 3, last: 5 }, { first: 6, last: 6 }]);
  assert.deepEqual(gridRows(2, 0), [{ first: 0, last: 0 }, { first: 1, last: 1 }]);
  for (let card = 0; card < 7; card++) {
    const row = rowOfCard(card, 3);
    const { first, last } = gridRows(7, 3)[row];
    assert.ok(card >= first && card <= last, `card ${card}`);
  }
});

/** Each long list, and what it must keep doing now that it is windowed. */
const LISTS: Record<string, RegExp[]> = {
  'features/library/FileBrowser.tsx': [
    // Every file is listed; no cap with a "Showing N of M" line.
    /const shown = entries;/,
    /keep: \[activeIndex\],/,
    /void revealRow\(index\)\.then\(row => \{/,
  ],
  'components/CalendarTodoPanel.tsx': [
    /keep: \[\s*filteredTasks\.findIndex\(t => t\.id === tabStopId\),/,
    // The assistant's task is drawn before its highlight looks for it.
    /setRowRequest\(prev => \(\{ id: task\.id, focus: false,/,
    /void revealRow\(index, rowRequest\.focus \? 'auto' : 'center'\)/,
  ],
  'features/inbox/InboxButton.tsx': [
    /keep: \[selected\[tab\]\],/,
    /void revealRow\(selected\[tab\]\)\.then\(\(\) => rowRefs\.current\.get\(id\)\?\.focus\(\)\);/,
  ],
  'features/artifacts/ArtifactsView.tsx': [
    /const cardRows = useMemo\(\(\) => gridRows\(shown\.length, columns\), \[shown\.length, columns\]\);/,
    // Tab goes on past the last card drawn.
    /tabPastWindow\(e, shown\.length, card => rowOfCard\(card, columns\), revealCardRow\)/,
  ],
  'features/visuals/VisualGallery.tsx': [
    /const cardRows = useMemo\(\(\) => gridRows\(shown\.length, columns\), \[shown\.length, columns\]\);/,
    /tabPastWindow\(e, shown\.length, card => rowOfCard\(card, columns\), revealCardRow\)/,
    // A card the assistant changed is drawn and scrolled to before its highlight looks for it.
    /useNavigationTarget\('visual', revealVisual, \{ consume: false \}\);/,
  ],
};

test('the long lists (Library, Tasks, Inbox, Artifacts) draw a window of rows', () => {
  for (const [file, rules] of Object.entries(LISTS)) {
    const text = withoutComments(source(file));
    assert.match(text, /useWindowedList\(\{/, `${file} is windowed`);
    // Rows are placed by the window and measured once drawn.
    assert.match(text, /ref=\{windowed\.virtualizer\.measureElement\}|measure=\{windowed\.virtualizer\.measureElement\}/, file);
    assert.match(text, /style=\{\{ height: windowed\.total \}\}/, file);
    assert.doesNotMatch(text, /MAX_ROWS\b/, file);
    for (const rule of rules) assert.match(text, rule, `${file}: ${rule}`);
  }
  // Activity's event table has its own window (fixed-height rows, one expanded).
  assert.match(source('features/audit/AuditTable.tsx'), /Windowed event table/);
});

test('a revealed row is drawn before it is returned', () => {
  const hook = withoutComments(source('hooks/useWindowedList.ts'));
  assert.match(hook, /virtualizer\.scrollToIndex\(index, \{ align \}\);/);
  assert.match(hook, /list\.querySelector<HTMLElement>\(`\[data-index="\$\{index\}"\]`\)/);
  assert.match(hook, /drawnIndexes\(defaultRangeExtractor\(range\), keep, range\.count\)/);
});

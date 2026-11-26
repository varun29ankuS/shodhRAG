/**
 * The keyboard path: Tab from the top of the window through the shell, Ask, the message
 * box, the model menu and Settings, with focus shown at every stop. The tests have no DOM,
 * so the path is read from the sources: nothing sets a positive `tabIndex`, so the order
 * elements are written in is the order Tab visits them; every stop on the path either
 * draws a focus style or keeps the browser's outline; and the menus move by one function,
 * walked here. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/keyboardPath.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { nextMenuIndex } from '../src/lib/menuKeys.ts';
import { COMPOSER_TEST_IDS } from '../src/features/agent/composerRow.ts';
import {
  FOCUS_STYLE,
  NATIVE_STOPS,
  NO_OUTLINE,
  files,
  lineOf,
  openingTags,
  resolvedClasses,
  showsFocus,
  skipped,
  source,
  srcDir,
  where,
  withoutComments,
} from './jsxScan.ts';

const views = files(srcDir, /\.tsx$/).map(file => ({ file: where(file), text: readFileSync(file, 'utf8') }));

/** `markers` appear in `text` in this order, each after the one before. */
function inOrder(text: string, markers: readonly string[], file: string): void {
  let from = 0;
  for (const marker of markers) {
    const at = text.indexOf(marker, from);
    assert.ok(at >= 0, `${file}: "${marker}" is missing, or comes before the stop it should follow`);
    from = at + marker.length;
  }
}

// ── Tab order is source order ──────────────────────────────────────

test('nothing sets a positive tabIndex, so Tab follows the order on screen', () => {
  const positive = /tabIndex=(?:\{\s*[1-9]|"[1-9])/;
  assert.ok(positive.test('tabIndex={2}') && positive.test('tabIndex="1"'));
  assert.ok(!positive.test('tabIndex={0}') && !positive.test('tabIndex={-1}'));
  assert.deepEqual(views.filter(v => positive.test(v.text)).map(v => v.file), []);
});

/** A row whose click a button inside it repeats: the file, and that button. */
const ROW_WITH_ITS_BUTTON: Record<string, RegExp> = {
  'src/features/audit/AuditTable.tsx': /aria-label=\{`\$\{expanded \? 'Hide' : 'Show'\} details of event/,
};

test('nothing is clickable by mouse only', () => {
  const mouseOnly: string[] = [];
  for (const { file, text } of views) {
    for (const { at, tag } of openingTags(text, ['div', 'span', 'li', 'p', 'td', 'tr', 'section', 'article', 'img', 'g', 'svg'])) {
      if (!/\sonClick=/.test(tag)) continue;
      // Something a key reaches: a tab stop with a key handler of its own.
      const keyed = /tabIndex=/.test(tag) && /onKeyDown=/.test(tag);
      // An item of a list, tree or grid: its owner takes the keys and moves among the items.
      const item = /role=(?:"|\{[^}]*')(?:option|treeitem)/.test(tag);
      // A row that opens on click, with a button inside it that does the same by key.
      const rowButton = ROW_WITH_ITS_BUTTON[file]?.test(text) ?? false;
      // A click-away backdrop (Escape closes what it is behind) is hidden from the path.
      const backdrop = /aria-hidden="true"/.test(tag);
      // A click that only passes along: focusing the field inside, or stopping a bubble.
      const passes = /e\.stopPropagation\(\)|\.focus\(\)|e\.target === e\.currentTarget/.test(tag);
      if (!keyed && !item && !rowButton && !backdrop && !passes) mouseOnly.push(`${file}:${lineOf(text, at)}`);
    }
  }
  assert.deepEqual(mouseOnly, []);
});

// ── The shell ──────────────────────────────────────────────────────

test('the shell: skip link, then the sidebar, then the view', () => {
  const app = withoutComments(source('App-SplitView.tsx'));
  inOrder(app, ['href="#main-content"', '<Sidebar', '<main', 'id="main-content"'], 'App-SplitView.tsx');
  const skip = openingTags(app, ['a']).find(t => t.tag.includes('href="#main-content"'));
  assert.ok(skip, 'the skip link');
  // Hidden until it has focus, then drawn with a ring.
  assert.match(skip.tag, /sr-only focus:not-sr-only/);
  assert.match(skip.tag, /focus:ring-2/);
  // Its target takes the focus it sends.
  const main = openingTags(app, ['main']).find(t => t.tag.includes('id="main-content"'));
  assert.match(main?.tag ?? '', /tabIndex=\{-1\}/);
});

test('the sidebar: collapse, New chat, the views, history, Settings, the model', () => {
  const sidebar = withoutComments(source('components/shell/Sidebar.tsx'));
  const body = sidebar.slice(sidebar.indexOf('aria-label="Primary"'));
  inOrder(
    body,
    ['onClick={toggleSidebar}', 'onClick={onNewConversation}', '{PRIMARY_VIEWS.map(', '<ConversationHistory', 'view="settings"', '<ConnectionStatusChip'],
    'components/shell/Sidebar.tsx',
  );
});

// ── Ask and the message box ────────────────────────────────────────

test('Ask: the conversation, then the message box', () => {
  const ask = withoutComments(source('features/ask/AskView.tsx'));
  inOrder(ask, ['<DiagramHostContext.Provider', 'data-testid="ask-composer-dock"', '<ModelNotice', '{composer(true)}'], 'features/ask/AskView.tsx');
  // The message box takes the caret when Ask opens, so typing needs no Tab at all.
  assert.match(ask, /autoFocus/);
});

test('the message box: text, image, dictate, mode, model, send', () => {
  const composer = withoutComments(source('features/agent/AgentComposer.tsx'));
  inOrder(
    composer,
    [
      '<textarea',
      'data-testid={COMPOSER_TEST_IDS.image}',
      'data-testid={COMPOSER_TEST_IDS.dictate}',
      '{modeControl}',
      '{modelControl}',
      'data-testid={COMPOSER_TEST_IDS.steer}',
      'data-testid={COMPOSER_TEST_IDS.stop}',
      'data-testid={COMPOSER_TEST_IDS.send}',
    ],
    'features/agent/AgentComposer.tsx',
  );
  for (const id of ['image', 'dictate', 'mode', 'model', 'send', 'stop', 'steer'] as const) assert.ok(COMPOSER_TEST_IDS[id], id);
  // The text removes its own outline: the box around it is drawn focused instead.
  const text = openingTags(composer, ['textarea'])[0];
  assert.match(text.tag, NO_OUTLINE);
  assert.match(composer, /has-\[textarea:focus-visible\]:ring-1/);
  // Tab leaves the text for the row (Shift+Tab changes mode only in an empty box).
  assert.doesNotMatch(composer, /e\.key === 'Tab' && !e\.shiftKey/);
});

// ── The model menu ─────────────────────────────────────────────────

test('the model menu: opens from its button, moves by arrows, Escape returns to the button', () => {
  const trigger = withoutComments(source('features/modelPicker/ModelTrigger.tsx'));
  const button = openingTags(trigger, ['button'])[0];
  assert.match(button.tag, /data-testid=\{COMPOSER_TEST_IDS\.model\}/);
  assert.ok(FOCUS_STYLE.test(resolvedClasses(button.tag, trigger)), 'the model button draws a ring');

  // The floating chat's menu.
  const menu = withoutComments(source('features/modelPicker/ModelChoiceControl.tsx'));
  inOrder(menu, ['<ModelTrigger', 'aria-haspopup="menu"', 'role="menu"', 'role="menuitemradio"', 'role="menuitem"'], 'ModelChoiceControl.tsx');
  assert.match(menu, /nextMenuIndex\(e\.key,/);
  assert.match(menu, /if \(e\.key === 'Escape'\) \{[\s\S]*?close\(true\);/);
  assert.match(menu, /if \(refocus\) triggerRef\.current\?\.focus\(\);/);
  // On opening, focus is on the model that answers now.
  assert.match(menu, /\(checked \?\? items\?\.\[0\]\)\?\.focus\(\);/);
  for (const item of openingTags(menu, ['button'])) assert.ok(FOCUS_STYLE.test(resolvedClasses(item.tag, menu)), item.tag);

  // Ask's picker: a dialog from the same button, closed by Escape back onto it.
  const chip = withoutComments(source('features/modelPicker/ModelChip.tsx'));
  inOrder(chip, ['<ModelTrigger', 'role="dialog"', "e.key === 'Escape'", 'close(true)'], 'ModelChip.tsx');
  assert.match(chip, /if \(refocus\) triggerRef\.current\?\.focus\(\);/);

  // The mode menu beside it moves the same way.
  assert.match(withoutComments(source('features/agent/ModePicker.tsx')), /nextMenuIndex\(e\.key, index, items\.length\)/);
});

test('arrows walk every item of a menu and wrap; Home and End jump', () => {
  for (const count of [1, 2, 5, 12]) {
    // Down from the first visits each item once and comes back.
    const down: number[] = [];
    let at = 0;
    for (let i = 0; i < count; i++) {
      at = nextMenuIndex('ArrowDown', at, count)!;
      down.push(at);
    }
    assert.deepEqual([...down].sort((a, b) => a - b), Array.from({ length: count }, (_, i) => i));
    assert.equal(at, 0);
    // Up undoes Down from anywhere.
    for (let from = 0; from < count; from++) {
      assert.equal(nextMenuIndex('ArrowUp', nextMenuIndex('ArrowDown', from, count)!, count), from);
    }
    assert.equal(nextMenuIndex('Home', count - 1, count), 0);
    assert.equal(nextMenuIndex('End', 0, count), count - 1);
    // With focus on no item: Down takes the first, Up the last.
    assert.equal(nextMenuIndex('ArrowDown', -1, count), 0);
    assert.equal(nextMenuIndex('ArrowUp', -1, count), count - 1);
  }
  for (const key of ['Tab', 'Enter', ' ', 'a', 'Escape', 'ArrowLeft']) assert.equal(nextMenuIndex(key, 0, 3), null, key);
  assert.equal(nextMenuIndex('ArrowDown', 0, 0), null);
});

// ── Settings ───────────────────────────────────────────────────────

test('Settings: the sections, then the open section', () => {
  const settings = withoutComments(source('components/shell/SettingsView.tsx'));
  const body = settings.slice(settings.indexOf('<PageFrame'));
  inOrder(body, ['aria-label="Settings sections"', '{SECTIONS.map(', 'aria-current=', '</nav>', 'aria-labelledby="settings-section-heading"'], 'SettingsView.tsx');
  const section = openingTags(body, ['button'])[0];
  assert.match(section.tag, /aria-current=/);
  assert.ok(FOCUS_STYLE.test(resolvedClasses(section.tag, settings)), 'a section button draws a ring');
});

// ── Focus shows at every stop ──────────────────────────────────────

/** The files the path runs through. */
const PATH = [
  'components/shell/Sidebar.tsx',
  'components/shell/ConversationHistory.tsx',
  'components/shell/PageFrame.tsx',
  'components/shell/ViewState.tsx',
  'components/shell/SettingsView.tsx',
  'features/connection/ConnectionStatusChip.tsx',
  'features/connection/ModelNotice.tsx',
  'features/ask/AskView.tsx',
  'features/agent/AgentComposer.tsx',
  'features/agent/ModePicker.tsx',
  'features/modelPicker/ModelTrigger.tsx',
  'features/modelPicker/ModelChip.tsx',
  'features/modelPicker/ModelChoiceControl.tsx',
  'features/modelPicker/ModelPicker.tsx',
  'features/modelPicker/ModelSettingsSection.tsx',
  'features/dock/DockView.tsx',
  'components/SearchSettings.tsx',
  'components/AnswerCheckSettings.tsx',
  'components/ScanReaderSettings.tsx',
  'components/TableModelSettings.tsx',
  'components/ToolsSettings.tsx',
  'components/EvidenceTableSettings.tsx',
  'components/SendKeySettings.tsx',
  'components/BackgroundSettings.tsx',
  'components/AppearanceSettings.tsx',
  'components/NotificationSettings.tsx',
  'components/PrivacySettings.tsx',
  'components/MemorySettings.tsx',
];

/** A stop whose focus is drawn by the box around it: the file, and that box's style. */
const DRAWN_BY_ITS_BOX: Record<string, RegExp> = {
  'features/agent/AgentComposer.tsx': /has-\[textarea:focus-visible\]:ring-1/,
};

test('every stop on the path shows focus', () => {
  const hidden: string[] = [];
  let stops = 0;
  for (const file of PATH) {
    const text = source(file);
    for (const { name, at, tag } of openingTags(text, NATIVE_STOPS)) {
      if (skipped(tag)) continue;
      stops += 1;
      if (showsFocus(tag, text)) continue;
      if (name === 'textarea' && DRAWN_BY_ITS_BOX[file]?.test(text)) continue;
      hidden.push(`${file}:${lineOf(text, at)}`);
    }
  }
  assert.deepEqual(hidden, []);
  // The scan found the path (a renamed file would otherwise pass by finding nothing).
  assert.ok(stops > 100, `only ${stops} stops read`);
});

test('the focus check tells a focus style from an outline taken away', () => {
  const file = "const RING = 'focus-visible:outline-none focus-visible:ring-2';\nconst BARE = 'outline-none';\nconst round = cn(BASE, RING);\n";
  assert.ok(showsFocus('<button className={RING}>', file));
  assert.ok(showsFocus('<button className={round}>', file), 'through a constant built from another');
  assert.ok(showsFocus('<button className="px-2">', file), 'keeps the browser outline');
  assert.ok(!showsFocus('<button className={BARE}>', file));
  assert.ok(!showsFocus('<input className="outline-none focus:outline-none" />', file));
  assert.ok(!showsFocus('<div tabIndex={0} className="focus-visible:outline-none">', file));
  assert.ok(showsFocus('<div className="outline-none group-focus-visible:bg-ring focus-visible:border-ring">', file));
  assert.ok(showsFocus('<g className="outline-none [&:focus-visible>circle]:stroke-[var(--c-text)]">', file));
});

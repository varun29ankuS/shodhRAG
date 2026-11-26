/**
 * The composer's one row of controls (features/agent/composerRow and AgentComposer): one
 * line that never wraps, every control one height, Send last; one mode menu holding what
 * the answer works on and its permissions; the floating chat renders the same composer.
 * The tests here have no DOM: the row's rules are pure functions, and its markup is read
 * from the source, as the repo's other component tests do. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/composerRow.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  COMPOSER_CONTROL_HEIGHT,
  COMPOSER_CONTROL_PX,
  COMPOSER_ICON_CONTROL,
  COMPOSER_ROW_CLASS,
  COMPOSER_ROW_ORDER,
  COMPOSER_TEST_IDS,
  CREATOR_PERMISSIONS_NOTE,
  PERMISSION_MODES,
  WORK_KINDS,
  isPermissionMode,
  isWorkKind,
  modeTriggerText,
  permissionsApply,
  workChange,
  workKindOf,
} from '../src/features/agent/composerRow.ts';
import { moduleGraph, source, withoutComments } from './moduleGraph.ts';

const composer = () => withoutComments(source('features/agent/AgentComposer.tsx'));
const picker = () => withoutComments(source('features/agent/ModePicker.tsx'));

test('the row is one line that never wraps, as tall as its controls', () => {
  assert.equal(COMPOSER_CONTROL_PX, 32);
  assert.equal(COMPOSER_CONTROL_HEIGHT, 'h-8');
  assert.match(COMPOSER_ROW_CLASS, /\bflex-nowrap\b/);
  assert.match(COMPOSER_ROW_CLASS, /\bh-8\b/);
  assert.doesNotMatch(COMPOSER_ROW_CLASS, /flex-wrap(?!\S)/);
  assert.match(COMPOSER_ICON_CONTROL, /\bw-8 h-8\b/);
  const row = composer();
  // The row uses the shared class and nothing in the composer wraps.
  assert.match(row, /data-testid=\{COMPOSER_TEST_IDS\.row\}\s*className=\{cn\(COMPOSER_ROW_CLASS/);
  assert.doesNotMatch(row, /\bflex-wrap\b/);
  // Every button in the row takes its size from the shared token: no other height.
  assert.doesNotMatch(row, /h-\[(30|34|26)px\]|\bw-9 h-9\b/);
});

test('the controls come in one order and Send is last, at the far right', () => {
  assert.deepEqual([...COMPOSER_ROW_ORDER], ['image', 'dictate', 'mode', 'model', 'send']);
  const row = composer();
  const at = (pattern: RegExp) => {
    const index = row.search(pattern);
    assert.ok(index >= 0, String(pattern));
    return index;
  };
  const positions = [
    at(/COMPOSER_TEST_IDS\.image/),
    at(/COMPOSER_TEST_IDS\.dictate/),
    at(/\{modeControl\}/),
    at(/\{modelControl\}/),
    at(/COMPOSER_TEST_IDS\.send/),
  ];
  assert.deepEqual([...positions].sort((a, b) => a - b), positions);
  // Send and Interrupt share the last place, pushed right.
  assert.match(row, /<div className="ml-auto shrink-0 flex flex-nowrap items-center gap-1\.5">/);
  assert.ok(at(/COMPOSER_TEST_IDS\.stop/) > at(/\{modelControl\}/));
  for (const id of Object.values(COMPOSER_TEST_IDS)) assert.match(id, /^composer-[a-z]+$/);
});

test('mode and model share one height token with the row', () => {
  assert.match(picker(), /COMPOSER_CONTROL_HEIGHT,/);
  const trigger = withoutComments(source('features/modelPicker/ModelTrigger.tsx'));
  assert.match(trigger, /COMPOSER_CONTROL_HEIGHT,/);
  // Labels truncate rather than wrap the row.
  assert.match(picker(), /min-w-0 truncate/);
  assert.match(trigger, /min-w-0 truncate/);
  // Ask's model chip is the same trigger.
  assert.match(withoutComments(source('features/modelPicker/ModelChip.tsx')), /<ModelTrigger/);
});

test("one model state, one colour: the composer's dot is the sidebar's, from one mapping", () => {
  const trigger = withoutComments(source('features/modelPicker/ModelTrigger.tsx'));
  // The trigger writes no colour of its own: its dot is the state's, through DOT_CLASS.
  assert.match(trigger, /DOT_CLASS\[dot\]/);
  assert.doesNotMatch(trigger, /bg-shodh-(accent|info|warning|error|success)\b/);
  // Ask's chip, the floating chat and the sidebar's status all take the state from modelDot.
  const chip = withoutComments(source('features/modelPicker/ModelChip.tsx'));
  assert.match(chip, /dot=\{modelDot\(connection\)\}/);
  assert.match(withoutComments(source('features/modelPicker/useModelLabel.ts')), /const dot = modelDot\(snapshot\);/);
  const status = withoutComments(source('features/connection/connectionStatus.ts'));
  assert.match(status, /dot: modelDot\(snapshot\),/);
  // And one name: the sidebar's status and the composer's control both use modelDisplayName.
  assert.match(status, /const model = modelDisplayName\(models, active\.model\);/);
  assert.match(withoutComments(source('features/modelPicker/modelFormat.ts')), /label: modelDisplayName\(/);
  assert.match(withoutComments(source('features/connection/ConnectionStatusChip.tsx')), /pillView\(snapshot, localTime, models\?\.models \?\? \[\]\)/);
});

test('what the answer works on, and when permissions apply', () => {
  assert.deepEqual(WORK_KINDS.map(w => w.id), ['research', 'code', 'creator']);
  assert.deepEqual([...PERMISSION_MODES], ['ask', 'acceptEdits', 'plan', 'auto']);
  assert.equal(workKindOf('research', 'plan'), 'research');
  assert.equal(workKindOf('code', 'auto'), 'code');
  assert.equal(workKindOf('code', 'creator'), 'creator');
  assert.equal(workKindOf('research', null), 'research');
  assert.equal(permissionsApply('research'), true);
  assert.equal(permissionsApply('code'), true);
  assert.equal(permissionsApply('creator'), false);
  assert.ok(CREATOR_PERMISSIONS_NOTE.split(' ').length <= 4);
  for (const kind of WORK_KINDS) assert.ok(kind.short.split(' ').length <= 5, kind.short);
  assert.ok(isWorkKind('creator') && !isWorkKind('ask') && !isWorkKind(null));
  assert.ok(isPermissionMode('auto') && !isPermissionMode('creator') && !isPermissionMode('code'));
});

test('picking a kind of work changes only what it names, and leaving Creator gives the permission back', () => {
  const now = { conversationMode: 'research' as const, agentMode: 'acceptEdits' as const };
  assert.deepEqual(workChange('code', now, null, 'ask'), { conversationMode: 'code' });
  assert.deepEqual(workChange('research', now, null, 'ask'), {});
  // Creator is an agent mode: Research or Code stays as it was.
  assert.deepEqual(workChange('creator', now, null, 'ask'), { agentMode: 'creator' });
  const creator = { conversationMode: 'code' as const, agentMode: 'creator' as const };
  assert.deepEqual(workChange('creator', creator, null, 'ask'), {});
  // Leaving Creator: the permission it had before, else the default, else Ask first.
  assert.deepEqual(workChange('code', creator, 'plan', 'auto'), { agentMode: 'plan' });
  assert.deepEqual(workChange('research', creator, null, 'auto'), { conversationMode: 'research', agentMode: 'auto' });
  assert.deepEqual(workChange('code', creator, null, null), { agentMode: 'ask' });
  // A remembered "creator" is never handed back as a permission.
  assert.deepEqual(workChange('code', creator, 'creator', 'creator'), { agentMode: 'ask' });
});

test('the trigger shows the work, and the permission where it applies', () => {
  assert.deepEqual(modeTriggerText('code', 'acceptEdits'), { label: 'Code', detail: 'Accept edits' });
  assert.deepEqual(modeTriggerText('research', 'ask'), { label: 'Research', detail: 'Ask first' });
  assert.deepEqual(modeTriggerText('creator', null), { label: 'Creator', detail: null });
  assert.deepEqual(modeTriggerText('research', null), { label: 'Research', detail: null });
});

test('mode and permissions live in one menu, which keeps its shape', () => {
  const menu = picker();
  // One ARIA menu with two groups: the work, then the permissions.
  assert.equal(menu.match(/role="menu"/g)?.length, 1);
  const work = menu.indexOf('data-testid="mode-menu-work"');
  const permissions = menu.indexOf('data-testid="mode-menu-permissions"');
  assert.ok(work > 0 && permissions > work);
  assert.match(menu, /role="menuitemradio"[\s\S]*role="menuitemradio"/);
  // The permissions always render; in Creator they are disabled with the reason, not hidden.
  assert.match(menu, /\{PERMISSION_MODES\.map\(/);
  assert.match(menu, /aria-disabled=\{!permissionsOn \|\| undefined\}/);
  assert.match(menu, /\{!permissionsOn && <span[^>]*data-testid="mode-menu-permissions-note">\{CREATOR_PERMISSIONS_NOTE\}/);
  // Creator and Auto still take the confirming click.
  assert.match(menu, /if \(next === 'creator' && work !== 'creator' && confirming\?\.kind !== 'work'\)/);
  assert.match(menu, /if \(next === 'auto' && permission !== 'auto' && confirming\?\.kind !== 'permission'\)/);
  // Keyboard: arrows, Home/End, Esc returns focus to the trigger; disabled items are skipped.
  for (const key of ['ArrowDown', 'ArrowUp', 'Home', 'End', 'Escape', 'Tab']) assert.match(menu, new RegExp(`'${key}'`));
  assert.match(menu, /:not\(\[aria-disabled="true"\]\)/);
  assert.match(menu, /if \(restore\) buttonRef\.current\?\.focus\(\)/);
  assert.match(menu, /aria-haspopup="menu"/);
  // Paragraphs are gone from the menu: labels, one-line tooltips.
  assert.doesNotMatch(menu, /MODE_FLOOR|info\.hint/);
});

test('the mode menu and the composer stay pure, so the floating chat renders them', () => {
  const graph = moduleGraph(['features/agent/ModePicker.tsx', 'features/agent/AgentComposer.tsx', 'features/modelPicker/ModelTrigger.tsx']);
  for (const [name, text] of graph) {
    assert.doesNotMatch(text, /@tauri-apps\/api|\binvoke\(/, name);
  }
});

test('the floating chat renders the same composer and the same row as Ask', () => {
  const panel = withoutComments(source('components/shell/DockPanel.tsx'));
  const ask = withoutComments(source('features/ask/AskView.tsx'));
  for (const host of [panel, ask]) {
    assert.match(host, /<AgentComposer/);
    assert.match(host, /modeControl=\{\(\s*<ModePicker/);
    assert.match(host, /modelControl=\{/);
  }
  // No second composer anywhere.
  assert.equal(panel.match(/<textarea/g), null);
  assert.equal(ask.match(/<textarea/g), null);
  // The floating chat's mode menu acts through main, the same hook as Ask's.
  assert.match(panel, /onPickWork=\{work => onAction\(\{ kind: 'setWork', work \}\)\}/);
  assert.match(panel, /onPickPermission=\{mode => onAction\(\{ kind: 'setPermission', mode \}\)\}/);
  for (const host of ['features/ask/AskView.tsx', 'components/shell/ConversationDock.tsx', 'features/dock/useDockBridge.ts']) {
    assert.match(withoutComments(source(host)), /useConversationModes\(\)/, host);
  }
});

test('the composer row holds only its five controls: sources, design check and status live elsewhere', () => {
  const row = composer();
  assert.doesNotMatch(row, /scopeLabel|scopeAction|onOpenLibrary|modelChip/);
  const ask = withoutComments(source('features/ask/AskView.tsx'));
  // The status line is under each answer, with the newest answer's design check and tokens saved.
  assert.match(ask, /data-testid="answer-status"[\s\S]*<StatusLine transcript=\{message\.transcript\}/);
  assert.match(ask, /data-testid="answer-status"[\s\S]*<DesignCheckChip/);
  assert.doesNotMatch(ask, /<StatusLine transcript=\{latest\}/);
  // Code branch, code map and tools go to Ask's top-bar slot.
  assert.match(ask, /createPortal\(\s*<>[\s\S]*<CodeBranchChip[\s\S]*<CodeMapChip[\s\S]*<ToolsChip[\s\S]*topBarSlot,/);
  // Sources, with "Search all my library", are the top bar's.
  const app = withoutComments(source('App-SplitView.tsx'));
  assert.match(app, /<SourcesControl/);
  assert.match(app, /data-testid="topbar-ask-slot"/);
  const sources = withoutComments(source('components/shell/SourcesControl.tsx'));
  assert.match(sources, /data-testid="topbar-search-all"/);
  // The floating chat's status line is under its answer too, not above the composer.
  const panel = withoutComments(source('components/shell/DockPanel.tsx'));
  const log = panel.indexOf('role="log"');
  const status = panel.indexOf('data-testid="answer-status"');
  assert.ok(log > 0 && status > log && status < panel.indexOf('model.wrapComposer'));
});

test("the in-page floating chat's model chip never takes Ask's \"Switch model\" request", () => {
  // requestModelPicker shows Ask; the floating chat's chip unmounts as it does, so only
  // Ask's chip may take the request (else Ask's menu would never open).
  const chip = withoutComments(source('features/modelPicker/ModelChip.tsx'));
  assert.match(chip, /if \(!takesPickerRequests\) return;\s*if \(!lockedRef\.current && takeModelPickerRequest\(\)\) setOpen\(true\);/);
  assert.match(chip, /else if \(takesPickerRequests && takeModelPickerRequest\(\)\) setOpen\(true\);/);
  const dock = withoutComments(source('components/shell/ConversationDock.tsx'));
  assert.match(dock, /<ModelChip\s+answerRunning=\{isStreaming\}\s+takesPickerRequests=\{false\}/);
});

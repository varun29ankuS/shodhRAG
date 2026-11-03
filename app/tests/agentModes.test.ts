/**
 * Agent modes in the app: the picker's order and Shift+Tab, the default from Settings
 * (fail closed to Ask), and the mode the assistant may only suggest. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/agentModes.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  AGENT_MODES,
  DEFAULT_AGENT_MODE,
  PLAN_RUN_MODES,
  isAgentMode,
  modeSuggestion,
  nextAgentMode,
  parseAgentMode,
  resolveAgentMode,
} from '../src/features/agent/agentModes.ts';
import { parseAgentPrefs, parseAppSettings } from '../src/lib/appSettings.ts';
import { matchesShortcut, shortcutText } from '../src/lib/shortcuts.ts';

test('the modes are Ask, Accept edits, Plan and Auto; Shift+Tab never turns Auto on', () => {
  assert.deepEqual(AGENT_MODES.map(m => m.id), ['ask', 'acceptEdits', 'plan', 'auto']);
  assert.equal(DEFAULT_AGENT_MODE, 'ask');
  assert.equal(nextAgentMode('ask'), 'acceptEdits');
  assert.equal(nextAgentMode('acceptEdits'), 'plan');
  assert.equal(nextAgentMode('plan'), 'ask');
  assert.equal(nextAgentMode('auto'), 'ask');
  for (const mode of AGENT_MODES) assert.notEqual(nextAgentMode(mode.id), 'auto');
  for (const mode of AGENT_MODES) assert.ok(mode.hint.length > 0 && mode.label.length > 0);
  // There is no mode that bypasses everything.
  for (const name of ['bypass', 'creator', 'yolo', 'Auto', '', null, 3]) {
    assert.equal(isAgentMode(name), false, String(name));
    assert.equal(parseAgentMode(name), null, String(name));
  }
  // A plan runs in a mode that can carry it out, never in Plan again.
  assert.ok(!PLAN_RUN_MODES.includes('plan'));
});

test('Shift+Tab is the registry\'s mode shortcut', () => {
  const chord = { key: 'Tab', code: 'Tab', ctrlKey: false, metaKey: false, shiftKey: true, altKey: false };
  assert.ok(matchesShortcut(chord, 'cycleMode', false));
  assert.ok(matchesShortcut(chord, 'cycleMode', true));
  assert.ok(!matchesShortcut({ ...chord, shiftKey: false }, 'cycleMode', false), 'Tab alone moves the focus');
  assert.ok(!matchesShortcut({ ...chord, ctrlKey: true }, 'cycleMode', false));
  assert.equal(shortcutText('cycleMode', false), 'Shift+Tab');
});

test('a conversation keeps its mode; only a new one takes the default', () => {
  assert.equal(resolveAgentMode('plan', true, 'auto'), 'plan');
  assert.equal(resolveAgentMode(undefined, false, 'auto'), 'auto');
  // Started before it had a mode: Ask, whatever the default says now.
  assert.equal(resolveAgentMode(undefined, true, 'auto'), 'ask');
  // The default is not known yet: no mode, so nothing is sent on a guess.
  assert.equal(resolveAgentMode(undefined, false, null), null);
  // A mode this version does not know is not taken.
  assert.equal(resolveAgentMode('creator', true, 'auto'), 'ask');
});

test('the default mode fails closed to Ask', () => {
  assert.deepEqual(parseAgentPrefs({ defaultMode: 'auto' }), { defaultMode: 'auto' });
  for (const value of [undefined, null, {}, { defaultMode: 'bypass' }, { defaultMode: 1 }, []]) {
    assert.deepEqual(parseAgentPrefs(value), { defaultMode: 'ask' }, JSON.stringify(value));
  }
  const base = {
    preferences: { theme: 'dark', searchMaxResults: 8 },
    policy: { localOnly: false, webAccess: true },
  };
  assert.equal(parseAppSettings(base)?.agent.defaultMode, 'ask');
  assert.equal(parseAppSettings({ ...base, agent: { defaultMode: 'plan' } })?.agent.defaultMode, 'plan');
});

test('the assistant\'s suggestion is read from its finished suggest_mode step only', () => {
  const step = (tool: string, status: string, detail: unknown) => ({ tool, status, detail });
  const suggested = step('suggest_mode', 'done', { suggestedMode: 'auto', reason: 'Many small edits and test runs' });
  assert.deepEqual(modeSuggestion([suggested], 'ask'), { mode: 'auto', reason: 'Many small edits and test runs' });
  // The current mode is not suggested again.
  assert.equal(modeSuggestion([suggested], 'auto'), null);
  // The last suggestion counts.
  const later = step('suggest_mode', 'done', { suggestedMode: 'plan', reason: 'Look first' });
  assert.equal(modeSuggestion([suggested, later], 'ask')?.mode, 'plan');
  // Unfinished, failed, other tools and unknown modes are ignored.
  for (const steps of [
    [step('suggest_mode', 'running', { suggestedMode: 'auto' })],
    [step('suggest_mode', 'failed', { suggestedMode: 'auto' })],
    [step('search_documents', 'done', { suggestedMode: 'auto' })],
    [step('suggest_mode', 'done', { suggestedMode: 'bypass' })],
    [step('suggest_mode', 'done', null)],
    [step('suggest_mode', 'done', 'auto')],
  ]) {
    assert.equal(modeSuggestion(steps, 'ask'), null, JSON.stringify(steps));
  }
  // A long reason is cut for the button's line.
  const long = step('suggest_mode', 'done', { suggestedMode: 'auto', reason: 'x'.repeat(500) });
  assert.equal(modeSuggestion([long], 'ask')?.reason.length, 160);
});

/**
 * The floating chat's model menu (features/modelPicker/dockChoices.ts): the rows main
 * offers the chat over the preview, and what choosing one does. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/dockChoices.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { applyDockChoice, choiceId, dockModelChoices } from '../src/features/modelPicker/dockChoices.ts';
import type { ChoicePicker } from '../src/features/modelPicker/dockChoices.ts';
import { chipEntries, modelLabelOf } from '../src/features/modelPicker/modelFormat.ts';
import { EMPTY_MODEL_PREFS } from '../src/features/modelPicker/modelTypes.ts';
import type { ModelRef, PickId, PickerError } from '../src/features/modelPicker/modelTypes.ts';
import { MAX_MODEL_CHOICES, MODEL_CHOICE_ID, parseDockAction, parseDockSnapshot } from '../src/lib/dockProtocol.ts';
import { modelDisplayName, modelDot } from '../src/features/connection/connectionStatus.ts';
import type { ConnectionSnapshot, Health } from '../src/features/connection/connectionStatus.ts';

const OPUS: ModelRef = { provider: 'claude-sub', model: 'claude-opus-5-5' };
const GEMMA: ModelRef = { provider: 'openrouter', model: 'google/gemma-4-31b-it:free' };
const LLAMA: ModelRef = { provider: 'openrouter', model: 'meta-llama/llama-4-70b:free' };

function view(active: { model: ModelRef; source: 'session' | 'environment' | 'settings' } | null, pick: PickId | null = 'best') {
  return {
    active,
    picks: [
      { pick: 'best' as const, label: 'Best quality', model: OPUS, name: 'Claude Opus 5.5', fallbacks: [] },
      { pick: 'fast' as const, label: 'Fast', model: null, name: null, fallbacks: [] },
      { pick: 'free' as const, label: 'Free', model: GEMMA, name: 'Google: Gemma 4 31B (free)', fallbacks: [] },
    ],
    prefs: { ...EMPTY_MODEL_PREFS, pick, recent: [LLAMA, GEMMA] },
    models: [],
    connected: ['claude-sub' as const, 'openrouter' as const],
  };
}

function picker(v: ReturnType<typeof view> | null, refuse: PickerError | null = null): { picker: ChoicePicker; calls: string[] } {
  const calls: string[] = [];
  return {
    calls,
    picker: {
      view: v,
      select: async model => {
        calls.push(`select(${model.provider}:${model.model})`);
        return refuse;
      },
      choosePick: async pick => {
        calls.push(`choosePick(${pick})`);
        return refuse;
      },
    },
  };
}

test('the menu offers the rows of the composer chip, each with an id the relay takes', () => {
  const v = view({ model: OPUS, source: 'settings' });
  const choices = dockModelChoices(v);
  // The picks that resolve, then the recent models no pick already has: the chip's own rows.
  assert.deepEqual(choices.map(c => c.id), ['pick:best', 'pick:free', 'model:openrouter:meta-llama/llama-4-70b:free']);
  assert.equal(choices.length, chipEntries(v).length);
  assert.deepEqual(choices.map(c => c.title), ['Best quality', 'Free', 'meta-llama/llama-4-70b:free']);
  assert.deepEqual(choices.map(c => c.detail), ['Claude Opus 5.5 · Claude (Pro/Max)', 'Google: Gemma 4 31B (free) · OpenRouter', 'Recent · OpenRouter']);
  // Exactly the one that answers is selected.
  assert.deepEqual(choices.map(c => c.selected), [true, false, false]);
  for (const c of choices) assert.match(c.id, MODEL_CHOICE_ID);
  assert.ok(choices.length <= MAX_MODEL_CHOICES);
  // Nothing to offer before the models are read.
  assert.deepEqual(dockModelChoices(null), []);
});

test('a row whose id the relay would refuse is left to the full list in Ask', () => {
  const spaced: ModelRef = { provider: 'ollama', model: 'my model:latest' };
  assert.equal(choiceId({ kind: 'recent', pick: null, label: 'Recent', model: spaced, name: 'my model' }), null);
  const v = { ...view(null), prefs: { ...EMPTY_MODEL_PREFS, recent: [spaced, LLAMA] }, connected: ['claude-sub' as const, 'openrouter' as const, 'ollama' as const] };
  assert.ok(!dockModelChoices(v).some(c => c.id.includes(' ')));
  assert.ok(dockModelChoices(v).some(c => c.id === 'model:openrouter:meta-llama/llama-4-70b:free'));
});

test('the rows travel in the snapshot, and a pick comes back as an action', () => {
  const choices = dockModelChoices(view({ model: OPUS, source: 'settings' }));
  for (const choice of choices) {
    assert.deepEqual(parseDockAction(JSON.parse(JSON.stringify({ kind: 'setModel', id: choice.id }))), { kind: 'setModel', id: choice.id });
  }
  // The launch probe's snapshot, with these rows in place of its empty menu.
  const probe = readFileSync(new URL('../src-tauri/src/launch_probe.rs', import.meta.url), 'utf8');
  const body = probe.slice(probe.indexOf('fn snapshot()'));
  const start = body.indexOf('serde_json::json!(') + 'serde_json::json!('.length;
  const end = body.indexOf('\n    })') + '\n    }'.length;
  const base = JSON.parse(body.slice(start, end));
  const parsed = parseDockSnapshot({ ...base, model: { label: 'Claude Opus 5.5', ready: true, dot: 'green', choices } });
  assert.ok(parsed);
  assert.deepEqual(parsed.model.choices, choices);
  assert.equal(parsed.model.dot, 'green');
});

test('choosing a row does what the same click in Ask does', async () => {
  const active = view({ model: OPUS, source: 'settings' });
  // A pick is chosen as a pick, a recent model as a model.
  let p = picker(active);
  assert.deepEqual(await applyDockChoice('pick:free', p.picker), { kind: 'changed', name: 'Google: Gemma 4 31B (free)' });
  assert.deepEqual(p.calls, ['choosePick(free)']);
  p = picker(active);
  assert.deepEqual(await applyDockChoice('model:openrouter:meta-llama/llama-4-70b:free', p.picker), { kind: 'changed', name: 'meta-llama/llama-4-70b:free' });
  assert.deepEqual(p.calls, ['select(openrouter:meta-llama/llama-4-70b:free)']);
  // The row that already answers changes nothing.
  p = picker(active);
  assert.deepEqual(await applyDockChoice('pick:best', p.picker), { kind: 'same', name: 'Claude Opus 5.5' });
  assert.deepEqual(p.calls, []);
});

test('only a row offered now is chosen, and a refusal is reported, never overridden', async () => {
  const active = view({ model: OPUS, source: 'settings' });
  for (const id of ['pick:fast', 'pick:private', 'model:openrouter:not/offered', 'model:google:gemini', 'pick:', '']) {
    const p = picker(active);
    assert.deepEqual(await applyDockChoice(id, p.picker), { kind: 'unknown' }, id);
    assert.deepEqual(p.calls, [], id);
  }
  // Before the models are read there is nothing to choose.
  const none = picker(null);
  assert.deepEqual(await applyDockChoice('pick:best', none.picker), { kind: 'unknown' });
  // The environment sets the model: Ask asks before overriding it; the floating chat never does.
  const refusal: PickerError = { code: 'environment_set', message: 'The environment sets the model for this computer.' };
  const refused = picker(active, refusal);
  assert.deepEqual(await applyDockChoice('pick:free', refused.picker), { kind: 'refused', name: 'Google: Gemma 4 31B (free)', error: refusal });
  assert.deepEqual(refused.calls, ['choosePick(free)']);
  const source = readFileSync(new URL('../src/features/modelPicker/dockChoices.ts', import.meta.url), 'utf8');
  assert.doesNotMatch(source, /sessionOverride|confirmStealth/);
});

test('the floating chat shows the model menu itself, and main applies the pick', () => {
  const read = (path: string) => readFileSync(new URL(`../src/${path}`, import.meta.url), 'utf8');
  const dock = read('features/dock/DockView.tsx');
  assert.match(dock, /<ModelChoiceControl/);
  assert.match(dock, /onChoose=\{id => onAction\(\{ kind: 'setModel', id \}\)\}/);
  assert.match(dock, /onAll=\{\(\) => onAction\(\{ kind: 'pickModel' \}\)\}/);
  assert.doesNotMatch(dock, /change in Ask/i);
  const bridge = read('features/dock/useDockBridge.ts');
  assert.match(bridge, /setModel: id => \{\s*void chooseModel\(id\)/);
  // Every outcome but "it already answers" is told: changed, refused, no longer offered.
  for (const kind of ['changed', 'refused', 'unknown']) assert.match(bridge, new RegExp(`result\\.kind === '${kind}'`));
});

test('one name and one colour for the model that answers', () => {
  const models = [{ provider: 'openrouter' as const, id: GEMMA.model, name: 'Google: Gemma 4 31B (free)' }] as never;
  // Listed in the catalogue: its name there. Not listed: its id without the provider and ":free".
  assert.equal(modelDisplayName(models, GEMMA), 'Google: Gemma 4 31B (free)');
  assert.equal(modelDisplayName([], GEMMA), 'gemma-4-31b-it');
  assert.deepEqual(modelLabelOf({ active: { model: GEMMA, source: 'settings' }, models }), { label: 'Google: Gemma 4 31B (free)', ready: true });
  assert.deepEqual(modelLabelOf({ active: { model: GEMMA, source: 'settings' }, models: [] }), { label: 'gemma-4-31b-it', ready: true });
  assert.deepEqual(modelLabelOf({ active: null, models: [] }), { label: 'Choose a model', ready: false });
  assert.deepEqual(modelLabelOf(null), { label: 'Model', ready: false });

  const snapshot = (health: Health): ConnectionSnapshot => ({
    active: { model: GEMMA, source: 'settings' },
    localOnly: false,
    providers: [{
      provider: 'openrouter', label: 'OpenRouter', source: 'saved', health, message: null, httpStatus: null, quota: false,
      resetAtMs: null, checkedAtMs: null, learnedFrom: null, keyHint: null, envVar: null, checking: false,
    }],
  });
  assert.equal(modelDot(snapshot('works')), 'green');
  assert.equal(modelDot(snapshot('rejected')), 'red');
  assert.equal(modelDot(snapshot('limited')), 'amber');
  assert.equal(modelDot(snapshot('unchecked')), 'grey');
  assert.equal(modelDot(snapshot('unreachable')), 'grey');
  assert.equal(modelDot({ ...snapshot('works'), active: null }), 'grey');
  assert.equal(modelDot({ ...snapshot('works'), providers: [] }), 'grey');
  assert.equal(modelDot(null), 'grey');
});

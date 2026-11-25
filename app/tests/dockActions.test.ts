/**
 * The main window acts on the floating chat's actions (features/dock/dockActions) on its own
 * session, draft, dictation and panes. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/dockActions.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { applyDockAction, submitDockDraft } from '../src/features/dock/dockActions.ts';
import type { DockTarget, DraftSubmit } from '../src/features/dock/dockActions.ts';
import { parseDockAction } from '../src/lib/dockProtocol.ts';
import { source, withoutComments } from './moduleGraph.ts';

/** A target that records every call, as `name(args)`. */
function recording(state: Partial<Pick<DockTarget, 'isStreaming' | 'draft'>> & { listening?: boolean } = {}) {
  const calls: string[] = [];
  const target: DockTarget = {
    isStreaming: state.isStreaming ?? false,
    draft: state.draft ?? '',
    send: () => calls.push('send()'),
    steer: text => calls.push(`steer(${text})`),
    cancel: () => calls.push('cancel()'),
    approve: (id, allow, forConversation, reviewToken) => calls.push(`approve(${id},${allow},${forConversation},${reviewToken})`),
    setDraft: text => calls.push(`setDraft(${text})`),
    dictation: {
      listening: state.listening ?? false,
      toggle: () => calls.push('toggle()'),
    },
    setOpen: open => calls.push(`setOpen(${open})`),
    openInAsk: () => calls.push('openInAsk()'),
    close: () => calls.push('close()'),
    typed: () => calls.push('typed()'),
    runtime: op => calls.push(`runtime(${op})`),
    setRuntimeInstalled: installed => calls.push(`setRuntimeInstalled(${installed})`),
    review: (code, language) => calls.push(`review(${code},${language})`),
    openUrl: url => calls.push(`openUrl(${url})`),
    stopCommand: id => calls.push(`stopCommand(${id})`),
    openSettings: () => calls.push('openSettings()'),
    setWork: work => calls.push(`setWork(${work})`),
    setPermission: mode => calls.push(`setPermission(${mode})`),
    pickModel: () => calls.push('pickModel()'),
  };
  return { target, calls };
}

/** Through the relay and the check, as main receives it. */
function act(value: unknown, target: DockTarget) {
  const action = parseDockAction(JSON.parse(JSON.stringify(value)));
  assert.ok(action, JSON.stringify(value));
  applyDockAction(action, target);
}

test("a draft edit sets main's draft and counts as typing", () => {
  const { target, calls } = recording();
  act({ kind: 'draft', text: 'Make the header blue' }, target);
  assert.deepEqual(calls, ['setDraft(Make the header blue)', 'typed()']);
});

test('dictate on starts dictation once and dictate off stops it', () => {
  const idle = recording({ listening: false });
  act({ kind: 'dictate', on: true }, idle.target);
  assert.deepEqual(idle.calls, ['toggle()']);
  // Already listening: a second "on" (a double click, a resend) changes nothing.
  const listening = recording({ listening: true });
  act({ kind: 'dictate', on: true }, listening.target);
  assert.deepEqual(listening.calls, []);
  act({ kind: 'dictate', on: false }, listening.target);
  assert.deepEqual(listening.calls, ['toggle()']);
  act({ kind: 'dictate', on: false }, idle.target);
  assert.deepEqual(idle.calls, ['toggle()']);
});

test('send steer cancel approve reach the session', () => {
  const { target, calls } = recording({ isStreaming: true, draft: '  go left  ' });
  act({ kind: 'send' }, target);
  act({ kind: 'steer' }, target);
  act({ kind: 'cancel' }, target);
  act({ kind: 'approve', id: 'step-3', allow: true }, target);
  act({ kind: 'approve', id: 'step-4', allow: false, forConversation: true, reviewToken: 'rt-1' }, target);
  assert.deepEqual(calls, [
    'send()',
    'setDraft()',
    'steer(go left)',
    'cancel()',
    'approve(step-3,true,undefined,undefined)',
    'approve(step-4,false,true,rt-1)',
  ]);
});

test('an action while nothing is streaming cannot steer', () => {
  const { target, calls } = recording({ isStreaming: false, draft: 'go left' });
  act({ kind: 'steer' }, target);
  assert.deepEqual(calls, []);
  // Nor with nothing typed.
  const empty = recording({ isStreaming: true, draft: '   ' });
  act({ kind: 'steer' }, empty.target);
  assert.deepEqual(empty.calls, []);
});

test("the transcript's buttons in the floating chat act in main", () => {
  const { target, calls } = recording();
  act({ kind: 'open', open: false }, target);
  act({ kind: 'openInAsk' }, target);
  act({ kind: 'close' }, target);
  act({ kind: 'runtime', op: 'install' }, target);
  act({ kind: 'runtimeInstalled', installed: true }, target);
  act({ kind: 'review', code: 'let x = 1;', language: 'ts' }, target);
  act({ kind: 'openUrl', url: 'https://example.com/' }, target);
  act({ kind: 'stopCommand', id: 'cmd-1' }, target);
  act({ kind: 'openSettings' }, target);
  act({ kind: 'setWork', work: 'code' }, target);
  act({ kind: 'setPermission', mode: 'plan' }, target);
  act({ kind: 'pickModel' }, target);
  assert.deepEqual(calls, [
    'setOpen(false)',
    'openInAsk()',
    'close()',
    'runtime(install)',
    'setRuntimeInstalled(true)',
    'review(let x = 1;,ts)',
    'openUrl(https://example.com/)',
    'stopCommand(cmd-1)',
    'openSettings()',
    'setWork(code)',
    'setPermission(plan)',
    'pickModel()',
  ]);
});

function ports(overrides: Partial<DraftSubmit> = {}) {
  const log: string[] = [];
  let checking = false;
  const value: DraftSubmit = {
    conversationId: 'c1',
    isStreaming: false,
    busyElsewhere: false,
    checking: () => checking,
    setChecking: on => {
      checking = on;
      log.push(`checking(${on})`);
    },
    currentDraft: () => 'make it blue',
    queuedFile: () => null,
    send: (text, files) => {
      log.push(`send(${text}|${files.join(',')})`);
      return true;
    },
    sent: file => log.push(`sent(${file?.path ?? ''})`),
    hold: () => false,
    missing: async () => [],
    reportMissing: missing => log.push(`missing(${missing.join(',')})`),
    shownConversation: () => 'c1',
    ...overrides,
  };
  return { value, log };
}

test('the shared draft is sent as Ask sends it', async () => {
  const plain = ports();
  submitDockDraft('make it blue', plain.value);
  assert.deepEqual(plain.log, ['send(make it blue|)', 'sent()']);
  // Nothing typed, an answer running, or another conversation busy: nothing goes.
  for (const blocked of [{ isStreaming: true }, { busyElsewhere: true }]) {
    const p = ports(blocked);
    submitDockDraft('make it blue', p.value);
    assert.deepEqual(p.log, []);
  }
  const empty = ports();
  submitDockDraft('   ', empty.value);
  assert.deepEqual(empty.log, []);
  // Before Shodh is ready the question waits for setup.
  const held = ports({ hold: () => true });
  submitDockDraft('make it blue', held.value);
  assert.deepEqual(held.log, []);
  // A chosen file is checked first, then limits the search.
  const file = { path: 'C:/Docs/lease.pdf' };
  const withFile = ports({ queuedFile: () => file });
  submitDockDraft('make it blue', withFile.value);
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.deepEqual(withFile.log, ['checking(true)', 'send(make it blue|C:/Docs/lease.pdf)', 'sent(C:/Docs/lease.pdf)', 'checking(false)']);
  // A file gone since it was chosen is said plainly, and nothing is sent.
  const gone = ports({ queuedFile: () => file, missing: async () => [file.path] });
  submitDockDraft('make it blue', gone.value);
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.deepEqual(gone.log, ['checking(true)', 'missing(C:/Docs/lease.pdf)', 'checking(false)']);
});

test('dictation runs in main: the bridge holds the microphone and the dock never does', () => {
  const bridge = withoutComments(source('features/dock/useDockBridge.ts'));
  assert.match(bridge, /const voice = useVoiceInput\(getDraft, setDraft\);/);
  assert.match(bridge, /dictating: voice\.listening,\s*voiceSupported: voice\.supported,/);
  assert.match(bridge, /dictation: \{ listening: voice\.listening, toggle: voice\.toggle \}/);
  assert.match(bridge, /typed: \(\) => typingActivity\.noteRemote\(Date\.now\(\)\)/);
  // The in-page chat steps aside while the layer is up, so there is one chat.
  const app = source('App-SplitView.tsx');
  assert.match(app, /\{dockLayerActive\s*\? <DockBridge active /);
});

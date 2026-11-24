/**
 * The floating chat's page (`features/dock/DockView`, the `dock` webview over the embedded
 * preview): it shows main's snapshot with the same transcript as Ask and the in-page chat,
 * never subscribes to the agent's events, calls only its own two commands and never touches a
 * device. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/dockView.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { moduleGraph, naming, source, withoutComments } from './moduleGraph.ts';

/** What main.tsx renders on the dock route (as acl_manifest.rs's DOCK_ROOTS). */
const DOCK_ROOTS = ['features/dock/DockView.tsx', 'contexts/ThemeContext.tsx', 'components/ErrorBoundary.tsx'];

/** Every command the app registers (lib.rs's generate_handler!), as agent_coverage.rs reads them. */
function registeredCommands(): Set<string> {
  const lib = readFileSync(new URL('../src-tauri/src/lib.rs', import.meta.url), 'utf8');
  const body = lib.slice(lib.indexOf('generate_handler!['));
  const list = body.slice(0, body.indexOf('])'));
  return new Set([...list.matchAll(/^\s*[A-Za-z_]\w*::([A-Za-z_]\w*)\s*,/gm)].map(m => m[1]));
}

test('the dock entry never listens to agent events', () => {
  const graph = moduleGraph(DOCK_ROOTS);
  // The shared transcript is in it, so this covers what the chat renders.
  for (const shown of ['components/shell/DockPanel.tsx', 'features/agent/Transcript.tsx', 'features/ask/MessageContentRenderer.tsx', 'features/agent/RuntimeCard.tsx']) {
    assert.ok(graph.has(shown), shown);
  }
  assert.deepEqual(naming(graph, /\bagent_event\b|\bAGENT_EVENT\b/), []);
  for (const mainOnly of ['features/ask/ChatSessionContext.tsx', 'features/agent/useAgentSession.ts', 'features/focus/FocusContext.tsx', 'features/setup/RuntimeSetupContext.tsx', 'features/code/CodeReviewContext.tsx']) {
    assert.ok(!graph.has(mainOnly), `the dock loads ${mainOnly}`);
  }
});

test('the dock entry calls only its own commands', () => {
  const dock = moduleGraph(DOCK_ROOTS);
  // The modules only the dock loads, as acl_manifest.rs's dock-window check reads them: main's
  // graph leaves out the routes main.tsx renders on their own.
  const main = moduleGraph(['main.tsx'], ['features/print/PrintView.tsx', 'features/dock/DockView.tsx']);
  const dockOnly = [...dock.keys()].filter(name => !main.has(name));
  assert.ok(dockOnly.includes('features/dock/DockView.tsx'));
  const commands = registeredCommands();
  assert.ok(commands.size > 50, 'lib.rs was read');
  const named = new Set<string>();
  for (const name of dockOnly) {
    for (const match of (dock.get(name) as string).matchAll(/["'`]([A-Za-z_][A-Za-z0-9_]*)["'`]/g)) {
      if (commands.has(match[1])) named.add(match[1]);
    }
  }
  assert.deepEqual([...named].sort(), ['dock_layer_action', 'dock_layer_size']);
  // No plugin is called from the dock's own code.
  assert.doesNotMatch(withoutComments(source('features/dock/DockView.tsx')), /@tauri-apps\/plugin-|plugin:/);
});

test('the dock entry never touches a device', () => {
  const graph = moduleGraph(DOCK_ROOTS);
  assert.deepEqual(naming(graph, /\bgetUserMedia\b|\bwebkitSpeechRecognition\b|\bSpeechRecognition\b|\buseVoiceInput\b/), []);
  // The microphone is the composer's `voice` prop, driven by main's dictation.
  const panel = source('components/shell/DockPanel.tsx');
  assert.match(panel, /listening: snapshot\.dictating, toggle: \(\) => onAction\(\{ kind: 'dictate', on: !snapshot\.dictating \}\)/);
});

test('main.tsx routes /dock-view to DockView', () => {
  const main = source('main.tsx');
  assert.match(main, /import DockView from "\.\/features\/dock\/DockView";/);
  assert.match(main, /path === '\/dock-view'[\s\S]*?<DockView \/>/);
});

test('the dock gives the shared transcript its hosts, sending their buttons to main', () => {
  const view = withoutComments(source('features/dock/DockView.tsx'));
  assert.match(view, /<RuntimeSetupContext\.Provider value=\{runtime\}>/);
  assert.match(view, /<CodeReviewContext\.Provider value=\{REMOTE_REVIEW\}>/);
  assert.match(view, /<TranscriptEffectsContext\.Provider value=\{REMOTE_EFFECTS\}>/);
  assert.match(view, /<DockPanel model=\{model\} onExpandAsk=\{\(\) => send\(\{ kind: 'openInAsk' \}\)\} fixed=\{false\}/);
  for (const action of ["kind: 'runtime', op: 'install'", "kind: 'review'", "kind: 'openUrl'", "kind: 'stopCommand'", "kind: 'runtimeInstalled'"]) {
    assert.ok(view.includes(action), action);
  }
  // The in-page chat renders the very same panel.
  assert.match(source('components/shell/ConversationDock.tsx'), /<DockPanel\b/);
  assert.match(source('components/shell/DockPanel.tsx'), /<Transcript\b/);
});

test('earlier work left out of a long answer is offered in Ask, never silently dropped', () => {
  const panel = source('components/shell/DockPanel.tsx');
  assert.match(panel, /\{snapshot\.trimmed && \(\s*<button\s+type="button"\s+onClick=\{onExpandAsk\}/);
});

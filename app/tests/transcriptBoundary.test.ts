/**
 * The transcript renders apart from the main window's session: its tree (the transcript,
 * its status line and the composer) imports none of the main-only providers and never
 * names the agent event stream or a device. It reads focus, the answer engine's setup and
 * code review through their host modules, which the main window's providers fill, so Ask
 * is unchanged, and which the floating chat over the preview fills from its snapshot.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/transcriptBoundary.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { moduleGraph, naming, source, withoutComments } from './moduleGraph.ts';

const TRANSCRIPT_TREE = ['features/agent/Transcript.tsx', 'features/agent/StatusLine.tsx', 'features/agent/AgentComposer.tsx'];

/** The providers that run main's session, its agent event stream and the microphone. */
const MAIN_ONLY = [
  'features/ask/ChatSessionContext.tsx',
  'features/focus/FocusContext.tsx',
  'features/setup/RuntimeSetupContext.tsx',
  'features/code/CodeReviewContext.tsx',
  'features/agent/useAgentSession.ts',
  'features/ask/useVoiceInput.ts',
];

const MAIN_ONLY_NAMES = /\bagent_event\b|\bAGENT_EVENT\b|\bgetUserMedia\b|\bwebkitSpeechRecognition\b|\bSpeechRecognition\b|\buseVoiceInput\b/;

test('the transcript tree imports none of the main-only providers', () => {
  const graph = moduleGraph(TRANSCRIPT_TREE);
  assert.ok(graph.size > 50, `only ${graph.size} modules: the graph was not walked`);
  // The renderer, the focus frame and the runtime card are in it: the test covers them.
  for (const shown of ['features/ask/MessageContentRenderer.tsx', 'features/focus/FocusFrame.tsx', 'features/agent/RuntimeCard.tsx', 'features/code/ReviewThisButton.tsx']) {
    assert.ok(graph.has(shown), shown);
  }
  assert.deepEqual(MAIN_ONLY.filter(name => graph.has(name)), []);
  assert.deepEqual(naming(graph, MAIN_ONLY_NAMES), []);
});

test('the transcript reads focus, setup and review through their hosts', () => {
  assert.match(source('features/focus/FocusFrame.tsx'), /from '\.\/focusHost'/);
  assert.match(source('features/agent/RuntimeCard.tsx'), /from '\.\.\/setup\/runtimeSetupHost'/);
  assert.match(source('features/code/ReviewThisButton.tsx'), /from '\.\/codeReviewHost'/);
  // The hosts hold no provider: they import no agent API that runs anything at render.
  for (const host of ['features/focus/focusHost.tsx', 'features/setup/runtimeSetupHost.ts', 'features/code/codeReviewHost.ts']) {
    assert.doesNotMatch(source(host), /useAgentSession|ChatSessionContext|invoke\(/, host);
  }
});

test("main's providers fill the same contexts, so Ask is unchanged", () => {
  const focus = source('features/focus/FocusContext.tsx');
  assert.match(focus, /import \{ FocusContext \} from '\.\/focusHost';/);
  assert.match(focus, /export \* from '\.\/focusHost';/);
  assert.match(focus, /<FocusContext\.Provider value=\{value\}>/);
  assert.doesNotMatch(focus, /createContext/);
  const runtime = source('features/setup/RuntimeSetupContext.tsx');
  assert.match(runtime, /import \{ RuntimeSetupContext \} from '\.\/runtimeSetupHost';/);
  assert.match(runtime, /<RuntimeSetupContext\.Provider value=\{value\}>/);
  assert.doesNotMatch(runtime, /createContext/);
  const review = source('features/code/CodeReviewContext.tsx');
  assert.match(review, /import \{ CodeReviewContext, isTextEntry \} from '\.\/codeReviewHost';/);
  assert.match(review, /export \* from '\.\/codeReviewHost';/);
  assert.doesNotMatch(review, /createContext/);
  // The useRuntimeSetup of the host still refuses to run outside a provider in main.
  assert.match(source('features/setup/runtimeSetupHost.ts'), /throw new Error\('useRuntimeSetup must be used within a RuntimeSetupProvider'\)/);
});

test('the agent event stream is subscribed only where main runs its session', () => {
  const stream = source('features/agent/useAgentSession.ts');
  assert.match(stream, /export const AGENT_EVENT = 'agent_event';/);
  assert.match(stream, /useTauriEvent\(AGENT_EVENT, isEnvelope, onEvent\)/);
  assert.doesNotMatch(source('features/agent/agentApi.ts').replace(/\/\*[\s\S]*?\*\//g, ''), /AGENT_EVENT|'agent_event'/);
});

test('Ask keeps its microphone; the composer only shows the dictation it is given', () => {
  const composer = withoutComments(source('features/agent/AgentComposer.tsx'));
  assert.doesNotMatch(composer, /useVoiceInput/);
  assert.match(composer, /\{voice\?\.supported && \(/);
  const ask = source('features/ask/AskView.tsx');
  assert.match(ask, /const voice = useVoiceInput\(getDraft, setDraft\);/);
  assert.match(ask, /voice=\{voice\}/);
});

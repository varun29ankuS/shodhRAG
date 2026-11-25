/**
 * The composer while an answer runs and the answer as it shows its work: an HTML block
 * previewed inline (locked frame) with its source one click away, mode and model locked
 * during a run, approvals docked above the composer, and the send key from Settings.
 * The rules are pure functions; the markup is read from the source, as the repo's other
 * component tests do. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/composerRuntime.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { HTML_PREVIEW_POLICY, HTML_PREVIEW_SANDBOX, isHtmlLanguage, previewDocument } from '../src/features/ask/htmlPreview.ts';
import { DEFAULT_SEND_KEY, SEND_KEYS, composerKey, parseSendKey, sendShortcut } from '../src/lib/sendKey.ts';
import { shortcutText } from '../src/lib/shortcuts.ts';
import { RUN_LOCKED_REASON } from '../src/features/agent/composerRow.ts';
import { source, withoutComments } from './moduleGraph.ts';

const policyMeta = `<meta http-equiv="Content-Security-Policy" content="${HTML_PREVIEW_POLICY}">`;

test('an HTML block previews in a frame that runs no scripts and loads nothing', () => {
  assert.equal(HTML_PREVIEW_SANDBOX, '');
  assert.match(HTML_PREVIEW_POLICY, /default-src 'none'/);
  assert.doesNotMatch(HTML_PREVIEW_POLICY, /script-src|connect-src|https?:/);
  for (const lang of ['html', 'HTML', 'htm', 'xhtml']) assert.ok(isHtmlLanguage(lang), lang);
  for (const lang of ['xml', 'svg', 'js', 'markdown']) assert.ok(!isHtmlLanguage(lang), lang);
  // The policy is the first thing in the head, before anything the page declares.
  assert.equal(
    previewDocument('<html><head><title>x</title></head><body>hi</body></html>'),
    `<html><head>${policyMeta}<title>x</title></head><body>hi</body></html>`,
  );
  assert.equal(
    previewDocument('<html lang="en"><body>hi</body></html>'),
    `<html lang="en"><head>${policyMeta}</head><body>hi</body></html>`,
  );
  // A fragment gets a whole page around it.
  const fragment = previewDocument('<button>Go</button>');
  assert.ok(fragment.startsWith('<!doctype html><html><head><meta charset="utf-8">'));
  assert.ok(fragment.includes(policyMeta) && fragment.endsWith('<body><button>Go</button></body></html>'));
});

test('the preview frame is the locked one, with Preview and Raw', () => {
  const block = withoutComments(source('features/ask/HtmlPreviewBlock.tsx'));
  assert.match(block, /<iframe[\s\S]*sandbox=\{HTML_PREVIEW_SANDBOX\}[\s\S]*srcDoc=\{previewDocument\(shown\)\}/);
  assert.doesNotMatch(block, /allow-scripts|allow-same-origin|dangerouslySetInnerHTML/);
  assert.match(block, /tab\('preview', 'Preview'\)/);
  assert.match(block, /tab\('raw', 'Raw'\)/);
  assert.match(block, /data-testid="html-preview"/);
  // Shown inline in the answer, never made into an artifact.
  const renderer = withoutComments(source('features/ask/MessageContentRenderer.tsx'));
  assert.match(renderer, /if \(isHtmlLanguage\(match\[1\]\)\) \{[\s\S]*<HtmlPreviewBlock/);
  assert.doesNotMatch(block, /artifact/i);
});

test('mode and model are locked while an answer runs, with one short tooltip', () => {
  assert.equal(RUN_LOCKED_REASON, 'Locked while answering');
  const ask = withoutComments(source('features/ask/AskView.tsx'));
  assert.match(ask, /const runLock = isStreaming \? RUN_LOCKED_REASON : null;/);
  assert.match(ask, /lockedReason=\{runLock\}\s*\/>\s*\)\}/);
  assert.ok(ask.includes("<ModelChip answerRunning={isStreaming} onOpenSettings={() => onNavigate('settings')} lockedReason={runLock} />"));
  // Shift+Tab does not change the permission during a run either.
  assert.match(ask, /onCycleMode=\{conversationId && !isStreaming \? cycleAgentMode : undefined\}/);
  const panel = withoutComments(source('components/shell/DockPanel.tsx'));
  assert.match(panel, /lockedReason=\{snapshot\.isStreaming \? RUN_LOCKED_REASON : null\}/);
  // The controls are disabled with the reason as their tooltip.
  const picker = withoutComments(source('features/agent/ModePicker.tsx'));
  assert.match(picker, /disabled=\{locked\}/);
  assert.match(picker, /const title = lockedReason !== null\s*\? lockedReason/);
  const trigger = withoutComments(source('features/modelPicker/ModelTrigger.tsx'));
  assert.match(trigger, /disabled=\{locked \|\| disabled\}/);
  assert.match(trigger, /title=\{lockedReason !== null \? lockedReason/);
  // A change sent from the floating chat during a run is refused by main too.
  const hook = withoutComments(source('features/ask/useConversationModes.ts'));
  assert.equal(hook.match(/if \(!id \|\| streamingRef\.current\) return;/g)?.length, 2);
});

test('the pending approval docks above the composer box, in its own slot', () => {
  const composer = withoutComments(source('features/agent/AgentComposer.tsx'));
  const dock = composer.indexOf('data-testid="approval-dock"');
  assert.ok(dock > 0 && dock < composer.indexOf('{box}'));
  for (const host of ['features/ask/AskView.tsx', 'components/shell/DockPanel.tsx']) {
    const text = withoutComments(source(host));
    assert.match(text, /approvalDock=\{\w+\?\.approval \? \(\s*<ApprovalPrompt/, host);
    // The transcript does not show it a second time.
    assert.match(text, /approvalsDocked\b/, host);
  }
  const transcript = withoutComments(source('features/agent/Transcript.tsx'));
  assert.match(transcript, /pendingApprovals=\{!approvalsDocked\}/);
  const step = withoutComments(source('features/agent/StepLine.tsx'));
  assert.match(step, /\(pendingApprovals \|\| step\.approval\.decision !== 'pending'\)/);
});

test('the send key: Enter (Shift+Enter for a new line) or Ctrl+Enter (Enter for a new line)', () => {
  assert.deepEqual([...SEND_KEYS], ['enter', 'modEnter']);
  assert.equal(DEFAULT_SEND_KEY, 'enter');
  for (const value of [undefined, null, 'altEnter', 1, {}]) assert.equal(parseSendKey(value), 'enter');
  assert.equal(parseSendKey('modEnter'), 'modEnter');
  const press = (over: Partial<{ ctrlKey: boolean; metaKey: boolean; shiftKey: boolean; altKey: boolean; isComposing: boolean; key: string }>) => ({
    key: 'Enter', code: 'Enter', ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, isComposing: false, ...over,
  });
  // Enter mode.
  assert.equal(composerKey(press({}), 'enter', false), 'send');
  assert.equal(composerKey(press({ shiftKey: true }), 'enter', false), 'newline');
  assert.equal(composerKey(press({ ctrlKey: true }), 'enter', false), 'other');
  // Ctrl+Enter mode (⌘ on a Mac).
  assert.equal(composerKey(press({ ctrlKey: true }), 'modEnter', false), 'send');
  assert.equal(composerKey(press({ metaKey: true }), 'modEnter', true), 'send');
  assert.equal(composerKey(press({ ctrlKey: true }), 'modEnter', true), 'other');
  assert.equal(composerKey(press({}), 'modEnter', false), 'newline');
  // An IME composition and other keys are never a send.
  assert.equal(composerKey(press({ isComposing: true }), 'enter', false), 'other');
  assert.equal(composerKey(press({ key: 'a', code: 'KeyA' }), 'enter', false), 'other');
  // The hints come from the registry.
  assert.equal(shortcutText(sendShortcut('enter'), false), 'Enter');
  assert.equal(shortcutText(sendShortcut('modEnter'), false), 'Ctrl+Enter');
  assert.equal(shortcutText(sendShortcut('modEnter'), true), '⌘Enter');
});

test('the send key is kept by the backend settings and shown in Settings → General', () => {
  const settings = withoutComments(source('lib/appSettings.ts'));
  assert.match(settings, /sendKey: parseSendKey\(preferences\.sendKey\),/);
  const hook = withoutComments(source('hooks/useSendKey.ts'));
  assert.match(hook, /updatePreferences\(\{ sendKey: next \}\)/);
  assert.doesNotMatch(hook, /localStorage/);
  const view = withoutComments(source('components/shell/SettingsView.tsx'));
  assert.match(view, /section === 'general' \? \(\s*<div className="flex flex-col gap-8">\s*<SendKeySettings \/>/);
  const panel = withoutComments(source('components/SendKeySettings.tsx'));
  assert.match(panel, /`Send with \$\{shortcutText\(sendShortcut\(key\), IS_MAC_PLATFORM\)\}`/);
  // The composer reads it in Ask and both floating chats (the one over the preview by snapshot).
  assert.match(withoutComments(source('features/ask/AskView.tsx')), /sendKey=\{sendKey\}/);
  assert.match(withoutComments(source('components/shell/DockPanel.tsx')), /sendKey=\{snapshot\.sendKey\}/);
  const composer = withoutComments(source('features/agent/AgentComposer.tsx'));
  assert.match(composer, /if \(press !== 'send'\) return;/);
  assert.match(composer, /title=\{withShortcut\('Send', sendShortcut\(sendKey\), IS_MAC_PLATFORM\)\}/);
});

test('a command running now stays in view under the folded line, with its own Stop', () => {
  const transcript = withoutComments(source('features/agent/Transcript.tsx'));
  assert.match(transcript, /if \(s && s\.command && s\.status === 'running'\) return s;/);
  assert.match(transcript, /if \(!s \|\| s === runningCommand\) return null;/);
  assert.match(transcript, /\{runningCommand && \(\s*<StepLine step=\{runningCommand\}/);
});

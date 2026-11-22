/**
 * When the Ask composer blocks Send, and the one-click fix it offers. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/sendBlock.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  CODE_MODE_OFF_REASON,
  CREATOR_CODE_MODE_OFF_REASON,
  sendBlock,
} from '../src/features/ask/sendBlock.ts';
import type { SendBlockInput } from '../src/features/ask/sendBlock.ts';

const FOLDER_PROBLEM = "Code mode works in a workspace: move this chat into your project's workspace (its one folder is the code folder).";

const ready: SendBlockInput = {
  waiting: false,
  agentMode: 'ask',
  sideRunLabel: null,
  busyElsewhere: false,
  conversationMode: 'research',
  codeModeEnabled: true,
  codeProblem: null,
  streaming: false,
};

test('nothing blocks a ready Research conversation', () => {
  assert.equal(sendBlock(ready), null);
});

test('Creator is not blocked by the workspace code-folder problem: it builds in its own folder', () => {
  const block = sendBlock({ ...ready, agentMode: 'creator', conversationMode: 'code', codeProblem: FOLDER_PROBLEM });
  assert.equal(block, null);
});

test('a Code conversation in another mode is still blocked by the code-folder problem', () => {
  const block = sendBlock({ ...ready, agentMode: 'acceptEdits', conversationMode: 'code', codeProblem: FOLDER_PROBLEM });
  assert.deepEqual(block, { reason: `${FOLDER_PROBLEM} Or switch this conversation to Research.`, fix: null });
});

test('Creator with Code mode off is blocked before sending, with the fix offered', () => {
  const block = sendBlock({ ...ready, agentMode: 'creator', codeModeEnabled: false });
  assert.deepEqual(block, { reason: CREATOR_CODE_MODE_OFF_REASON, fix: 'turnOnCodeMode' });
});

test('a Code conversation with Code mode off offers the fix', () => {
  const block = sendBlock({ ...ready, conversationMode: 'code', codeModeEnabled: false });
  assert.deepEqual(block, { reason: CODE_MODE_OFF_REASON, fix: 'turnOnCodeMode' });
});

test('Code mode not yet read blocks nothing (no block on a guess)', () => {
  assert.equal(sendBlock({ ...ready, agentMode: 'creator', codeModeEnabled: null }), null);
});

test('a running answer can be steered: Code checks do not block it', () => {
  const block = sendBlock({ ...ready, agentMode: 'creator', codeModeEnabled: false, streaming: true });
  assert.equal(block, null);
});

test('waiting, reading the mode, a side question and another answer come first, in that order', () => {
  assert.equal(sendBlock({ ...ready, waiting: true, agentMode: null })?.reason, 'Your question is sent as soon as Shodh is ready.');
  assert.equal(sendBlock({ ...ready, agentMode: null, busyElsewhere: true })?.reason, 'Reading your default mode…');
  assert.match(sendBlock({ ...ready, sideRunLabel: 'gradient', busyElsewhere: true })?.reason ?? '', /side question about “gradient”/);
  assert.equal(
    sendBlock({ ...ready, busyElsewhere: true, codeModeEnabled: false, conversationMode: 'code' })?.reason,
    'An answer is still being written in another conversation. You can send once it finishes.',
  );
});

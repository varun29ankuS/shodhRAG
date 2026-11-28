/**
 * Finding K (UI structure audit): Ask cannot build a page or an app; Creator can. The Ask
 * prompt says so, and an Ask answer to a request to build one carries "Build in Creator",
 * which hands the user's words to a new Creator chat. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/buildInCreator.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { asksToBuild } from '../src/features/ask/buildRequest.ts';
import { appDir, source, withoutComments } from './jsxScan.ts';

test('a request to build a page, an app or a game is one', () => {
  for (const ask of [
    'build me a small artifact page',
    'Make me an HTML page about recall',
    'create a landing page for the lab',
    'can you build a simple game?',
    'Please write a web app that tracks my reading',
    'design a dashboard of my tasks',
    'make a quiz from chapter 3',
  ]) assert.ok(asksToBuild(ask), ask);
});

test('a question about building, or a request for something else, is not', () => {
  for (const ask of [
    'How do I build a web app?',
    'What does the paper say about the app store?',
    'Explain how the dashboard is built',
    'Summarise the reading notes',
    'make a list of the papers I read',
    'write the comparison with its sources',
    '',
  ]) assert.ok(!asksToBuild(ask), ask);
});

test('the Ask prompt says it cannot build pages or apps, and never claims it did', () => {
  const profile = readFileSync(join(appDir, '..', 'crates', 'shodh-rag', 'src', 'harness', 'profile.rs'), 'utf8');
  assert.match(profile, /- You cannot build or save pages, apps, games or other files in this chat: only Creator mode \\\ncan, and the app offers it under your answer\./);
  assert.match(profile, /never say or suggest that you built, saved or opened one\./);
});

test('an Ask answer to a build request carries the card; a Creator answer does not', () => {
  const ask = withoutComments(source('features/ask/AskView.tsx'));
  assert.match(ask, /message\.transcript && !isLive\(message\.transcript\) && message\.transcript\.agentMode !== 'creator' && asksToBuild\(asked\)/);
  assert.match(ask, /<BuildInCreatorCard codeAvailable=\{codeModeEnabled\} onBuild=\{\(\) => buildInCreator\(asked\)\} onOpenSettings=\{openSettings\} \/>/);
  // A new chat, Creator confirmed and on, the user's words in its message box.
  assert.match(ask, /const id = session\.createConversation\(\);\s*confirmCreator\(id\);\s*session\.updateConversationMeta\(id, \{ agentMode: 'creator' \}\);\s*composerDrafts\.set\(id, request\);/);
  const card = withoutComments(source('features/ask/BuildInCreatorCard.tsx'));
  assert.match(card, /onClick=\{codeAvailable \? onBuild : onOpenSettings\}/);
  assert.match(card, /data-testid="build-in-creator"/);
});

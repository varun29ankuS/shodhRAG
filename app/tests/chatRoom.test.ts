/**
 * The launch test's measure of the sidebar's chats (features/launch/chatRoom.ts): the rule
 * that counts whole rows, and that every view of the tour reports it and fails on too few.
 * The measuring runs in the built app (the Windows launch job); here are its rule and wiring.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/chatRoom.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { wholeRows } from '../src/features/launch/chatRoom.ts';
import { appDir, source, withoutComments } from './jsxScan.ts';

test('a row is whole only when all of it is inside what its boxes show', () => {
  const clip = { top: 100, bottom: 200 };
  assert.equal(wholeRows([], clip), 0);
  assert.equal(wholeRows([{ top: 100, bottom: 130 }, { top: 131, bottom: 161 }, { top: 162, bottom: 192 }], clip), 3);
  // Cut at the bottom by one pixel, or above the top: not whole.
  assert.equal(wholeRows([{ top: 170, bottom: 201 }], clip), 0);
  assert.equal(wholeRows([{ top: 99, bottom: 120 }], clip), 0);
  // Exactly at the edges: whole.
  assert.equal(wholeRows([{ top: 170, bottom: 200 }], clip), 1);
});

test('the sidebar marks each chat row, and every view reports them', () => {
  assert.match(source('components/shell/ConversationHistory.tsx'), /<li ref=\{rowRef\} data-chat-row="" /);
  const room = withoutComments(source('features/launch/chatRoom.ts'));
  assert.match(room, /querySelectorAll<HTMLElement>\('\[data-chat-row\]'\)/);
  // Every scrolling or hiding box the rows are in cuts them, and the window does.
  assert.match(room, /if \(overflowY === 'visible'\) continue;/);
  assert.match(room, /let clip: Span = \{ top: 0, bottom: view\.innerHeight \};/);
  const host = withoutComments(source('features/launch/LaunchProbeHost.tsx'));
  assert.match(host, /chats: chatRoom\(document\),/);

  const probe = readFileSync(join(appDir, 'src-tauri', 'src', 'launch_probe.rs'), 'utf8');
  assert.match(probe, /pub const MIN_CHAT_ROWS: u32 = 3;/);
  assert.match(probe, /chats\.listed > 0 && chats\.whole >= MIN_CHAT_ROWS\.min\(chats\.listed\)/);
  // Checked in each view of the app (a scene folds the sidebar), and failed after the tour.
  assert.match(probe, /if !scene && !chats_have_room\(fit\.chats\) \{/);
  assert.match(probe, /if chats_short > 0 \{\s+return Err\(/);
});

test('a short window tightens the sidebar so the chats keep room', () => {
  const config = readFileSync(join(appDir, 'tailwind.config.js'), 'utf8');
  assert.match(config, /short: \{ raw: '\(max-height: 800px\)' \}/);
  const sidebar = source('components/shell/Sidebar.tsx');
  // The views' rows, the New chat row and the footer.
  assert.match(sidebar, /'flex items-center h-9 short:h-7 rounded-lg text-body/);
  assert.match(sidebar, /'flex items-center gap-2 h-9 short:h-8 text-small font-semibold/);
  assert.match(sidebar, /'flex flex-col gap-0\.5 short:gap-0 py-1 shrink-0'/);
  assert.match(sidebar, /'shrink-0 border-t border-shodh-border flex flex-col gap-2 short:gap-1'/);
});

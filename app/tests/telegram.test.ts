/**
 * Telegram page tests: the token check (the same rules as the Rust token type), the
 * wizard's step, the pill and next steps for every state, the link countdown, the QR code
 * drawing, and the page navigation an Inbox item uses. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/telegram.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  checkTelegramToken,
  countdown,
  groupPrivacyLine,
  guestSourceLine,
  ownerLine,
  qrPath,
  telegramNextSteps,
  telegramPill,
  telegramStep,
  telegramTabBadges,
  telegramTileAction,
  type TelegramStatus,
} from '../src/features/connectors/telegramModel.ts';
import { TELEGRAM_TABS, moveTab, parseConnectorTarget } from '../src/features/connectors/navigation.ts';

const TOKEN = '123456789:AAH4f9Zk2_x-QwErTyUiOpAsDfGhJkLzXcV';

function status(over: Partial<TelegramStatus> = {}): TelegramStatus {
  return {
    state: 'connected',
    bot: { username: 'shodh_notes_bot', name: 'Shodh', link: 'https://t.me/shodh_notes_bot', readsAllGroupMessages: false },
    error: null,
    nextRetryAt: null,
    retryReason: null,
    lastUpdateAt: null,
    owner: { id: '42', name: 'Ann Lee', username: 'annlee', linkedAt: '2026-10-27T09:00:00Z' },
    people: [],
    groups: [],
    answerWorkspace: null,
    answersToday: 0,
    dailyCap: 20,
    minDailyCap: 1,
    maxDailyCap: 1000,
    resetsAt: '2026-10-28T00:00:00Z',
    lastSendFailedAt: null,
    privacyNote: 'Telegram bot chats are not end-to-end encrypted.',
    ...over,
  };
}

test('the token check follows the Rust rules', () => {
  assert.deepEqual(checkTelegramToken(`  ${TOKEN} `), { ok: true, problem: null });
  assert.deepEqual(checkTelegramToken(''), { ok: false, problem: null }, 'nothing typed: no complaint yet');
  for (const bad of ['123456789', '123456789:short', 'abc:AAH4f9Zk2_x-QwErTyUiOpAsDfGhJkLzXcV', '0:AAH4f9Zk2_x-QwErTyUiOpAsDfGhJkLzXcV', `${TOKEN}V`, '123456789:AAH4f9Zk2_x-QwErTyUiOpAsDfGhJkLzXc!']) {
    const check = checkTelegramToken(bad);
    assert.equal(check.ok, false, bad);
    assert.ok(check.problem, bad);
    assert.ok(!check.problem.includes('AAH4'), 'never echoes the token');
  }
  // The Rust type is tested with the same token.
  const rust = readFileSync(new URL('../../crates/shodh-rag/src/connectors/telegram/token.rs', import.meta.url), 'utf8');
  assert.ok(rust.includes(TOKEN));
  assert.match(rust, /pub const SECRET_CHARS: usize = 35;/);
});

test('the wizard shows the token step until a bot is connected, then linking', () => {
  assert.equal(telegramStep({ bot: null, owner: null }, 1), 1);
  assert.equal(telegramStep({ bot: null, owner: null }, 2), 2);
  assert.equal(telegramStep({ bot: null, owner: null }, 3), 2, 'no linking without a bot');
  assert.equal(telegramStep(status({ owner: null }), 1), 3);
});

test('the pill says what the connector is doing and what needs the user', () => {
  assert.equal(telegramPill(status({ state: 'not_set_up', bot: null, owner: null })).label, 'Not connected');
  assert.deepEqual(
    [telegramPill(status({ state: 'local_only' })).label, telegramPill(status({ state: 'local_only' })).reason],
    ['Off', 'Local-only mode'],
  );
  assert.equal(telegramPill(status({ state: 'reconnecting' })).label, 'Connecting…');
  assert.equal(telegramPill(status({ state: 'error' })).reason, 'Connection stopped');
  assert.equal(telegramPill(status()).label, 'Connected');
  assert.equal(telegramPill(status()).team, '@shodh_notes_bot');
  assert.equal(telegramPill(status({ owner: null })).reason, 'Not linked to you yet');
  assert.equal(telegramPill(status({ lastSendFailedAt: '2026-10-27T09:00:00Z' })).reason, 'An answer was not sent');
  const waiting = { id: '-100', title: 'Deals', allowed: false, addedBy: 'Bob', seenAt: '2026-10-27T09:00:00Z' };
  assert.equal(telegramPill(status({ groups: [waiting] })).reason, 'A group is waiting');
  assert.equal(telegramTileAction(status({ bot: null })), 'Connect');
  assert.equal(telegramTileAction(status()), 'Manage');
});

test('next steps: link, waiting groups, and a workspace once others can ask', () => {
  assert.deepEqual(telegramNextSteps(status()).map(s => s.id), [], 'set up');
  assert.deepEqual(telegramNextSteps(status({ owner: null })).map(s => s.id), ['link']);
  const waiting = { id: '-100', title: 'Deals', allowed: false, addedBy: null, seenAt: '2026-10-27T09:00:00Z' };
  const steps = telegramNextSteps(status({ groups: [waiting] }));
  assert.deepEqual(steps.map(s => s.id), ['groups']);
  assert.equal(steps[0].title, '“Deals” wants answers');
  assert.equal(steps[0].tab, 'people');
  const person = { id: '77', name: 'Bob', username: null, linkedAt: '2026-10-27T09:00:00Z' };
  assert.deepEqual(telegramNextSteps(status({ people: [person] })).map(s => s.id), ['workspace']);
  assert.deepEqual(
    telegramNextSteps(status({ people: [person], answerWorkspace: { id: 'w', name: 'Clients', available: true } })).map(s => s.id),
    [],
  );
  assert.deepEqual(telegramNextSteps(status({ state: 'local_only' })).map(s => s.id), ['local-only']);
  assert.deepEqual(telegramTabBadges(status({ groups: [waiting] })), { people: { count: 1, tone: 'warning' } });
});

test('what others get is said plainly: one workspace, never memories', () => {
  assert.match(guestSourceLine({ answerWorkspace: { id: 'w', name: 'Clients', available: true } }).text, /never your memories/);
  assert.equal(guestSourceLine({ answerWorkspace: null }).tone, 'warning');
  assert.equal(ownerLine({ owner: null }), 'Not linked yet: nobody can use this bot as you.');
  assert.equal(ownerLine(status()), 'Linked to @annlee');
  assert.match(groupPrivacyLine(status()), /\/ask@shodh_notes_bot/);
});

test('a link counts down and then says it expired', () => {
  const at = Date.parse('2026-10-27T09:00:00Z');
  assert.equal(countdown('2026-10-27T09:10:00Z', at), '10:00');
  assert.equal(countdown('2026-10-27T09:10:00Z', at + 59_000), '9:01');
  assert.equal(countdown('2026-10-27T09:10:00Z', at + 600_000), null);
  assert.equal(countdown('not a time', at), null);
});

test('the QR code is drawn from its modules with a quiet zone', () => {
  const { path, size } = qrPath(['110', '000', '011']);
  assert.equal(size, 3 + 8);
  assert.equal(path, 'M4 4h2v1h-2zM5 6h2v1h-2z');
});

test('an Inbox item opens the Telegram page at its tab; the tabs move with the keys', () => {
  assert.deepEqual(parseConnectorTarget({ connector: 'telegram', tab: 'people' }), { connector: 'telegram', tab: 'people' });
  assert.deepEqual(parseConnectorTarget({ connector: 'telegram', tab: 'indexing' }), { connector: 'telegram', tab: null });
  assert.equal(moveTab('overview', 'ArrowRight', TELEGRAM_TABS), 'people');
  assert.equal(moveTab('overview', 'ArrowLeft', TELEGRAM_TABS), 'settings');
  assert.equal(moveTab('people', 'End', TELEGRAM_TABS), 'settings');
  // The Rust side links the same tabs.
  const rust = readFileSync(new URL('../src-tauri/src/connectors/telegram_notices.rs', import.meta.url), 'utf8');
  const named = [...rust.matchAll(/TelegramTab::[A-Za-z]+ => "([a-z]+)"/g)].map(m => m[1]);
  assert.deepEqual(named.sort(), [...TELEGRAM_TABS].sort());
  assert.match(rust, /"connector": "telegram", "tab": tab\.as_str\(\)/);
});

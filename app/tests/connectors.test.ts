/**
 * Connectors view and Slack card tests: the card's wording for every connection state, the
 * day's answers, the setup steps and token checks (the same rules as the Rust token types),
 * and the new view in the navigation lists of both the shell and the agent. Run with Node
 * 22.6+:
 *   node --experimental-strip-types --test app/tests/connectors.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  MODEL_PRIVACY_NOTE,
  NOT_ALLOWED_REPLY,
  RUNNING_HINT,
  answerPlacesLine,
  answerSourceLine,
  canConnect,
  channelLabel,
  exposureLine,
  slackWarnings,
  checkAppToken,
  checkBotToken,
  lastEventLine,
  setupSteps,
  stateLine,
  tokenHint,
  usageLine,
  validCap,
  type SlackState,
  type SlackStatus,
} from '../src/features/connectors/slackModel.ts';
import { CONNECTORS } from '../src/features/connectors/registry.ts';
import { VIEW_TABS, VIEW_TAB_LABELS, normalizeViewTab } from '../src/lib/viewTabs.ts';
import { navigationFromEvent } from '../src/features/agent/navigation.ts';
import type { NavigatedEvent } from '../src/features/agent/events.ts';

const UTC = { locale: 'en-GB', timeZone: 'UTC' };
const NOW = Date.UTC(2026, 9, 23, 9, 0);

function status(state: SlackState, patch: Partial<SlackStatus> = {}): SlackStatus {
  return {
    state,
    installation: state === 'not_set_up' ? null : { teamName: 'Acme', botName: 'shodh' },
    error: null,
    nextRetryAt: null,
    retryReason: null,
    lastEventAt: null,
    answersToday: 3,
    dailyCap: 20,
    minDailyCap: 1,
    maxDailyCap: 1000,
    resetsAt: '2026-10-24T00:00:00Z',
    answerWorkspace: null,
    answerChannels: [],
    otherConnections: false,
    lastPostFailedAt: null,
    missingScopes: [],
    createAppUrl: 'https://api.slack.com/apps?new_app=1&manifest_json=%7B%7D',
    botScopes: [
      'app_mentions:read',
      'channels:read',
      'chat:write',
      'groups:read',
      'im:history',
      'im:read',
      'im:write',
      'users:read',
    ],
    botEvents: ['app_mention', 'message.im'],
    appTokenScope: 'connections:write',
    ...patch,
  };
}

test('every state has its own headline', () => {
  assert.deepEqual(stateLine(status('not_set_up'), NOW, UTC), {
    tone: 'neutral',
    title: 'Not connected',
    detail: 'Create a Slack app for Shodh, then paste its two tokens.',
  });
  assert.equal(stateLine(status('off'), NOW, UTC).title, 'Off');
  assert.match(stateLine(status('off'), NOW, UTC).detail ?? '', /Connected to Acme as @shodh, but switched off/);
  assert.equal(stateLine(status('connecting'), NOW, UTC).title, 'Connecting to Acme…');
  assert.deepEqual(stateLine(status('connected'), NOW, UTC), {
    tone: 'ok',
    title: 'Connected to Acme as @shodh',
    detail: null,
  });
  const reconnecting = stateLine(
    status('reconnecting', { nextRetryAt: '2026-10-23T09:05:00Z', retryReason: 'Slack closed the connection' }),
    NOW,
    UTC,
  );
  assert.equal(reconnecting.tone, 'warning');
  assert.match(reconnecting.detail ?? '', /^Next try at 0?9:05\. The connection dropped: Slack closed the connection\.$/);
  const missing = 'The Slack app is missing these permissions: im:history, im:write.';
  assert.deepEqual(stateLine(status('error', { error: missing }), NOW, UTC), {
    tone: 'error',
    title: 'Stopped',
    detail: missing,
  });
  assert.equal(stateLine(status('local_only'), NOW, UTC).title, 'Paused: Local-only mode is on');
  // A refusal while connected (a scope removed later) is a warning, with its reason.
  const warned = stateLine(status('connected', { error: missing }), NOW, UTC);
  assert.equal(warned.tone, 'warning');
  assert.equal(warned.detail, missing);
});

test('the day’s answers, the last event and the running hint are said plainly', () => {
  assert.match(usageLine(status('connected'), NOW, UTC), /^Slack answers today: 3 of 20; resets at 0?0:00 tomorrow\.$/);
  assert.equal(lastEventLine(status('connected'), NOW, UTC), 'No Slack events since Shodh started.');
  assert.match(
    lastEventLine(status('connected', { lastEventAt: '2026-10-23T08:58:00Z' }), NOW, UTC),
    /^Last Slack event: 0?8:58\.$/,
  );
  assert.match(RUNNING_HINT, /must be running/);
  assert.match(MODEL_PRIVACY_NOTE, /openrouter\.ai\/settings\/privacy/);
});

test('the answer workspace line says what answers come from, and what happens without one', () => {
  assert.match(answerSourceLine(status('connected')).text, /isn’t set up to answer here yet/);
  assert.equal(answerSourceLine(status('connected')).tone, 'warning');
  const gone = answerSourceLine(status('connected', { answerWorkspace: { id: 'w1', name: '', available: false } }));
  assert.match(gone.text, /deleted or archived/);
  const chosen = answerSourceLine(status('connected', { answerWorkspace: { id: 'w1', name: 'Research', available: true } }));
  assert.equal(chosen.tone, 'ok');
  assert.match(chosen.text, /only from the documents of “Research”/);
  assert.match(chosen.text, /memories, notes, tasks, chats/);
});

test('the card says where Shodh answers, and that it answers nowhere until channels are chosen', () => {
  const none = answerPlacesLine(status('connected'));
  assert.equal(none.tone, 'warning');
  assert.match(none.text, /no channel yet/);
  assert.ok(none.text.includes(NOT_ALLOWED_REPLY));
  assert.match(none.text, /only for members of Acme: not guests, and not people from other organizations/);
  const some = answerPlacesLine(
    status('connected', {
      answerChannels: [
        { id: 'C1', name: 'research', shared: false, nowShared: false },
      ],
    }),
  );
  assert.equal(some.tone, 'ok');
  assert.match(some.text, /answers mentions in #research;/);
  const shared = answerPlacesLine(
    status('connected', {
      answerChannels: [
        { id: 'C1', name: 'research', shared: false, nowShared: false },
        { id: 'C2', name: 'client-a', shared: true, nowShared: false },
      ],
    }),
  );
  assert.equal(shared.tone, 'warning');
  assert.match(shared.text, /#client-a · shared with other organizations/);
  assert.match(shared.text, /People from other organizations in a shared channel can ask too/);
  assert.equal(channelLabel({ name: 'board', private: true, shared: true }), '#board · private · shared with other organizations');
  assert.equal(channelLabel({ name: 'general', shared: false }), '#general');
  assert.equal(NOT_ALLOWED_REPLY, 'Shodh isn’t set to answer in this channel.');
});

test('missing scopes, another connection, a failed post and a newly shared channel are warned about', () => {
  assert.deepEqual(slackWarnings(status('connected'), NOW, UTC), []);
  const warnings = slackWarnings(
    status('connected', {
      missingScopes: ['channels:read', 'groups:read', 'users:read'],
      otherConnections: true,
      lastPostFailedAt: '2026-10-23T08:59:00Z',
      answerChannels: [{ id: 'C1', name: 'research', shared: false, nowShared: true }],
    }),
    NOW,
    UTC,
  );
  assert.equal(warnings.length, 4);
  assert.match(warnings[0], /missing these permissions: channels:read, groups:read, users:read/);
  assert.match(warnings[1], /Another Shodh is connected to this Slack app/);
  assert.match(warnings[2], /^The last answer failed to post to Slack \(0?8:59\)/);
  assert.match(warnings[3], /#research is now shared with other organizations/);
});

test('choosing a workspace says what people in Slack will be able to read', () => {
  assert.equal(
    exposureLine('Acme', 'Client A', [{ kind: 'folder' }, { kind: 'file' }, { kind: 'paper' }, { kind: 'snippet' }]),
    'People in Acme will get answers from 1 folder and 2 files in “Client A”, in the channels chosen below and in DMs.',
  );
  assert.match(exposureLine('Acme', 'Empty', []), /0 folders and 0 files/);
});

test('tokens are checked like the Rust token types', () => {
  assert.deepEqual(checkAppToken(' xapp-1-A1-2-abc '), { ok: true, message: null });
  assert.deepEqual(checkBotToken('xoxb-1-2-abc'), { ok: true, message: null });
  assert.match((checkBotToken('xoxp-1-2-3') as { message: string }).message, /user token/);
  assert.match((checkAppToken('xoxp-1-2-3') as { message: string }).message, /user token/);
  assert.match((checkBotToken('xoxe.xoxb-1-abc') as { message: string }).message, /rotating/);
  assert.match((checkAppToken('xoxb-1-2-abc') as { message: string }).message, /looks like the bot token/);
  assert.match((checkBotToken('xapp-1-abc') as { message: string }).message, /looks like the app-level token/);
  assert.match((checkBotToken('xoxb-12 34') as { message: string }).message, /spaces or characters/);
  assert.match((checkBotToken('xoxb-') as { message: string }).message, /starts with xoxb-/);
  assert.equal(tokenHint('', checkBotToken), null, 'nothing is said before a paste');
  assert.equal(tokenHint('xoxb-1-2-abc', checkBotToken), null);
  assert.ok(tokenHint('nope', checkBotToken));
  assert.equal(canConnect('xapp-1-a', 'xoxb-1-b'), true);
  assert.equal(canConnect('xapp-1-a', 'xoxp-1-b'), false);
  assert.equal(canConnect('', 'xoxb-1-b'), false);
});

test('the setup steps name the scopes the manifest asks for', () => {
  const steps = setupSteps(status('not_set_up'));
  assert.deepEqual(
    steps.map(s => s.title),
    ['Create the Slack app', 'Create the app-level token', 'Install the app', 'Paste both tokens'],
  );
  assert.match(
    steps[0].detail,
    /app_mentions:read, channels:read, chat:write, groups:read, im:history, im:read, im:write, users:read/,
  );
  assert.match(steps[1].detail, /connections:write/);
  assert.match(steps[2].detail, /xoxb-/);
});

test('the daily cap accepts whole numbers in range only', () => {
  const s = status('connected');
  assert.equal(validCap('20', s), 20);
  assert.equal(validCap(' 1 ', s), 1);
  assert.equal(validCap('0', s), null);
  assert.equal(validCap('1001', s), null);
  assert.equal(validCap('2.5', s), null);
  assert.equal(validCap('', s), null);
});

test('the Connectors view sits after Library and lists Slack', () => {
  const library = VIEW_TABS.indexOf('library');
  assert.equal(VIEW_TABS[library + 1], 'connectors');
  assert.equal(VIEW_TAB_LABELS.connectors, 'Connectors');
  assert.equal(normalizeViewTab('connectors'), 'connectors');
  assert.deepEqual(CONNECTORS.map(c => c.id), ['slack']);
});

test('the agent can open the Connectors view, and its list matches the shell', () => {
  const event = { type: 'navigated', runId: 'r', view: 'connectors', focus: null, target: null } as unknown as NavigatedEvent;
  assert.equal(navigationFromEvent(event).view, 'connectors');
  const rust = readFileSync(new URL('../../crates/shodh-rag/src/harness/tools/navigate.rs', import.meta.url), 'utf8');
  const block = rust.match(/pub const VIEWS: \[&str; (\d+)\] = \[([^\]]*)\]/);
  assert.ok(block, 'VIEWS in navigate.rs');
  const views = [...block[2].matchAll(/"([a-z]+)"/g)].map(m => m[1]);
  assert.deepEqual(views, [...VIEW_TABS]);
  assert.equal(Number(block[1]), VIEW_TABS.length);
});

test('the Slack commands and event names match the Rust side', () => {
  const rust = readFileSync(new URL('../src-tauri/src/connectors/slack.rs', import.meta.url), 'utf8');
  assert.match(rust, /pub const SLACK_STATUS_EVENT: &str = "connectors-slack-status";/);
  const api = readFileSync(new URL('../src/features/connectors/api.ts', import.meta.url), 'utf8');
  for (const command of [...api.matchAll(/invoke<[^>]+>\('([a-z_]+)'/g)].map(m => m[1])) {
    assert.match(rust, new RegExp(`pub async fn ${command}\\(`), command);
  }
});

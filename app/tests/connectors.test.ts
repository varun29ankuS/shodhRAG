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
  INDEX_PRIVACY_NOTE,
  durationWords,
  indexChannelLine,
  memoriesCountLine,
  memoryPlace,
  memoryProvenance,
  ownerLine,
  sharedIndexLine,
  validWindow,
  type IndexChannelView,
  type IndexStatus,
  type SlackMemoryView,
  type SlackState,
  type SlackStatus,
} from '../src/features/connectors/slackModel.ts';
import { passagesFromDetail, slackPermalink } from '../src/features/agent/reducer.ts';
import { CONNECTORS } from '../src/features/connectors/registry.ts';
import { VIEW_TABS, VIEW_TAB_LABELS, normalizeViewTab } from '../src/lib/viewTabs.ts';
import { navigationFromEvent } from '../src/features/agent/navigation.ts';
import type { NavigatedEvent } from '../src/features/agent/events.ts';

const UTC = { locale: 'en-GB', timeZone: 'UTC' };
const NOW = Date.UTC(2026, 9, 23, 9, 0);

function indexStatus(patch: Partial<IndexStatus> = {}): IndexStatus {
  return {
    available: true,
    channels: [],
    windowDays: 90,
    minWindowDays: 7,
    maxWindowDays: 365,
    missingScopes: [],
    missingScopesMessage: null,
    memories: 0,
    owner: null,
    costNote: 'Indexing runs on this computer: no model calls, no answers spent.',
    ...patch,
  };
}

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
    index: indexStatus(),
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

// ── Indexed channels and memories from Slack ────────────────────────

function indexed(patch: Partial<IndexChannelView> = {}): IndexChannelView {
  return {
    id: 'C1',
    name: 'deals',
    shared: false,
    private: false,
    workspace: { id: 'w1', name: 'Client A', available: true },
    state: 'backfilling',
    detail: null,
    threads: 120,
    estimatedThreads: 360,
    secondsLeft: 312,
    lastSyncAt: null,
    pausedUntil: null,
    windowDays: 90,
    ...patch,
  };
}

test('an indexed channel says where its sync stands', () => {
  assert.deepEqual(indexChannelLine(indexed(), NOW, UTC), {
    tone: 'active',
    title: 'Backfilling: 120 of ~360 threads',
    detail: 'about 5 minutes left at Slack’s pace. Reading the last 90 days; it carries on where it stopped if Shodh closes.',
  });
  assert.equal(
    indexChannelLine(indexed({ estimatedThreads: null, secondsLeft: null, threads: 1 }), NOW, UTC).title,
    'Backfilling: 1 thread so far',
  );
  const upToDate = indexChannelLine(indexed({ state: 'up_to_date', lastSyncAt: '2026-10-23T08:45:00Z' }), NOW, UTC);
  assert.equal(upToDate.tone, 'ok');
  assert.equal(upToDate.title, 'Up to date: 120 threads');
  assert.match(upToDate.detail ?? '', /^Last sync 0?8:45\.$/);
  const paused = indexChannelLine(
    indexed({ state: 'paused', pausedUntil: '2026-10-23T09:01:00Z', detail: "Slack's rate limit" }),
    NOW,
    UTC,
  );
  assert.match(paused.title, /^Paused: rate limit until 0?9:01$/);
  assert.equal(paused.tone, 'warning');
  const failing = indexChannelLine(
    indexed({ state: 'paused', pausedUntil: '2026-10-23T09:05:00Z', detail: 'Slack could not be read (reset); trying again shortly.' }),
    NOW,
    UTC,
  );
  assert.match(failing.title, /^Paused until 0?9:05$/);
  assert.match(failing.detail ?? '', /could not be read/);
  assert.deepEqual(indexChannelLine(indexed({ state: 'error', detail: 'Slack refused the bot token (invalid_auth).' }), NOW, UTC), {
    tone: 'error',
    title: 'Error',
    detail: 'Slack refused the bot token (invalid_auth).',
  });
  for (const [state, title] of [
    ['archived', 'Archived in Slack'],
    ['removed', 'Shodh was removed from the channel'],
    ['now_shared', 'Shared with other organizations now'],
    ['forgetting', 'Forgetting…'],
  ] as const) {
    assert.equal(indexChannelLine(indexed({ state }), NOW, UTC).title, title);
  }
});

test('durations, the window and the shared-channel confirmation read plainly', () => {
  assert.equal(durationWords(20), 'less than a minute');
  assert.equal(durationWords(60), 'about 1 minute');
  assert.equal(durationWords(3_000), 'about 50 minutes');
  assert.equal(durationWords(7_200), 'about 2 hours');
  const index = indexStatus();
  assert.equal(validWindow('30', index), 30);
  assert.equal(validWindow('3', index), null);
  assert.equal(validWindow('400', index), null);
  assert.equal(validWindow('9.5', index), null);
  assert.match(sharedIndexLine({ name: 'client-b' }), /#client-b is shared with other organizations\. Indexing it stores their messages on this computer/);
  assert.match(INDEX_PRIVACY_NOTE, /only the threads of the channel it is asked in/);
  assert.match(INDEX_PRIVACY_NOTE, /client’s workspace/);
});

function memory(patch: Partial<SlackMemoryView> = {}): SlackMemoryView {
  return {
    id: 'm1',
    channelName: '#deals',
    authorName: 'Bob',
    text: 'Standup moved to 10:30.',
    kind: 'text',
    permalink: 'https://acme.slack.com/archives/C1/p1',
    savedAt: '2026-10-23T08:00:00Z',
    owner: false,
    inMemories: false,
    ...patch,
  };
}

test('memories from Slack say who saved them, where, and where they live', () => {
  assert.equal(memoryProvenance(memory(), UTC), 'Saved by Bob in #deals, 23 Oct');
  assert.equal(memoryProvenance(memory({ kind: 'thread' }), UTC), 'Thread saved by Bob in #deals, 23 Oct');
  assert.equal(memoryPlace(memory()), 'For #deals only');
  assert.equal(memoryPlace(memory({ inMemories: true })), 'For #deals, and in your memories');
  assert.equal(memoryPlace(memory({ owner: true, inMemories: true })), 'In your memories');
  assert.match(memoriesCountLine({ memories: 0 }), /Nothing saved from Slack yet/);
  assert.equal(memoriesCountLine({ memories: 1 }), '1 memory saved from Slack.');
  assert.equal(memoriesCountLine({ memories: 4 }), '4 memories saved from Slack.');
  assert.match(ownerLine({ owner: null }), /Until then, everything saved from Slack is kept for its channel only/);
  assert.match(ownerLine({ owner: { userId: 'U1', name: 'Ann' } }), /You are Ann in Slack/);
});

test('a Slack citation in Ask opens its permalink, never a path', () => {
  const [slack, file] = passagesFromDetail({
    passages: [
      { n: 1, file: '#deals: budget', path: 'slack://T1/C1/1.1', text: 'ten', url: 'https://acme.slack.com/archives/C1/p11' },
      { n: 2, file: 'lease.pdf', path: 'c:/docs/lease.pdf', text: 'sixty', url: 'https://evil.example/x' },
    ],
  });
  assert.equal(slack.url, 'https://acme.slack.com/archives/C1/p11');
  assert.equal(file.url, undefined, 'a file passage never carries a link');
  assert.equal(slackPermalink('slack://T1/C1/1.1', 'javascript:alert(1)'), null);
  assert.equal(slackPermalink('slack://T1/C1/1.1', 'http://acme.slack.com/x'), null);
  assert.equal(slackPermalink('note://n1', 'https://acme.slack.com/x'), null);
});

/**
 * Connectors view and Slack page tests: the wording for every connection state, the day's
 * answers, the setup wizard and token checks (the same rules as the Rust token types), the
 * tile's pill, the page's next steps and tab badges, the page navigation an Inbox item
 * uses, and the view in the navigation lists of both the shell and the agent. Run with Node
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
  wizardSteps,
  canLeaveStep,
  reachableStep,
  stepToFix,
  scopeWords,
  slackPill,
  tileAction,
  capShare,
  nextSteps,
  slackTabBadges,
  overviewWarnings,
  indexSummary,
  channelNeedsAttention,
  isRateLimited,
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
  UPDATE_APP_LINE,
  UPDATE_APP_STEPS,
  appManifestUrl,
  manifestFromCreateUrl,
  stillMissingLine,
  type IndexChannelView,
  type IndexStatus,
  type SlackMemoryView,
  type SlackState,
  type SlackStatus,
} from '../src/features/connectors/slackModel.ts';
import { passagesFromDetail, slackPermalink } from '../src/features/agent/reducer.ts';
import { CONNECTORS } from '../src/features/connectors/registry.ts';
import {
  SLACK_TABS,
  SLACK_TAB_LABELS,
  escapeGoesBack,
  lastConnectorPage,
  moveTab,
  parseConnectorTarget,
  rememberConnectorPage,
  requestConnectorPage,
  takeRequestedConnectorPage,
} from '../src/features/connectors/navigation.ts';
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
    installation: state === 'not_set_up' ? null : { teamName: 'Acme', botName: 'shodh', appId: 'A0123ABCD' },
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

test('the setup wizard has four steps, one per thing to do in Slack', () => {
  const steps = wizardSteps(status('not_set_up'));
  assert.deepEqual(
    steps.map(s => [s.step, s.title]),
    [
      [1, 'Create the Shodh app in Slack'],
      [2, 'Create the app-level token'],
      [3, 'Install to your workspace'],
      [4, 'Connect'],
    ],
  );
  assert.match(steps[1].detail, /App-Level Tokens/);
  assert.match(steps[1].detail, /connections:write/);
  assert.match(steps[2].detail, /Bot User OAuth Token/);
  assert.match(steps[3].detail, /credential store/);
});

test('every permission the manifest asks for is said in plain words', () => {
  const rust = readFileSync(new URL('../../crates/shodh-rag/src/connectors/slack/setup.rs', import.meta.url), 'utf8');
  const scopes: string[] = [];
  for (const name of ['REQUIRED_BOT_SCOPES', 'INDEX_BOT_SCOPES']) {
    const block = rust.match(new RegExp(`pub const ${name}: \\[&str; \\d+\\] = \\[([^\\]]*)\\]`));
    assert.ok(block, name);
    scopes.push(...[...block[1].matchAll(/"([a-z_.:]+)"/g)].map(m => m[1]));
  }
  assert.ok(scopes.length >= 10, scopes.join(', '));
  for (const scope of scopes) {
    assert.notEqual(scopeWords(scope), scope, `${scope} has no plain words`);
    assert.doesNotMatch(scopeWords(scope), /:/, `${scope} is said without its id`);
  }
  assert.equal(scopeWords('admin:everything'), 'admin:everything', 'an unknown scope is shown as it is');
});

test('the wizard moves on only with a token that looks right, and never past a missing one', () => {
  assert.equal(canLeaveStep(1, '', ''), true);
  assert.equal(canLeaveStep(2, '', ''), false);
  assert.equal(canLeaveStep(2, 'xoxb-1-2', ''), false, 'the bot token in the app-level field');
  assert.equal(canLeaveStep(2, 'xapp-1-a', ''), true);
  assert.equal(canLeaveStep(3, 'xapp-1-a', 'xoxp-1-b'), false, 'a user token');
  assert.equal(canLeaveStep(3, 'xapp-1-a', 'xoxb-1-b'), true);
  assert.equal(canLeaveStep(4, 'xapp-1-a', 'xoxb-1-b'), true);
  assert.equal(canLeaveStep(4, '', 'xoxb-1-b'), false);
  // The tokens live in memory only: a step past a missing one falls back to it.
  assert.equal(reachableStep(4, '', ''), 2);
  assert.equal(reachableStep(4, 'xapp-1-a', ''), 3);
  assert.equal(reachableStep(3, 'xapp-1-a', ''), 3);
  assert.equal(reachableStep(4, 'xapp-1-a', 'xoxb-1-b'), 4);
  assert.equal(reachableStep(1, '', ''), 1);
  // What Slack refused points at the step that fixes it.
  assert.equal(stepToFix('Slack refused the app-level token (invalid_auth). Check that the whole token was copied.'), 2);
  assert.equal(stepToFix('Slack refused the bot token (token_revoked). Check that the whole token was copied.'), 3);
  assert.equal(stepToFix('The app-level token and the bot token belong to different Slack apps, so Shodh would never see the questions.'), 2);
  assert.equal(stepToFix('error sending request. Check the internet connection and try again.'), 4);
});

test('the tile’s pill says the state in a word or two, with the reason when Slack needs you', () => {
  assert.deepEqual(slackPill(status('not_set_up')), { kind: 'not_connected', tone: 'neutral', label: 'Not connected', reason: null, team: null });
  assert.equal(tileAction(status('not_set_up')), 'Connect');
  assert.equal(tileAction(status('connected')), 'Manage');
  assert.deepEqual(slackPill(status('connected')), { kind: 'connected', tone: 'ok', label: 'Connected', reason: null, team: 'Acme' });
  assert.equal(slackPill(status('connecting')).label, 'Connecting…');
  assert.equal(slackPill(status('reconnecting')).kind, 'connecting');
  assert.deepEqual(slackPill(status('off')), { kind: 'off', tone: 'neutral', label: 'Off', reason: null, team: 'Acme' });
  assert.equal(slackPill(status('local_only')).reason, 'Local-only mode');
  assert.deepEqual(slackPill(status('error')), { kind: 'attention', tone: 'error', label: 'Needs attention', reason: 'Connection stopped', team: 'Acme' });
  const attention = (patch: Partial<SlackStatus>) => slackPill(status('connected', patch));
  assert.equal(attention({ missingScopes: ['users:read'] }).reason, 'Missing permissions');
  assert.equal(attention({ index: indexStatus({ missingScopes: ['channels:history'] }) }).reason, 'Missing permissions');
  assert.equal(attention({ error: 'Slack refused the bot token.' }).reason, 'Slack refused a reply');
  assert.equal(attention({ otherConnections: true }).reason, 'Another Shodh is connected');
  assert.equal(attention({ lastPostFailedAt: '2026-10-23T08:59:00Z' }).reason, 'An answer was not posted');
  assert.equal(attention({ index: indexStatus({ channels: [indexed({ state: 'removed' })] }) }).reason, 'A channel stopped');
  assert.equal(attention({ answerChannels: [{ id: 'C1', name: 'r', shared: true, nowShared: true }] }).reason, 'A channel is shared now');
  const limited = attention({ index: indexStatus({ channels: [indexed({ state: 'paused', detail: 'Slack’s rate limit' })] }) });
  assert.deepEqual([limited.kind, limited.tone, limited.reason], ['attention', 'warning', 'Rate-limited by Slack']);
  // Missing permissions come first: they stop everything.
  assert.equal(attention({ missingScopes: ['users:read'], otherConnections: true }).reason, 'Missing permissions');
  // A pause that is not the rate limit, a backfill, an archive and a forget are not problems.
  for (const state of ['paused', 'backfilling', 'forgetting', 'archived', 'up_to_date'] as const) {
    const pill = attention({ index: indexStatus({ channels: [indexed({ state, detail: 'Slack could not be read (reset)' })] }) });
    assert.equal(pill.kind, 'connected', state);
  }
  assert.equal(isRateLimited({ state: 'paused', detail: "Slack's rate limit" }), true);
  assert.equal(isRateLimited({ state: 'error', detail: "Slack's rate limit" }), false);
  assert.equal(channelNeedsAttention({ state: 'now_shared' }), true);
  assert.equal(channelNeedsAttention({ state: 'archived' }), false);
});

test('the overview says what is left to set up, and where to do it', () => {
  const fresh = nextSteps(status('connected'));
  assert.deepEqual(fresh.map(s => [s.id, s.tab]), [['workspace', 'answering'], ['channels', 'answering'], ['owner', 'memories']]);
  const ready = status('connected', {
    answerWorkspace: { id: 'w1', name: 'Research', available: true },
    answerChannels: [{ id: 'C1', name: 'research', shared: false, nowShared: false }],
    index: indexStatus({ owner: { userId: 'U1', name: 'Ann' } }),
  });
  assert.deepEqual(nextSteps(ready), []);
  const scopes = nextSteps({ ...ready, missingScopes: ['users:read'], index: { ...ready.index, missingScopes: ['channels:history'] } });
  assert.equal(scopes[0].title, 'Update the Slack app');
  // A short line; the permission ids are their own field, shown behind a disclosure.
  assert.equal(scopes[0].detail, UPDATE_APP_LINE);
  assert.doesNotMatch(scopes[0].detail, /[a-z_]+:[a-z]+/);
  assert.deepEqual(scopes[0].permissions, ['channels:history', 'users:read']);
  assert.equal(scopes[0].tab, null);
  assert.equal(nextSteps({ ...ready, state: 'off' })[0].tab, 'answering');
  assert.equal(nextSteps({ ...ready, state: 'local_only' })[0].id, 'local_only');
  const gone = nextSteps({ ...ready, answerWorkspace: { id: 'w1', name: '', available: false } });
  assert.equal(gone[0].title, 'The answer workspace is gone');
  const stopped = nextSteps({ ...ready, index: { ...ready.index, channels: [indexed({ state: 'removed', name: 'deals' })] } });
  assert.deepEqual([stopped[0].title, stopped[0].tab], ['#deals stopped indexing', 'indexing']);
});

test('updating the Slack app opens its manifest page and copies the manifest the create link carries', () => {
  assert.equal(appManifestUrl('A0123ABCD'), 'https://api.slack.com/apps/A0123ABCD/app-manifest');
  // Unknown (an installation from before Shodh kept it) or not an app id: the list of apps.
  assert.equal(appManifestUrl(null), 'https://api.slack.com/apps');
  assert.equal(appManifestUrl(undefined), 'https://api.slack.com/apps');
  assert.equal(appManifestUrl('A1/../../evil'), 'https://api.slack.com/apps');
  assert.equal(appManifestUrl('T0123ABCD'), 'https://api.slack.com/apps');

  const manifest = {
    display_information: { name: 'Shodh' },
    oauth_config: { scopes: { bot: ['channels:history', 'chat:write'] } },
    settings: { event_subscriptions: { bot_events: ['app_mention', 'message.channels'] }, socket_mode_enabled: true },
  };
  const url = `https://api.slack.com/apps?new_app=1&manifest_json=${encodeURIComponent(JSON.stringify(manifest))}`;
  const copied = manifestFromCreateUrl(url);
  assert.equal(copied, JSON.stringify(manifest, null, 2));
  assert.deepEqual(JSON.parse(copied ?? ''), manifest);
  assert.ok((copied ?? '').split('\n').includes('  "oauth_config": {'), 'pretty-printed');
  assert.equal(manifestFromCreateUrl('https://api.slack.com/apps?new_app=1'), null);
  assert.equal(manifestFromCreateUrl('https://api.slack.com/apps?manifest_json=%7Bnot%20json'), null);
  assert.equal(manifestFromCreateUrl('https://api.slack.com/apps?manifest_json=%5B1%5D'), null);
  assert.equal(manifestFromCreateUrl('not a url'), null);

  assert.equal(UPDATE_APP_STEPS.length, 3);
  assert.match(UPDATE_APP_STEPS[0], /App Manifest/);
  assert.match(UPDATE_APP_STEPS[1], /reinstall/);
  assert.match(UPDATE_APP_STEPS[2], /Check again/);

  assert.equal(stillMissingLine(status('connected')), null);
  assert.match(stillMissingLine(status('connected', { missingScopes: ['users:read'] })) ?? '', /one permission/);
  assert.match(
    stillMissingLine(status('connected', { missingScopes: ['users:read'], index: indexStatus({ missingScopes: ['channels:history', 'users:read'] }) })) ?? '',
    /2 permissions/,
  );
});

test('the tabs carry what needs you and what they hold', () => {
  assert.deepEqual(slackTabBadges(status('connected'), NOW), { overview: { count: 2, tone: 'warning' } });
  const busy = status('connected', {
    answerWorkspace: { id: 'w1', name: 'Research', available: true },
    answerChannels: [{ id: 'C1', name: 'research', shared: false, nowShared: false }],
    index: indexStatus({
      owner: { userId: 'U1', name: 'Ann' },
      memories: 4,
      channels: [indexed({ id: 'C1', state: 'up_to_date' }), indexed({ id: 'C2', state: 'error' })],
    }),
  });
  assert.deepEqual(slackTabBadges(busy, NOW), {
    overview: { count: 1, tone: 'warning' },
    indexing: { count: 1, tone: 'warning' },
    memories: { count: 4, tone: 'neutral' },
  });
  // One problem, one count: missing permissions are a next step, not also a warning.
  const scopes = { ...busy, missingScopes: ['users:read'], index: { ...busy.index, channels: [] } };
  assert.deepEqual(slackTabBadges(scopes, NOW).overview, { count: 1, tone: 'warning' });
  assert.deepEqual(overviewWarnings(scopes, NOW), []);
  const shared = { ...scopes, missingScopes: [], answerChannels: [{ id: 'C1', name: 'research', shared: true, nowShared: true }] };
  assert.deepEqual(overviewWarnings(shared, NOW), []);
  assert.deepEqual(slackTabBadges(shared, NOW).overview, { count: 1, tone: 'warning' });
  const sharedStep = nextSteps(shared).find(s => s.id === 'now_shared:C1');
  assert.deepEqual([sharedStep?.title, sharedStep?.tab], ['#research is shared with other organizations now', 'answering']);
  assert.match(overviewWarnings({ ...shared, otherConnections: true }, NOW)[0], /Another Shodh is connected/);
  const calm = { ...busy, index: { ...busy.index, channels: [indexed({ state: 'up_to_date' })] } };
  assert.deepEqual(slackTabBadges(calm, NOW).indexing, { count: 1, tone: 'neutral' });
  assert.equal(slackTabBadges(calm, NOW).overview, undefined);
  assert.equal(indexSummary(indexStatus()), 'No channel indexed yet.');
  assert.equal(indexSummary(busy.index), '2 channels, 240 threads; 1 needs attention.');
  assert.equal(indexSummary(calm.index), '1 channel, 120 threads.');
  assert.equal(capShare({ answersToday: 5, dailyCap: 20 }), 0.25);
  assert.equal(capShare({ answersToday: 30, dailyCap: 20 }), 1);
});

test('an Inbox link opens the Slack page at its tab, and the page is remembered for the session', () => {
  assert.deepEqual(parseConnectorTarget({ connector: 'slack', tab: 'memories' }), { connector: 'slack', tab: 'memories' });
  assert.deepEqual(parseConnectorTarget({ connector: 'slack', tab: 'billing' }), { connector: 'slack', tab: null });
  assert.deepEqual(parseConnectorTarget({ connector: 'slack' }), { connector: 'slack', tab: null });
  assert.equal(parseConnectorTarget({ connector: 'teams', tab: 'overview' }), null);
  assert.equal(parseConnectorTarget(null), null);
  assert.equal(parseConnectorTarget(['slack']), null);
  requestConnectorPage({ connector: 'slack', tab: 'indexing' });
  assert.deepEqual(takeRequestedConnectorPage(), { connector: 'slack', tab: 'indexing' });
  assert.equal(takeRequestedConnectorPage(), null, 'taken once');
  assert.equal(lastConnectorPage(), null);
  rememberConnectorPage({ connector: 'slack', tab: 'answering' });
  assert.deepEqual(lastConnectorPage(), { connector: 'slack', tab: 'answering' });
  rememberConnectorPage(null);
  assert.equal(lastConnectorPage(), null);
  // The Rust side links the same tabs, the same way.
  const rust = readFileSync(new URL('../src-tauri/src/connectors/slack_notices.rs', import.meta.url), 'utf8');
  const named = [...rust.matchAll(/SlackTab::[A-Za-z]+ => "([a-z]+)"/g)].map(m => m[1]);
  assert.ok(named.length >= 4, named.join(', '));
  for (const tab of named) assert.ok((SLACK_TABS as readonly string[]).includes(tab), tab);
  assert.match(rust, /"connectors",\s*Some\(json!\(\{ "connector": "slack", "tab": tab\.as_str\(\) \}\)\)/);
});

test('the Slack page’s tabs move with the arrow keys, and Escape goes back only when nothing else takes it', () => {
  assert.deepEqual([...SLACK_TABS], ['overview', 'answering', 'indexing', 'memories', 'settings']);
  assert.equal(SLACK_TAB_LABELS.memories, 'Memories');
  assert.equal(moveTab('overview', 'ArrowRight'), 'answering');
  assert.equal(moveTab('overview', 'ArrowLeft'), 'settings');
  assert.equal(moveTab('settings', 'ArrowRight'), 'overview');
  assert.equal(moveTab('indexing', 'Home'), 'overview');
  assert.equal(moveTab('indexing', 'End'), 'settings');
  assert.equal(moveTab('indexing', 'Enter'), null);
  const element = (tagName: string, inside: string | null = null) => ({
    tagName,
    isContentEditable: false,
    closest: (selector: string) => (inside && selector.includes(inside) ? {} : null),
  });
  assert.equal(escapeGoesBack(element('BUTTON')), true);
  assert.equal(escapeGoesBack(element('DIV')), true);
  assert.equal(escapeGoesBack(null), true);
  assert.equal(escapeGoesBack(element('INPUT')), false);
  assert.equal(escapeGoesBack(element('select')), false);
  assert.equal(escapeGoesBack(element('TEXTAREA')), false);
  assert.equal(escapeGoesBack({ tagName: 'DIV', isContentEditable: true }), false);
  assert.equal(escapeGoesBack(element('BUTTON', 'role="menu"')), false);
  assert.equal(escapeGoesBack(element('BUTTON', 'role="alertdialog"')), false);
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

test('the Connectors view sits after Library and lists Slack and Telegram', () => {
  const library = VIEW_TABS.indexOf('library');
  assert.equal(VIEW_TABS[library + 1], 'connectors');
  assert.equal(VIEW_TAB_LABELS.connectors, 'Connectors');
  assert.equal(normalizeViewTab('connectors'), 'connectors');
  assert.deepEqual(CONNECTORS.map(c => c.id), ['slack', 'telegram']);
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
  // The card's commands live in the connector's two modules: answering, and indexing and memories.
  const commands = rust + readFileSync(new URL('../src-tauri/src/connectors/slack_index.rs', import.meta.url), 'utf8');
  const api = readFileSync(new URL('../src/features/connectors/api.ts', import.meta.url), 'utf8');
  const invoked = [...api.matchAll(/invoke<[^>]+>\('([a-z_]+)'/g)].map(m => m[1]);
  assert.ok(invoked.includes('slack_index_channel') && invoked.includes('slack_delete_memory'), 'every command is read');
  for (const command of invoked) {
    assert.match(commands, new RegExp(`#\\[tauri::command\\]\\s+pub async fn ${command}\\(`), command);
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
    backfillDone: false,
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
  assert.match(INDEX_PRIVACY_NOTE, /indexed into the workspace Slack answers come from/);
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

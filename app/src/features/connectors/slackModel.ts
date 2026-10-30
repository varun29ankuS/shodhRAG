/**
 * Words and checks of the Slack card in the Connectors view: the connection state, the day's
 * answers, the setup steps and the token checks (the same rules as
 * `shodh_rag::connectors::slack::tokens`, so a bad paste is caught before Slack is asked).
 * Pure (type-only imports and the shared time words), so `node --test` loads it directly
 * (`app/tests/connectors.test.ts`).
 */

import { localTime, type TimeOptions } from '../tables/format.ts';
import { STALE_TITLE } from '../../lib/freshness.ts';

/** Emitted with the full {@link SlackStatus} whenever it changes (`connectors/slack.rs`). */
export const SLACK_STATUS_EVENT = 'connectors-slack-status';

export type SlackState =
  | 'not_set_up'
  | 'off'
  | 'connecting'
  | 'connected'
  | 'reconnecting'
  | 'error'
  | 'local_only';

/** A channel Shodh answers mentions in (`connectors::slack::ChannelView`). */
export interface AnswerChannelView {
  id: string;
  name: string;
  /** Shared with other organizations (Slack Connect). */
  shared: boolean;
  /** Chosen while not shared, shared now: refused until chosen again. */
  nowShared: boolean;
}

/** A channel the bot is in, offered on the card (`connectors::slack::ChannelChoice`). */
export interface ChannelChoice {
  id: string;
  name: string;
  private: boolean;
  shared: boolean;
  selected: boolean;
  /** The Shodh workspace it is indexed into, if it is. */
  indexedInto: string | null;
}

export type IndexChannelState =
  | 'backfilling'
  | 'up_to_date'
  | 'paused'
  | 'error'
  | 'archived'
  | 'removed'
  | 'now_shared'
  | 'forgetting';

/** An indexed channel (`connectors::slack_index::IndexChannelView`). */
export interface IndexChannelView {
  id: string;
  name: string;
  shared: boolean;
  private: boolean;
  workspace: { id: string; name: string; available: boolean };
  state: IndexChannelState;
  detail: string | null;
  threads: number;
  estimatedThreads: number | null;
  secondsLeft: number | null;
  lastSyncAt: string | null;
  pausedUntil: string | null;
  windowDays: number;
  /** The first backfill finished (it may be paused or stopped since). */
  backfillDone: boolean;
}

/** The Slack member picked as the user. */
export interface SlackOwner {
  userId: string;
  name: string;
}

/** The indexing part of the card (`connectors::slack_index::IndexStatus`). */
export interface IndexStatus {
  available: boolean;
  channels: IndexChannelView[];
  windowDays: number;
  minWindowDays: number;
  maxWindowDays: number;
  missingScopes: string[];
  missingScopesMessage: string | null;
  memories: number;
  owner: SlackOwner | null;
  costNote: string;
}

/** A member of the Slack workspace, for the "This is me" pick. */
export interface Person {
  id: string;
  name: string;
}

/** A memory saved from Slack (`connectors::slack_index::SlackMemoryView`). */
export interface SlackMemoryView {
  id: string;
  channelName: string;
  authorName: string;
  text: string;
  kind: 'text' | 'thread';
  permalink: string | null;
  savedAt: string;
  owner: boolean;
  inMemories: boolean;
}

/** `connectors::slack::SlackStatus`. Never holds a token. */
export interface SlackStatus {
  state: SlackState;
  /** `appId`: the Slack app the bot belongs to (`A…`), `null` when it could not be read. */
  installation: { teamName: string; botName: string; appId: string | null } | null;
  error: string | null;
  nextRetryAt: string | null;
  retryReason: string | null;
  lastEventAt: string | null;
  answersToday: number;
  dailyCap: number;
  minDailyCap: number;
  maxDailyCap: number;
  resetsAt: string;
  answerWorkspace: { id: string; name: string; available: boolean } | null;
  answerChannels: AnswerChannelView[];
  /** Another connection (another Shodh) uses the same Slack app. */
  otherConnections: boolean;
  lastPostFailedAt: string | null;
  /** Scopes the bot lacks (an app created from an earlier manifest). */
  missingScopes: string[];
  /** Indexed channels, memories from Slack, the owner. */
  index: IndexStatus;
  createAppUrl: string;
  botScopes: string[];
  botEvents: string[];
  appTokenScope: string;
}

export type Tone = 'neutral' | 'active' | 'ok' | 'warning' | 'error';

export interface StateLine {
  tone: Tone;
  title: string;
  detail: string | null;
}

/** Shown whenever the connector is on: Socket Mode is a connection from this computer. */
export const RUNNING_HINT = 'Shodh must be running, and this computer awake, for Slack answers.';

/** Where Slack questions go, said before anyone else can ask. */
export const MODEL_PRIVACY_NOTE =
  'Questions from Slack, and the passages found for them, go to the model set in Settings → Model. ' +
  'Some providers log or train on prompts: check your provider’s privacy settings (for OpenRouter: ' +
  'openrouter.ai/settings/privacy) before others can ask.';

function parseTime(iso: string | null): number | null {
  if (!iso) return null;
  const ms = Date.parse(iso);
  return Number.isNaN(ms) ? null : ms;
}

/** The card's headline: what the connection is doing, in plain words. */
export function stateLine(status: SlackStatus, now: number, options: TimeOptions = {}): StateLine {
  const team = status.installation?.teamName ?? 'Slack';
  const bot = status.installation?.botName ?? 'shodh';
  switch (status.state) {
    case 'not_set_up':
      return {
        tone: 'neutral',
        title: 'Not connected',
        detail: 'Create a Slack app for Shodh, then paste its two tokens.',
      };
    case 'off':
      return {
        tone: 'neutral',
        title: 'Off',
        detail: `Connected to ${team} as @${bot}, but switched off: Shodh does not answer in Slack.`,
      };
    case 'connecting':
      return { tone: 'active', title: `Connecting to ${team}…`, detail: status.error };
    case 'connected':
      return status.error
        ? { tone: 'warning', title: `Connected to ${team} as @${bot}`, detail: status.error }
        : { tone: 'ok', title: `Connected to ${team} as @${bot}`, detail: null };
    case 'reconnecting': {
      const at = parseTime(status.nextRetryAt);
      const when = at === null ? 'shortly' : `at ${localTime(at, now, options)}`;
      const why = status.retryReason ? ` The connection dropped: ${status.retryReason}.` : '';
      return { tone: 'warning', title: 'Reconnecting…', detail: `Next try ${when}.${why}` };
    }
    case 'error':
      return {
        tone: 'error',
        title: 'Stopped',
        detail: status.error ?? 'The Slack connection stopped. Switch the connector off and on to try again.',
      };
    case 'local_only':
      return {
        tone: 'neutral',
        title: 'Paused: Local-only mode is on',
        detail: 'Shodh does not connect to Slack while Local-only mode is on. Turn it off in Settings → Privacy.',
      };
  }
}

/** "Slack answers today: 3 of 20; resets at 05:30." */
export function usageLine(status: SlackStatus, now: number, options: TimeOptions = {}): string {
  const reset = parseTime(status.resetsAt);
  const resets = reset === null ? 'at midnight UTC' : `at ${localTime(reset, now, options)}`;
  return `Slack answers today: ${status.answersToday} of ${status.dailyCap}; resets ${resets}.`;
}

/** When Slack last sent something to Shodh. */
export function lastEventLine(status: SlackStatus, now: number, options: TimeOptions = {}): string {
  const at = parseTime(status.lastEventAt);
  return at === null ? 'No Slack events since Shodh started.' : `Last Slack event: ${localTime(at, now, options)}.`;
}

/** Which workspace answers come from, and what the bot says until one is chosen. */
export function answerSourceLine(status: SlackStatus): { tone: Tone; text: string } {
  const ws = status.answerWorkspace;
  if (!ws) {
    return {
      tone: 'warning',
      text: 'Choose the workspace Slack answers come from. Until then the bot replies “Shodh isn’t set up to answer here yet.”',
    };
  }
  if (!ws.available) {
    return {
      tone: 'warning',
      text: 'The workspace chosen for Slack answers was deleted or archived, so the bot says it isn’t set up. Choose another.',
    };
  }
  return {
    tone: 'ok',
    text: `Slack answers come only from the documents of “${ws.name}”. Nothing else in Shodh (other workspaces, memories, notes, tasks, chats) is used.`,
  };
}

/** The reply to a mention where Shodh does not answer (`access::NOT_ALLOWED`). */
export const NOT_ALLOWED_REPLY = 'Shodh isn’t set to answer in this channel.';

/** A channel as the card names it: `#name`, and whether it is private or shared. */
export function channelLabel(channel: { name: string; private?: boolean; shared: boolean }): string {
  const parts = [`#${channel.name}`];
  if (channel.private) parts.push('private');
  if (channel.shared) parts.push('shared with other organizations');
  return parts.join(' · ');
}

/** Where Shodh answers, and who may DM it. */
export function answerPlacesLine(status: SlackStatus): { tone: Tone; text: string } {
  const team = status.installation?.teamName ?? 'your Slack workspace';
  const dms = `Direct messages are answered only for members of ${team}: not guests, and not people from other organizations.`;
  if (status.answerChannels.length === 0) {
    return {
      tone: 'warning',
      text: `Shodh answers mentions in no channel yet: a mention gets “${NOT_ALLOWED_REPLY}” Choose the channels below. ${dms}`,
    };
  }
  const names = status.answerChannels.map(c => channelLabel(c)).join(', ');
  const shared = status.answerChannels.some(c => c.shared);
  return {
    tone: shared ? 'warning' : 'ok',
    text:
      `Shodh answers mentions in ${names}; anywhere else a mention gets “${NOT_ALLOWED_REPLY}” ` +
      (shared ? 'People from other organizations in a shared channel can ask too. ' : '') +
      dms,
  };
}

/** Problems the user should know about, beyond the connection state. */
export function slackWarnings(status: SlackStatus, now: number, options: TimeOptions = {}): string[] {
  const warnings: string[] = [];
  if (status.missingScopes.length > 0) {
    warnings.push(
      `The Slack app is missing these permissions: ${status.missingScopes.join(', ')}. Until it has them Shodh cannot list ` +
        'channels or check who sent a DM, so it answers nowhere. Add them under OAuth & Permissions → Bot Token Scopes ' +
        '(or create the app again from Shodh’s manifest), reinstall the app, and connect again with the new bot token.',
    );
  }
  if (status.otherConnections) {
    warnings.push(
      'Another Shodh is connected to this Slack app. Slack sends each question to only one of them, so answers may come ' +
        'from the other computer and its chosen workspace. Disconnect the other one.',
    );
  }
  const failed = parseTime(status.lastPostFailedAt);
  if (failed !== null) {
    warnings.push(`The last answer failed to post to Slack (${localTime(failed, now, options)}). Details are in the app log.`);
  }
  for (const c of status.answerChannels.filter(c => c.nowShared)) {
    warnings.push(
      `#${c.name} is now shared with other organizations, so Shodh stopped answering there. Choose it again below to answer there.`,
    );
  }
  return warnings;
}

/** What choosing `workspace` exposes, for the confirmation on the card. */
export function exposureLine(
  team: string,
  workspace: string,
  sources: ReadonlyArray<{ kind: string }>,
): string {
  const folders = sources.filter(s => s.kind === 'folder').length;
  const files = sources.filter(s => s.kind === 'file' || s.kind === 'paper').length;
  const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
  return (
    `People in ${team} will get answers from ${count(folders, 'folder', 'folders')} and ${count(files, 'file', 'files')} ` +
    `in “${workspace}”, in the channels chosen below and in DMs.`
  );
}

// ── Setup ───────────────────────────────────────────────────────────

/** `message` says what is wrong, `null` when the token is fine. */
export interface TokenCheck {
  ok: boolean;
  message: string | null;
}

const OK: TokenCheck = { ok: true, message: null };

const TOKEN_CHARS = /^[A-Za-z0-9._-]+$/;
const MAX_TOKEN_CHARS = 512;

function shape(raw: string, kind: string): TokenCheck {
  const token = raw.trim();
  if (!token) return { ok: false, message: `Paste the ${kind}.` };
  if (token.length > MAX_TOKEN_CHARS || !TOKEN_CHARS.test(token)) {
    return { ok: false, message: `The ${kind} contains spaces or characters a Slack token never has.` };
  }
  if (token.startsWith('xoxp-')) {
    return { ok: false, message: 'That is a user token (xoxp-). Shodh needs the bot token (xoxb-), from OAuth & Permissions.' };
  }
  if (token.startsWith('xoxe.') || token.startsWith('xoxe-')) {
    return { ok: false, message: `That is a rotating token (xoxe.). Turn token rotation off for the app and paste the ${kind}.` };
  }
  return OK;
}

/** The app-level token: `xapp-…`. */
export function checkAppToken(raw: string): TokenCheck {
  const shaped = shape(raw, 'app-level token');
  if (!shaped.ok) return shaped;
  const token = raw.trim();
  if (token.startsWith('xoxb-')) return { ok: false, message: 'That looks like the bot token. The app-level token starts with xapp-.' };
  if (!token.startsWith('xapp-') || token.length <= 5) {
    return { ok: false, message: 'The app-level token starts with xapp-. Check that the whole token was copied.' };
  }
  return OK;
}

/** The bot token: `xoxb-…`. */
export function checkBotToken(raw: string): TokenCheck {
  const shaped = shape(raw, 'bot token');
  if (!shaped.ok) return shaped;
  const token = raw.trim();
  if (token.startsWith('xapp-')) return { ok: false, message: 'That looks like the app-level token. The bot token starts with xoxb-.' };
  if (!token.startsWith('xoxb-') || token.length <= 5) {
    return { ok: false, message: 'The bot token starts with xoxb-. Check that the whole token was copied.' };
  }
  return OK;
}

/** The message to show under a token field: none while it is empty (not yet pasted). */
export function tokenHint(raw: string, check: (raw: string) => TokenCheck): string | null {
  if (!raw.trim()) return null;
  const result = check(raw);
  return result.ok ? null : result.message;
}

export function canConnect(appToken: string, botToken: string): boolean {
  return checkAppToken(appToken).ok && checkBotToken(botToken).ok;
}

// ── The setup wizard ─────────────────────────────────────────────────

/** The wizard's steps, 1 to 4. */
export type WizardStep = 1 | 2 | 3 | 4;

export const WIZARD_STEPS: readonly WizardStep[] = [1, 2, 3, 4];

export interface WizardStepText {
  step: WizardStep;
  title: string;
  /** One or two short lines: where to click, what to copy. */
  detail: string;
}

/** The steps of setting up the Slack app, in order. */
export function wizardSteps(status: Pick<SlackStatus, 'appTokenScope'>): WizardStepText[] {
  return [
    {
      step: 1,
      title: 'Create the Shodh app in Slack',
      detail: 'Slack opens with Shodh’s settings filled in. Pick your workspace, then click Create.',
    },
    {
      step: 2,
      title: 'Create the app-level token',
      detail: `In the app’s Basic Information, under App-Level Tokens, generate a token with the ${status.appTokenScope} scope.`,
    },
    {
      step: 3,
      title: 'Install to your workspace',
      detail: 'Under Install App, install it to your workspace, then copy the Bot User OAuth Token.',
    },
    {
      step: 4,
      title: 'Connect',
      detail: 'Shodh checks both tokens with Slack, then keeps them in this computer’s credential store. They are never shown again.',
    },
  ];
}

/** Whether Next (or Connect, on the last step) may be pressed on `step`. */
export function canLeaveStep(step: WizardStep, appToken: string, botToken: string): boolean {
  switch (step) {
    case 1:
      return true;
    case 2:
      return checkAppToken(appToken).ok;
    case 3:
      return checkBotToken(botToken).ok;
    case 4:
      return canConnect(appToken, botToken);
  }
}

/**
 * The step the wizard shows when `step` is asked for: never past a step whose token is
 * missing or wrong (the tokens live in memory only, so they can be gone).
 */
export function reachableStep(step: WizardStep, appToken: string, botToken: string): WizardStep {
  for (const s of WIZARD_STEPS) {
    if (s >= step) return step;
    if (!canLeaveStep(s, appToken, botToken)) return s;
  }
  return step;
}

/** The step that fixes what Slack refused on Connect: the token the message names, else Connect. */
export function stepToFix(message: string): WizardStep {
  const text = message.toLowerCase();
  if (text.includes('different slack apps')) return 2;
  if (text.includes('app-level token') || text.includes('xapp-')) return 2;
  if (text.includes('bot token') || text.includes('xoxb-')) return 3;
  return 4;
}

/** What each permission of Shodh's Slack app lets it do, in plain words. */
const SCOPE_WORDS: Record<string, string> = {
  'app_mentions:read': 'See messages that mention @shodh',
  'channels:read': 'See the public channels it is in',
  'groups:read': 'See the private channels it is added to',
  'chat:write': 'Post answers as @shodh',
  'im:history': 'Read direct messages people send it',
  'im:read': 'See its direct message conversations',
  'im:write': 'Reply in direct messages',
  'users:read': 'See people’s names, to check who is asking',
  'channels:history': 'Read public channels you choose to index',
  'groups:history': 'Read private channels you choose to index',
};

/** `scope` in plain words (its id when Shodh has none for it). */
export function scopeWords(scope: string): string {
  return SCOPE_WORDS[scope] ?? scope;
}

// ── The tile and the page header ────────────────────────────────────

export type PillKind = 'not_connected' | 'connecting' | 'connected' | 'attention' | 'off';

/** The connector's status pill: what it is doing, in one or two words. */
export interface Pill {
  kind: PillKind;
  tone: Tone;
  label: string;
  /** Why it needs attention, or why it is off. */
  reason: string | null;
  /** The Slack workspace, when connected. */
  team: string | null;
}

/**
 * The pill over a status kept from an earlier read when the latest read
 * failed (lib/freshness "stale"): the status may have changed, so it is not
 * shown as current. The last known state is kept as the reason.
 */
export function stalePill(pill: Pill): Pill {
  return { kind: 'attention', tone: 'warning', label: STALE_TITLE, reason: `last known: ${pill.label.toLowerCase()}`, team: pill.team };
}

/** The pill of the Slack connector, from its status. */
export function slackPill(status: SlackStatus): Pill {
  const team = status.installation?.teamName ?? null;
  switch (status.state) {
    case 'not_set_up':
      return { kind: 'not_connected', tone: 'neutral', label: 'Not connected', reason: null, team: null };
    case 'off':
      return { kind: 'off', tone: 'neutral', label: 'Off', reason: null, team };
    case 'local_only':
      return { kind: 'off', tone: 'neutral', label: 'Off', reason: 'Local-only mode', team };
    case 'connecting':
    case 'reconnecting':
      return { kind: 'connecting', tone: 'active', label: 'Connecting…', reason: null, team };
    case 'error':
      return { kind: 'attention', tone: 'error', label: 'Needs attention', reason: 'Connection stopped', team };
    case 'connected': {
      const reason = attentionReason(status);
      return reason
        ? { kind: 'attention', tone: 'warning', label: 'Needs attention', reason, team }
        : { kind: 'connected', tone: 'ok', label: 'Connected', reason: null, team };
    }
  }
}

/** Paused by Slack's rate limit (it resumes on its own). */
export function isRateLimited(channel: Pick<IndexChannelView, 'state' | 'detail'>): boolean {
  return channel.state === 'paused' && (channel.detail === 'Slack’s rate limit' || channel.detail === "Slack's rate limit");
}

/** A channel that stopped indexing until the user acts. */
export function channelNeedsAttention(channel: Pick<IndexChannelView, 'state'>): boolean {
  return channel.state === 'error' || channel.state === 'removed' || channel.state === 'now_shared';
}

/** Why a connected Slack needs the user, most pressing first; `null` when it does not. */
function attentionReason(status: SlackStatus): string | null {
  if (status.missingScopes.length > 0 || status.index.missingScopes.length > 0) return 'Missing permissions';
  if (status.error) return 'Slack refused a reply';
  if (status.otherConnections) return 'Another Shodh is connected';
  if (status.lastPostFailedAt) return 'An answer was not posted';
  if (status.index.channels.some(channelNeedsAttention)) return 'A channel stopped';
  if (status.answerChannels.some(c => c.nowShared)) return 'A channel is shared now';
  if (status.index.channels.some(isRateLimited)) return 'Rate-limited by Slack';
  return null;
}

/** The tile's one action. */
export function tileAction(status: Pick<SlackStatus, 'installation'>): 'Connect' | 'Manage' {
  return status.installation ? 'Manage' : 'Connect';
}

// ── The connected page ──────────────────────────────────────────────

/** The day's answers as a share of the cap, 0 to 1. */
export function capShare(status: Pick<SlackStatus, 'answersToday' | 'dailyCap'>): number {
  if (status.dailyCap <= 0) return 1;
  return Math.min(1, Math.max(0, status.answersToday / status.dailyCap));
}

export type SlackTabId = 'overview' | 'answering' | 'indexing' | 'memories' | 'settings';

/** Something left to set up or to fix, on the overview. */
export interface NextStep {
  id: string;
  tone: Tone;
  title: string;
  detail: string;
  /** The tab that fixes it; `null` when it is fixed outside the page. */
  tab: SlackTabId | null;
  action: string | null;
  /** The Slack permissions it is about (the "Update the Slack app" step). */
  permissions?: string[];
}

// ── Updating the Slack app ──────────────────────────────────────────

/** Why the app needs updating, in one line (the permission names are a click away). */
export const UPDATE_APP_LINE =
  'Your Slack app was made from an older version of Shodh’s settings and lacks permissions Shodh now uses. ' +
  'Give it the updated settings, then reinstall it.';

/** The same for the Indexing tab, where only the indexing permissions are missing. */
export const UPDATE_APP_FOR_INDEXING_LINE =
  'To index channels, your Slack app needs permissions it was not given when it was made. Give it Shodh’s updated ' +
  'settings, then reinstall it. Answering keeps working meanwhile.';

/** What to do in Slack, in order. */
export const UPDATE_APP_STEPS: readonly string[] = [
  'On the app’s App Manifest page, replace the manifest with the copied one and press Save Changes.',
  'Slack then shows a banner asking you to reinstall the app: reinstall it to your workspace.',
  'Come back here and press Check again.',
];

const SLACK_APP_ID = /^A[A-Z0-9]{2,30}$/;

/**
 * The App Manifest page of the user's Slack app; the list of their apps when the app is
 * not known (an installation from before Shodh kept it) or its id is not one.
 */
export function appManifestUrl(appId: string | null | undefined): string {
  return appId && SLACK_APP_ID.test(appId)
    ? `https://api.slack.com/apps/${appId}/app-manifest`
    : 'https://api.slack.com/apps';
}

/**
 * The manifest the "create the app" link carries (`manifest_json`), pretty-printed for
 * pasting into an existing app's App Manifest page: the very settings a new app gets.
 * `null` when the link carries none that parses.
 */
export function manifestFromCreateUrl(createAppUrl: string): string | null {
  let raw: string | null;
  try {
    raw = new URL(createAppUrl).searchParams.get('manifest_json');
  } catch {
    return null;
  }
  if (!raw) return null;
  try {
    const manifest: unknown = JSON.parse(raw);
    if (manifest === null || typeof manifest !== 'object' || Array.isArray(manifest)) return null;
    return JSON.stringify(manifest, null, 2);
  } catch {
    return null;
  }
}

/** The permissions still missing after Check again, for the message saying so. */
export function stillMissingLine(status: Pick<SlackStatus, 'missingScopes' | 'index'>): string | null {
  const missing = [...new Set([...status.missingScopes, ...status.index.missingScopes])];
  if (missing.length === 0) return null;
  return (
    `Slack still lists the app without ${missing.length === 1 ? 'one permission' : `${missing.length} permissions`}. ` +
    'Check that the manifest was saved and the app reinstalled, then check again.'
  );
}

/** What is left to set up, or to fix, most pressing first. */
export function nextSteps(status: SlackStatus): NextStep[] {
  const steps: NextStep[] = [];
  const scopes = [...new Set([...status.missingScopes, ...status.index.missingScopes])].sort();
  if (scopes.length > 0) {
    steps.push({
      id: 'scopes',
      tone: 'warning',
      title: 'Update the Slack app',
      detail: UPDATE_APP_LINE,
      tab: null,
      action: null,
      permissions: scopes,
    });
  }
  if (status.state === 'local_only') {
    steps.push({
      id: 'local_only',
      tone: 'neutral',
      title: 'Local-only mode is on',
      detail: 'Shodh does not connect to Slack while it is on. Turn it off in Settings → Privacy.',
      tab: null,
      action: null,
    });
  } else if (status.state === 'off') {
    steps.push({
      id: 'off',
      tone: 'neutral',
      title: 'Slack answers are off',
      detail: 'Shodh is connected but does not answer. Switch it on under Answering.',
      tab: 'answering',
      action: 'Switch on',
    });
  }
  const ws = status.answerWorkspace;
  if (!ws || !ws.available) {
    steps.push({
      id: 'workspace',
      tone: 'warning',
      title: ws ? 'The answer workspace is gone' : 'Choose what Slack answers from',
      detail: ws
        ? 'It was deleted or archived, so the bot says it isn’t set up. Choose another workspace.'
        : 'Until you pick a workspace, the bot replies that it isn’t set up yet.',
      tab: 'answering',
      action: 'Choose a workspace',
    });
  }
  for (const c of status.answerChannels.filter(c => c.nowShared)) {
    steps.push({
      id: `now_shared:${c.id}`,
      tone: 'warning',
      title: `#${c.name} is shared with other organizations now`,
      detail: 'Shodh stopped answering there. Choose it again under Answering to answer there.',
      tab: 'answering',
      action: 'Choose channels',
    });
  }
  if (status.answerChannels.length === 0) {
    steps.push({
      id: 'channels',
      tone: 'warning',
      title: 'Choose where Shodh answers',
      detail: 'It answers mentions in no channel yet. Direct messages from your members already work.',
      tab: 'answering',
      action: 'Choose channels',
    });
  }
  const stopped = status.index.channels.filter(channelNeedsAttention);
  if (stopped.length > 0) {
    steps.push({
      id: 'stopped',
      tone: 'warning',
      title: stopped.length === 1 ? `#${stopped[0].name} stopped indexing` : `${stopped.length} channels stopped indexing`,
      detail: 'Open Indexing to see why and what to do.',
      tab: 'indexing',
      action: 'Open Indexing',
    });
  }
  if (!status.index.owner) {
    steps.push({
      id: 'owner',
      tone: 'neutral',
      title: 'Tell Shodh which Slack member you are',
      detail: 'Then your “@shodh remember:” goes into your own memories.',
      tab: 'memories',
      action: 'Pick yourself',
    });
  }
  return steps;
}

/**
 * The warnings the overview shows besides its next steps: missing permissions and newly
 * shared channels are next steps there (with where to fix them), so not said twice.
 */
export function overviewWarnings(status: SlackStatus, now: number, options: TimeOptions = {}): string[] {
  return slackWarnings(
    { ...status, missingScopes: [], answerChannels: status.answerChannels.filter(c => !c.nowShared) },
    now,
    options,
  );
}

/** A small count on a tab, and whether it needs the user. */
export interface TabBadge {
  count: number;
  tone: 'neutral' | 'warning';
}

/** The badges of the page's tabs: what needs the user on each, and what each holds. */
export function slackTabBadges(status: SlackStatus, now: number): Partial<Record<SlackTabId, TabBadge>> {
  const badges: Partial<Record<SlackTabId, TabBadge>> = {};
  const fixes = nextSteps(status).filter(s => s.tone === 'warning').length + overviewWarnings(status, now).length;
  if (fixes > 0) badges.overview = { count: fixes, tone: 'warning' };
  const stopped = status.index.channels.filter(channelNeedsAttention).length;
  if (stopped > 0) badges.indexing = { count: stopped, tone: 'warning' };
  else if (status.index.channels.length > 0) badges.indexing = { count: status.index.channels.length, tone: 'neutral' };
  if (status.index.memories > 0) badges.memories = { count: status.index.memories, tone: 'neutral' };
  return badges;
}

/** What the indexing summary on the overview says. */
export function indexSummary(index: Pick<IndexStatus, 'channels'>): string {
  const channels = index.channels.length;
  if (channels === 0) return 'No channel indexed yet.';
  const threadCount = index.channels.reduce((n, c) => n + c.threads, 0);
  const stopped = index.channels.filter(channelNeedsAttention).length;
  const base = `${channels} ${channels === 1 ? 'channel' : 'channels'}, ${threads(threadCount)}`;
  return stopped > 0 ? `${base}; ${stopped} ${stopped === 1 ? 'needs' : 'need'} attention.` : `${base}.`;
}

/** Whether the daily cap input holds a value the card accepts. */
export function validCap(input: string, status: Pick<SlackStatus, 'minDailyCap' | 'maxDailyCap'>): number | null {
  if (!/^\d+$/.test(input.trim())) return null;
  const n = Number(input.trim());
  return Number.isInteger(n) && n >= status.minDailyCap && n <= status.maxDailyCap ? n : null;
}

// ── Indexed channels ────────────────────────────────────────────────

/** "about 3 minutes", "about 2 hours", "less than a minute". */
export function durationWords(seconds: number): string {
  if (seconds < 60) return 'less than a minute';
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `about ${minutes} ${minutes === 1 ? 'minute' : 'minutes'}`;
  const hours = Math.round(minutes / 60);
  return `about ${hours} ${hours === 1 ? 'hour' : 'hours'}`;
}

function threads(n: number): string {
  return `${n} ${n === 1 ? 'thread' : 'threads'}`;
}

/** What the card says about one indexed channel. */
export function indexChannelLine(channel: IndexChannelView, now: number, options: TimeOptions = {}): StateLine {
  switch (channel.state) {
    case 'backfilling': {
      const title =
        channel.estimatedThreads !== null && channel.estimatedThreads > channel.threads
          ? `Backfilling: ${channel.threads} of ~${channel.estimatedThreads} threads`
          : `Backfilling: ${threads(channel.threads)} so far`;
      const left =
        channel.secondsLeft !== null && channel.secondsLeft > 0
          ? `${durationWords(channel.secondsLeft)} left at Slack’s pace. `
          : '';
      return {
        tone: 'active',
        title,
        detail: `${left}Reading the last ${channel.windowDays} days; it carries on where it stopped if Shodh closes.`,
      };
    }
    case 'up_to_date': {
      const at = parseTime(channel.lastSyncAt);
      return {
        tone: 'ok',
        title: `Up to date: ${threads(channel.threads)}`,
        detail: at === null ? null : `Last sync ${localTime(at, now, options)}.`,
      };
    }
    case 'paused': {
      const until = parseTime(channel.pausedUntil);
      const when = until === null ? 'shortly' : localTime(until, now, options);
      return isRateLimited(channel)
        ? { tone: 'warning', title: `Paused: rate limit until ${when}`, detail: 'Slack asked Shodh to wait; it resumes on its own.' }
        : { tone: 'warning', title: `Paused until ${when}`, detail: channel.detail };
    }
    case 'error':
      return { tone: 'error', title: 'Error', detail: channel.detail ?? 'Indexing stopped. Connect again to retry.' };
    case 'archived':
      return { tone: 'neutral', title: 'Archived in Slack', detail: channel.detail };
    case 'removed':
      return { tone: 'warning', title: 'Shodh was removed from the channel', detail: channel.detail };
    case 'now_shared':
      return { tone: 'warning', title: 'Shared with other organizations now', detail: channel.detail };
    case 'forgetting':
      return { tone: 'active', title: 'Forgetting…', detail: null };
  }
}

/** The confirmation a shared channel needs before it is indexed. */
export function sharedIndexLine(channel: { name: string }): string {
  return (
    `#${channel.name} is shared with other organizations. Indexing it stores their messages on this computer ` +
    'and puts them in the workspace you choose. Index it only if they agreed.'
  );
}

/** What indexing does, and what it does not. */
export const INDEX_PRIVACY_NOTE =
  'Indexed threads are stored on this computer (use your disk’s encryption: BitLocker or FileVault) and show up in ' +
  'Ask and the workspace like its documents, with links back to Slack. A Slack answer uses only the threads of the ' +
  'channel it is asked in, and only when that channel is indexed into the workspace Slack answers come from. Index ' +
  'channels of your own Slack workspace only: Slack’s terms do not allow copying a client’s workspace with your app.';

/** Who "remember:" saves for, on the card. */
export function ownerLine(index: Pick<IndexStatus, 'owner'>): string {
  return index.owner
    ? `You are ${index.owner.name} in Slack: what you ask Shodh to remember goes into your memories. ` +
        'What anyone else asks it to remember is kept for their channel only.'
    : 'Pick yourself below so your “@shodh remember:” goes into your memories. Until then, everything saved from Slack ' +
        'is kept for its channel only.';
}

/** Where a memory from Slack lives. */
export function memoryPlace(memory: SlackMemoryView): string {
  if (memory.owner) return 'In your memories';
  if (memory.inMemories) return `For ${memory.channelName}, and in your memories`;
  return `For ${memory.channelName} only`;
}

/** "Saved by Ann in #deals, 24 Oct" (a thread: "Thread saved by …"). */
export function memoryProvenance(memory: SlackMemoryView, options: TimeOptions = {}): string {
  const at = parseTime(memory.savedAt);
  const date =
    at === null
      ? ''
      : `, ${new Intl.DateTimeFormat(options.locale, { day: 'numeric', month: 'short', timeZone: options.timeZone }).format(new Date(at))}`;
  const what = memory.kind === 'thread' ? 'Thread saved' : 'Saved';
  return `${what} by ${memory.authorName} in ${memory.channelName}${date}`;
}

/** How many memories were saved from Slack. */
export function memoriesCountLine(index: Pick<IndexStatus, 'memories'>): string {
  if (index.memories === 0) return 'Nothing saved from Slack yet. People can say “@shodh remember: …” in an answer channel.';
  return `${index.memories} ${index.memories === 1 ? 'memory' : 'memories'} saved from Slack.`;
}

/** Whether the window input holds a value the card accepts. */
export function validWindow(input: string, index: Pick<IndexStatus, 'minWindowDays' | 'maxWindowDays'>): number | null {
  if (!/^\d+$/.test(input.trim())) return null;
  const n = Number(input.trim());
  return Number.isInteger(n) && n >= index.minWindowDays && n <= index.maxWindowDays ? n : null;
}

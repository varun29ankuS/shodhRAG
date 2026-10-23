/**
 * Words and checks of the Slack card in the Connectors view: the connection state, the day's
 * answers, the setup steps and the token checks (the same rules as
 * `shodh_rag::connectors::slack::tokens`, so a bad paste is caught before Slack is asked).
 * Pure (type-only imports and the shared time words), so `node --test` loads it directly
 * (`app/tests/connectors.test.ts`).
 */

import { localTime, type TimeOptions } from '../tables/format.ts';

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
}

/** `connectors::slack::SlackStatus`. Never holds a token. */
export interface SlackStatus {
  state: SlackState;
  installation: { teamName: string; botName: string } | null;
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

export interface SetupStep {
  title: string;
  detail: string;
}

/** The steps of setting up the Slack app, in order. */
export function setupSteps(status: Pick<SlackStatus, 'appTokenScope' | 'botScopes'>): SetupStep[] {
  return [
    {
      title: 'Create the Slack app',
      detail:
        'Opens Slack with Shodh’s settings filled in: Socket Mode (no public URL), a bot people can mention or message, ' +
        `and only these permissions: ${status.botScopes.join(', ')}. Pick your Slack workspace and create the app.`,
    },
    {
      title: 'Create the app-level token',
      detail: `In the app’s Basic Information → App-Level Tokens, generate a token with the ${status.appTokenScope} scope. It starts with xapp-.`,
    },
    {
      title: 'Install the app',
      detail: 'Under Install App, install it to your workspace, then copy the Bot User OAuth Token. It starts with xoxb-.',
    },
    {
      title: 'Paste both tokens',
      detail: 'Shodh checks them with Slack before saving. They are kept in this computer’s credential store and never shown again.',
    },
  ];
}

/** Whether the daily cap input holds a value the card accepts. */
export function validCap(input: string, status: Pick<SlackStatus, 'minDailyCap' | 'maxDailyCap'>): number | null {
  if (!/^\d+$/.test(input.trim())) return null;
  const n = Number(input.trim());
  return Number.isInteger(n) && n >= status.minDailyCap && n <= status.maxDailyCap ? n : null;
}

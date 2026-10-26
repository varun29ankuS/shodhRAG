/**
 * The Telegram page's words and rules, from the status the backend sends
 * (`app/src-tauri/src/connectors/telegram.rs`). Pure, so `node --test` loads it
 * (`app/tests/telegram.test.ts`).
 */

import { localTime, type TimeOptions } from '../tables/format.ts';
import type { Pill, StateLine, Tone } from './slackModel.ts';

/** Emitted with the full {@link TelegramStatus} whenever it changes. */
export const TELEGRAM_STATUS_EVENT = 'connectors-telegram-status';

/** @BotFather, where a bot is made. */
export const BOTFATHER_URL = 'https://t.me/BotFather';

export type TelegramState = 'not_set_up' | 'off' | 'connecting' | 'connected' | 'reconnecting' | 'error' | 'local_only';

export interface TelegramBotView {
  username: string;
  name: string;
  link: string;
  readsAllGroupMessages: boolean;
}

export interface TelegramPersonView {
  id: string;
  name: string;
  username: string | null;
  linkedAt: string;
}

export interface TelegramGroupView {
  id: string;
  title: string;
  allowed: boolean;
  addedBy: string | null;
  seenAt: string;
}

export interface TelegramStatus {
  state: TelegramState;
  bot: TelegramBotView | null;
  error: string | null;
  nextRetryAt: string | null;
  retryReason: string | null;
  lastUpdateAt: string | null;
  owner: TelegramPersonView | null;
  people: TelegramPersonView[];
  groups: TelegramGroupView[];
  answerWorkspace: { id: string; name: string; available: boolean } | null;
  answersToday: number;
  dailyCap: number;
  minDailyCap: number;
  maxDailyCap: number;
  resetsAt: string;
  lastSendFailedAt: string | null;
  privacyNote: string;
}

/** A one-time link and its QR code (`telegram_link_code`). */
export interface LinkInvite {
  url: string;
  expiresAt: string;
  qr: { size: number; rows: string[] };
}

export type TelegramTabId = 'overview' | 'people' | 'settings';

/** Where Telegram questions go, said before anyone else can ask. */
export const TELEGRAM_MODEL_NOTE =
  'Your messages to the bot, and the passages found for them, go to the model set in Settings → Model; so do the ' +
  'questions of people and groups you allow. Some providers log or train on prompts: check your provider’s privacy ' +
  'settings (for OpenRouter: openrouter.ai/settings/privacy) before others can ask.';

/** Shown whenever the connector is on: polling runs on this computer. */
export const TELEGRAM_RUNNING_HINT = 'Shodh must be running, and this computer awake, to answer in Telegram.';

// ── The token ───────────────────────────────────────────────────────

export interface TokenCheck {
  ok: boolean;
  /** What is wrong, in plain words; `null` when it looks right (or nothing is typed). */
  problem: string | null;
}

/** The same rules as the Rust token type: digits, a colon, then 35 of `A-Z a-z 0-9 _ -`. */
export function checkTelegramToken(raw: string): TokenCheck {
  const token = raw.trim();
  if (!token) return { ok: false, problem: null };
  const at = token.indexOf(':');
  if (at < 0) return { ok: false, problem: 'A bot token has a colon after the bot’s number: 123456789:AAH…' };
  const id = token.slice(0, at);
  const secret = token.slice(at + 1);
  if (!/^[0-9]{1,20}$/.test(id) || /^0+$/.test(id)) {
    return { ok: false, problem: 'A bot token starts with the bot’s number, then a colon.' };
  }
  if (!/^[A-Za-z0-9_-]*$/.test(secret)) {
    return { ok: false, problem: 'After the colon a bot token has only letters, digits, - and _.' };
  }
  if (secret.length !== 35) {
    return { ok: false, problem: `After the colon a bot token has 35 characters; this has ${secret.length}. Check that the whole token was copied.` };
  }
  return { ok: true, problem: null };
}

// ── The wizard ──────────────────────────────────────────────────────

export type TelegramStep = 1 | 2 | 3;

export const TELEGRAM_STEPS: readonly TelegramStep[] = [1, 2, 3];

export interface StepText {
  step: TelegramStep;
  title: string;
  detail: string;
}

export function telegramSteps(): StepText[] {
  return [
    {
      step: 1,
      title: 'Make a bot with @BotFather',
      detail: 'In Telegram, open @BotFather and send /newbot. Give the bot a name, then a username that ends in “bot”.',
    },
    {
      step: 2,
      title: 'Paste its token',
      detail: '@BotFather replies with the bot’s token. Shodh keeps it in this computer’s credential store and checks it with Telegram.',
    },
    {
      step: 3,
      title: 'Link your account',
      detail: 'Open the link on your phone, or scan the code, and press Start. That chat becomes your Shodh: all your workspaces and memories.',
    },
  ];
}

/** The step the page shows: the token step until a bot is connected, then linking. */
export function telegramStep(status: Pick<TelegramStatus, 'bot' | 'owner'>, wanted: TelegramStep): TelegramStep {
  if (!status.bot) return wanted === 3 ? 2 : wanted;
  return 3;
}

/** "9:41" left of a link, or `null` once it expired. */
export function countdown(expiresAt: string, now: number): string | null {
  const end = Date.parse(expiresAt);
  if (Number.isNaN(end)) return null;
  const left = Math.floor((end - now) / 1000);
  if (left <= 0) return null;
  const minutes = Math.floor(left / 60);
  const seconds = left % 60;
  return `${minutes}:${String(seconds).padStart(2, '0')}`;
}

/** The SVG path of a QR code's dark modules (one unit per module, a 4-module quiet zone). */
export function qrPath(rows: readonly string[]): { path: string; size: number } {
  const quiet = 4;
  const parts: string[] = [];
  rows.forEach((row, y) => {
    let x = 0;
    while (x < row.length) {
      if (row[x] !== '1') {
        x += 1;
        continue;
      }
      let run = 1;
      while (x + run < row.length && row[x + run] === '1') run += 1;
      parts.push(`M${x + quiet} ${y + quiet}h${run}v1h-${run}z`);
      x += run;
    }
  });
  return { path: parts.join(''), size: rows.length + quiet * 2 };
}

// ── The tile and the page ───────────────────────────────────────────

function parseTime(iso: string | null): number | null {
  if (!iso) return null;
  const ms = Date.parse(iso);
  return Number.isNaN(ms) ? null : ms;
}

/** Groups that added the bot and wait for the user. */
export function waitingGroups(status: Pick<TelegramStatus, 'groups'>): TelegramGroupView[] {
  return status.groups.filter(g => !g.allowed);
}

/** Why a connected bot needs the user, most pressing first; `null` when it does not. */
function attentionReason(status: TelegramStatus): string | null {
  if (!status.owner) return 'Not linked to you yet';
  if (status.lastSendFailedAt) return 'An answer was not sent';
  if (waitingGroups(status).length > 0) return 'A group is waiting';
  return null;
}

/** The pill of the Telegram connector. `team` carries the bot's @username. */
export function telegramPill(status: TelegramStatus): Pill {
  const bot = status.bot ? `@${status.bot.username}` : null;
  switch (status.state) {
    case 'not_set_up':
      return { kind: 'not_connected', tone: 'neutral', label: 'Not connected', reason: null, team: null };
    case 'off':
      return { kind: 'off', tone: 'neutral', label: 'Off', reason: null, team: bot };
    case 'local_only':
      return { kind: 'off', tone: 'neutral', label: 'Off', reason: 'Local-only mode', team: bot };
    case 'connecting':
    case 'reconnecting':
      return { kind: 'connecting', tone: 'active', label: 'Connecting…', reason: null, team: bot };
    case 'error':
      return { kind: 'attention', tone: 'error', label: 'Needs attention', reason: 'Connection stopped', team: bot };
    case 'connected': {
      const reason = attentionReason(status);
      return reason
        ? { kind: 'attention', tone: 'warning', label: 'Needs attention', reason, team: bot }
        : { kind: 'connected', tone: 'ok', label: 'Connected', reason: null, team: bot };
    }
  }
}

/** The tile's one action. */
export function telegramTileAction(status: Pick<TelegramStatus, 'bot'>): 'Connect' | 'Manage' {
  return status.bot ? 'Manage' : 'Connect';
}

/** "Linked to @ann", or who it is when they have no username. */
export function ownerLine(status: Pick<TelegramStatus, 'owner'>): string {
  const owner = status.owner;
  if (!owner) return 'Not linked yet: nobody can use this bot as you.';
  return owner.username ? `Linked to @${owner.username}` : `Linked to ${owner.name}`;
}

/** What the connection is doing, in plain words. */
export function telegramStateLine(status: TelegramStatus, now: number, options: TimeOptions = {}): StateLine {
  const bot = status.bot ? `@${status.bot.username}` : 'the bot';
  switch (status.state) {
    case 'not_set_up':
      return { tone: 'neutral', title: 'Not connected', detail: 'Make a bot with @BotFather, then paste its token.' };
    case 'off':
      return { tone: 'neutral', title: 'Off', detail: `${bot} is saved but switched off: Shodh does not read its messages.` };
    case 'connecting':
      return { tone: 'active', title: `Connecting to ${bot}…`, detail: null };
    case 'connected':
      return { tone: 'ok', title: `Reading ${bot}`, detail: null };
    case 'reconnecting': {
      const at = parseTime(status.nextRetryAt);
      const when = at === null ? 'shortly' : `at ${localTime(at, now, options)}`;
      const why = status.retryReason ? ` ${status.retryReason}.` : '';
      return { tone: 'warning', title: 'Reconnecting…', detail: `Next try ${when}.${why}` };
    }
    case 'error':
      return {
        tone: 'error',
        title: 'Stopped',
        detail: status.error ?? 'The Telegram connection stopped. Switch the connector off and on to try again.',
      };
    case 'local_only':
      return {
        tone: 'neutral',
        title: 'Paused: Local-only mode is on',
        detail: 'Shodh does not connect to Telegram while Local-only mode is on. Turn it off in Settings → Privacy.',
      };
  }
}

/** "Answers to others today: 3 of 20; resets at 05:30. Your own chat has no limit." */
export function telegramUsageLine(status: TelegramStatus, now: number, options: TimeOptions = {}): string {
  const reset = parseTime(status.resetsAt);
  const resets = reset === null ? 'at midnight UTC' : `at ${localTime(reset, now, options)}`;
  return `Answers to others today: ${status.answersToday} of ${status.dailyCap}; resets ${resets}. Your own chat has no limit.`;
}

/** Which workspace others are answered from. */
export function guestSourceLine(status: Pick<TelegramStatus, 'answerWorkspace'>): { tone: Tone; text: string } {
  const ws = status.answerWorkspace;
  if (!ws) {
    return {
      tone: 'warning',
      text: 'Choose the workspace people and groups are answered from. Until then they hear “Shodh isn’t set up to answer here yet.”',
    };
  }
  if (!ws.available) {
    return { tone: 'warning', text: 'The workspace chosen for others was deleted or archived. Choose another.' };
  }
  return {
    tone: 'ok',
    text: `People and groups you allow get answers only from “${ws.name}”: never your memories or other workspaces.`,
  };
}

/** Something left to set up or fix, on the overview. */
export interface TelegramNextStep {
  id: string;
  tone: Tone;
  title: string;
  detail: string;
  tab: TelegramTabId | null;
  action: string | null;
}

export function telegramNextSteps(status: TelegramStatus): TelegramNextStep[] {
  const steps: TelegramNextStep[] = [];
  if (status.state === 'local_only') {
    steps.push({
      id: 'local-only',
      tone: 'neutral',
      title: 'Local-only mode is on',
      detail: 'Shodh does not connect to Telegram while it is on. Turn it off in Settings → Privacy.',
      tab: null,
      action: null,
    });
  } else if (status.state === 'off') {
    steps.push({
      id: 'off',
      tone: 'neutral',
      title: 'The connector is off',
      detail: 'Switch it on under Settings to read the bot’s messages again.',
      tab: 'settings',
      action: 'Open Settings',
    });
  }
  if (!status.owner) {
    steps.push({
      id: 'link',
      tone: 'warning',
      title: 'Link your Telegram account',
      detail: 'Until you do, the bot refuses everyone, you included.',
      tab: 'people',
      action: 'Link my account',
    });
  }
  const waiting = waitingGroups(status);
  if (waiting.length > 0) {
    steps.push({
      id: 'groups',
      tone: 'warning',
      title: waiting.length === 1 ? `“${waiting[0].title}” wants answers` : `${waiting.length} groups want answers`,
      detail: 'Nobody there gets an answer until you allow the group.',
      tab: 'people',
      action: 'Review',
    });
  }
  const others = status.people.length + status.groups.filter(g => g.allowed).length;
  if (others > 0 && (!status.answerWorkspace || !status.answerWorkspace.available)) {
    steps.push({
      id: 'workspace',
      tone: 'warning',
      title: 'Choose what others are answered from',
      detail: 'People and groups you allowed hear that Shodh isn’t set up until you choose a workspace.',
      tab: 'people',
      action: 'Choose a workspace',
    });
  }
  return steps;
}

/** The People & groups tab's badge: groups waiting for the user. */
export function telegramTabBadges(status: TelegramStatus): Partial<Record<TelegramTabId, { count: number; tone: 'warning' | 'neutral' }>> {
  const waiting = waitingGroups(status).length;
  return waiting > 0 ? { people: { count: waiting, tone: 'warning' } } : {};
}

/** What the page says about who receives group messages. */
export function groupPrivacyLine(status: Pick<TelegramStatus, 'bot'>): string {
  if (status.bot?.readsAllGroupMessages) {
    return 'Privacy mode is off for this bot: Telegram sends it every message in its groups. Shodh still answers only when mentioned or replied to.';
  }
  return 'In groups, people ask with /ask@' + (status.bot?.username ?? 'yourbot') +
    ' followed by the question, or by replying to Shodh’s answer. Telegram doesn’t send the bot other group messages while privacy mode is on.';
}

/** When the last update arrived. */
export function lastUpdateLine(status: Pick<TelegramStatus, 'lastUpdateAt'>, now: number, options: TimeOptions = {}): string {
  const at = parseTime(status.lastUpdateAt);
  return at === null ? 'No messages since Shodh started.' : `Last message: ${localTime(at, now, options)}.`;
}

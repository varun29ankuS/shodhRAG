/**
 * The floating chat over the embedded Artifacts preview (`dock_layer.rs`) runs in a webview of
 * its own, `dock`, which holds no chat session. The main webview is the only session: it
 * publishes a snapshot of what the chat shows (`DockSnapshot`), and the dock sends the user's
 * clicks and keys back (`DockAction`), which main checks with `parseDockAction` before acting.
 * Pure: no React, type-only imports, so the Node tests load it.
 */

import type { TranscriptState } from '../features/agent/reducer';
import type { ChatMessage } from '../features/ask/types';
import type { RuntimeSetupState } from '../features/setup/runtimeSetupHost';

/** The largest snapshot or action relayed, in bytes of JSON (Rust's `MAX_RELAY_BYTES`). */
export const MAX_SNAPSHOT_BYTES = 262144;

/** The longest draft the dock may send (characters). */
export const MAX_DRAFT_CHARS = 20000;

/** What the floating chat shows: the active conversation's last exchange, as the dock shows it. */
export interface DockSnapshot {
  v: 1;
  conversationId: string | null;
  title: string;
  /** The last question, as the chat shows it above the answer. */
  lastUser: { content: string } | null;
  /**
   * The last answer's transcript (live while it runs). Only when it is too large for the relay
   * are its oldest blocks left out (`fitTranscript`), and then `trimmed` says so, so the chat
   * shows that earlier work is in Ask instead of hiding it.
   */
  transcript: TranscriptState | null;
  trimmed: boolean;
  /** The answer engine's setup as main sees it, for the runtime card in the transcript. */
  runtime: RuntimeSetupState;
  isStreaming: boolean;
  /** An answer runs in another conversation, or a side question holds the run. */
  busyElsewhere: boolean;
  /** The side question holding the run, if one does. */
  sideRunLabel: string | null;
  /** The connector a read-only conversation came from (Slack, Telegram), if it did. */
  readOnlyConnector: string | null;
  /** The shared draft, owned by main: "Open in Ask" keeps what was typed. */
  draft: string;
  /** Main is dictating into the draft. */
  dictating: boolean;
  /** Main can dictate (it holds the microphone; the dock never does). */
  voiceSupported: boolean;
  /** The chat is open (the panel), not minimised (the pill). */
  open: boolean;
  /** New activity while minimised. */
  unread: number;
  theme: 'light' | 'dark';
}

/** What the dock may ask main to do. Anything else is refused. */
export type DockAction =
  | { kind: 'send' }
  | { kind: 'steer' }
  | { kind: 'cancel' }
  | { kind: 'approve'; id: string; allow: boolean; forConversation?: boolean; reviewToken?: string }
  | { kind: 'draft'; text: string }
  | { kind: 'dictate'; on: boolean }
  | { kind: 'open'; open: boolean }
  | { kind: 'openInAsk' }
  | { kind: 'close' }
  /** The answer engine's card: install it, check again, or put away the "installed" note. */
  | { kind: 'runtime'; op: RuntimeOp }
  /** A start found the engine there or gone (the card's `onRuntimeInstalled`). */
  | { kind: 'runtimeInstalled'; installed: boolean }
  /** "Review this" on a code block of the answer: main opens its review pane on the snippet. */
  | { kind: 'review'; code: string; language: string }
  /** A web source of the answer, opened in the browser by main (http and https only). */
  | { kind: 'openUrl'; url: string }
  /** Stop a command the answer is running (Code mode). */
  | { kind: 'stopCommand'; id: string }
  | { kind: 'openSettings' };

export const RUNTIME_OPS = ['install', 'refresh', 'dismissInstalled'] as const;
export type RuntimeOp = typeof RUNTIME_OPS[number];

type Kind = DockAction['kind'];

interface Field {
  type: 'string' | 'boolean';
  optional?: true;
}

/** Each kind's fields besides `kind`, their types, and which may be left out. */
const ACTION_FIELDS: Record<Kind, Record<string, Field>> = {
  send: {},
  steer: {},
  cancel: {},
  approve: { id: { type: 'string' }, allow: { type: 'boolean' }, forConversation: { type: 'boolean', optional: true }, reviewToken: { type: 'string', optional: true } },
  draft: { text: { type: 'string' } },
  dictate: { on: { type: 'boolean' } },
  open: { open: { type: 'boolean' } },
  openInAsk: {},
  close: {},
  runtime: { op: { type: 'string' } },
  runtimeInstalled: { installed: { type: 'boolean' } },
  review: { code: { type: 'string' }, language: { type: 'string' } },
  openUrl: { url: { type: 'string' } },
  stopCommand: { id: { type: 'string' } },
  openSettings: {},
};

/** An approval, review token or command id: 1 to 128 printable ASCII characters, no spaces. */
const ID = /^[\x21-\x7e]{1,128}$/;

/** The longest code block the dock may send for review (characters). */
export const MAX_REVIEW_CHARS = 200000;
/** The longest web address the dock may send. */
export const MAX_URL_CHARS = 2048;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isWebAddress(text: string): boolean {
  if (text.length > MAX_URL_CHARS) return false;
  try {
    const url = new URL(text);
    return (url.protocol === 'https:' || url.protocol === 'http:') && url.username === '' && url.password === '';
  } catch {
    return false;
  }
}

/** The dock's action, or null for anything that is not exactly one of `DockAction`. */
export function parseDockAction(value: unknown): DockAction | null {
  if (!isRecord(value) || typeof value.kind !== 'string') return null;
  if (!Object.prototype.hasOwnProperty.call(ACTION_FIELDS, value.kind)) return null;
  const kind = value.kind as Kind;
  const fields = ACTION_FIELDS[kind];
  for (const key of Object.keys(value)) {
    if (key === 'kind') continue;
    if (!Object.prototype.hasOwnProperty.call(fields, key) || typeof value[key] !== fields[key].type) return null;
  }
  for (const [key, field] of Object.entries(fields)) {
    if (!field.optional && !Object.prototype.hasOwnProperty.call(value, key)) return null;
  }
  switch (kind) {
    case 'draft':
      if ((value.text as string).length > MAX_DRAFT_CHARS) return null;
      break;
    case 'approve':
      if (!ID.test(value.id as string)) return null;
      if (value.reviewToken !== undefined && !ID.test(value.reviewToken as string)) return null;
      break;
    case 'runtime':
      if (!(RUNTIME_OPS as readonly string[]).includes(value.op as string)) return null;
      break;
    case 'review':
      if ((value.code as string).length > MAX_REVIEW_CHARS || !/^[\w+#.-]{0,64}$/.test(value.language as string)) return null;
      break;
    case 'openUrl':
      if (!isWebAddress(value.url as string)) return null;
      break;
    case 'stopCommand':
      if (!ID.test(value.id as string)) return null;
      break;
    default:
      break;
  }
  return value as DockAction;
}

function isStringOrNull(value: unknown): value is string | null {
  return value === null || typeof value === 'string';
}

function isTranscript(value: unknown): value is TranscriptState {
  return isRecord(value)
    && typeof value.runId === 'string'
    && typeof value.status === 'string'
    && Array.isArray(value.blocks)
    && isRecord(value.steps);
}

function isNumberOrNull(value: unknown): boolean {
  return value === null || (typeof value === 'number' && Number.isFinite(value));
}

/** The runtime setup state as main publishes it (`runtimeSetupState`). */
function isRuntimeState(value: unknown): value is RuntimeSetupState {
  return isRecord(value)
    && (value.status === null || isRecord(value.status))
    && isStringOrNull(value.statusError)
    && (value.installed === null || typeof value.installed === 'boolean')
    && typeof value.installing === 'boolean'
    && (value.progress === null || (isRecord(value.progress) && typeof value.progress.downloaded === 'number'))
    && isStringOrNull(value.installError)
    && typeof value.installOffline === 'boolean'
    && (value.justInstalled === null || isRecord(value.justInstalled))
    && isNumberOrNull(value.installedAt);
}

/** Main's snapshot as the dock receives it, or null when it is not one. */
export function parseDockSnapshot(value: unknown): DockSnapshot | null {
  if (!isRecord(value) || value.v !== 1) return null;
  const s = value;
  const lastUser = s.lastUser;
  const lastUserOk = lastUser === null || (isRecord(lastUser) && typeof lastUser.content === 'string');
  const ok = isStringOrNull(s.conversationId)
    && typeof s.title === 'string'
    && lastUserOk
    && (s.transcript === null || isTranscript(s.transcript))
    && typeof s.trimmed === 'boolean'
    && isRuntimeState(s.runtime)
    && typeof s.isStreaming === 'boolean'
    && typeof s.busyElsewhere === 'boolean'
    && isStringOrNull(s.sideRunLabel)
    && isStringOrNull(s.readOnlyConnector)
    && typeof s.draft === 'string'
    && typeof s.dictating === 'boolean'
    && typeof s.voiceSupported === 'boolean'
    && typeof s.open === 'boolean'
    && typeof s.unread === 'number' && Number.isFinite(s.unread) && s.unread >= 0
    && (s.theme === 'light' || s.theme === 'dark');
  if (!ok) return null;
  return {
    v: 1,
    conversationId: s.conversationId as string | null,
    title: s.title as string,
    lastUser: lastUser === null ? null : { content: (lastUser as { content: string }).content },
    transcript: s.transcript as TranscriptState | null,
    trimmed: s.trimmed as boolean,
    runtime: s.runtime as RuntimeSetupState,
    isStreaming: s.isStreaming as boolean,
    busyElsewhere: s.busyElsewhere as boolean,
    sideRunLabel: s.sideRunLabel as string | null,
    readOnlyConnector: s.readOnlyConnector as string | null,
    draft: s.draft as string,
    dictating: s.dictating as boolean,
    voiceSupported: s.voiceSupported as boolean,
    open: s.open as boolean,
    unread: s.unread as number,
    theme: s.theme as 'light' | 'dark',
  };
}

/** The session fields the floating chat reads, as `ConversationDock` reads them in main. */
export interface SnapshotInput {
  conversationId: string | null;
  title: string;
  messages: readonly ChatMessage[];
  isStreaming: boolean;
  streamingConversationId: string | null;
  sideRun: { label: string } | null;
  /** The connector of a read-only conversation, if it came from one. */
  readOnlyConnector: string | null;
  draft: string;
  dictating: boolean;
  voiceSupported: boolean;
  open: boolean;
  unread: number;
  theme: 'light' | 'dark';
  runtime: RuntimeSetupState;
}

function jsonBytes(value: unknown): number {
  return new TextEncoder().encode(JSON.stringify(value)).length;
}

/** A step's arguments with each long text cut to `max` characters (a written file's text). */
function shortArgs(args: unknown, max: number): unknown {
  if (typeof args === 'string') return args.length > max ? `${args.slice(0, max)}…` : args;
  if (Array.isArray(args)) return args.map(item => shortArgs(item, max));
  if (isRecord(args)) {
    const out: Record<string, unknown> = {};
    for (const [key, item] of Object.entries(args)) out[key] = shortArgs(item, max);
    return out;
  }
  return args;
}

/**
 * The transcript, made to fit `budget` bytes of JSON only when it does not already: first each
 * step's arguments and details are cut short (the text of a file it wrote, a passage), then the
 * oldest blocks go, so the newest activity, which the chat scrolls to, always shows.
 */
export function fitTranscript(transcript: TranscriptState, budget: number): TranscriptState {
  if (jsonBytes(transcript) <= budget) return transcript;
  const steps: TranscriptState['steps'] = {};
  for (const [id, step] of Object.entries(transcript.steps)) {
    steps[id] = { ...step, args: shortArgs(step.args, 400), detail: shortArgs(step.detail, 400) };
  }
  let fitted: TranscriptState = { ...transcript, steps };
  let blocks = fitted.blocks;
  while (blocks.length > 1 && jsonBytes(fitted) > budget) {
    blocks = blocks.slice(Math.ceil(blocks.length / 4));
    const kept = new Set(blocks.flatMap(b => (b.kind === 'step' ? [b.stepId] : [])));
    const keptSteps: TranscriptState['steps'] = {};
    for (const [id, step] of Object.entries(steps)) {
      // A kept step's nested steps stay with it.
      if (kept.has(id) || (step.parentId !== null && step.parentId !== undefined && kept.has(step.parentId))) keptSteps[id] = step;
    }
    fitted = { ...fitted, blocks, steps: keptSteps };
  }
  return fitted;
}

/** Room left for the transcript in a snapshot, after the other fields and some headroom. */
const SNAPSHOT_HEADROOM = 8192;

/**
 * What the floating chat shows: only the active conversation's last exchange. `relay` (the
 * snapshot main publishes to the dock) fits it to the relay's size; the in-page chat, which reads
 * main's state directly, passes false and gets it whole.
 */
export function snapshotOf(input: SnapshotInput, relay = true): DockSnapshot {
  let lastUser: ChatMessage | null = null;
  let lastAnswer: ChatMessage | null = null;
  for (let i = input.messages.length - 1; i >= 0 && (!lastUser || !lastAnswer); i -= 1) {
    const message = input.messages[i];
    if (!lastUser && message.role === 'user') lastUser = message;
    if (!lastAnswer && message.role === 'assistant' && message.transcript) lastAnswer = message;
  }
  const busyElsewhere = (input.streamingConversationId !== null && !input.isStreaming) || input.sideRun !== null;
  const base: DockSnapshot = {
    v: 1,
    conversationId: input.conversationId,
    title: input.title,
    lastUser: lastUser ? { content: lastUser.content } : null,
    transcript: null,
    trimmed: false,
    runtime: input.runtime,
    isStreaming: input.isStreaming,
    busyElsewhere,
    sideRunLabel: input.sideRun?.label ?? null,
    readOnlyConnector: input.readOnlyConnector,
    draft: relay && input.draft.length > MAX_DRAFT_CHARS ? input.draft.slice(0, MAX_DRAFT_CHARS) : input.draft,
    dictating: input.dictating,
    voiceSupported: input.voiceSupported,
    open: input.open,
    unread: input.unread,
    theme: input.theme,
  };
  const transcript = lastAnswer?.transcript ?? null;
  if (!transcript) return base;
  if (!relay) return { ...base, transcript };
  const budget = MAX_SNAPSHOT_BYTES - jsonBytes(base) - SNAPSHOT_HEADROOM;
  const fitted = fitTranscript(transcript, budget);
  return { ...base, transcript: fitted, trimmed: fitted.blocks.length < transcript.blocks.length };
}

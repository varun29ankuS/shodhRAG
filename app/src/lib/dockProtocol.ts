/**
 * The floating chat over the embedded Artifacts preview (`dock_layer.rs`) runs in a webview of
 * its own, `dock`, which holds no chat session. The main webview is the only session: it
 * publishes a snapshot of what the chat shows (`DockSnapshot`), and the dock sends the user's
 * clicks and keys back (`DockAction`), which main checks with `parseDockAction` before acting.
 * Pure: no React, type-only imports, so the Node tests load it.
 */

import type { TranscriptState } from '../features/agent/reducer';
import type { ChatMessage } from '../features/ask/types';

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
  /** The last answer's transcript (live while it runs). */
  transcript: TranscriptState | null;
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
  /** Main's text-size zoom, which the dock page is drawn at too. */
  zoom: number;
}

/** What the dock may ask main to do. Anything else is refused. */
export type DockAction =
  | { kind: 'send' }
  | { kind: 'steer' }
  | { kind: 'cancel' }
  | { kind: 'approve'; id: string; allow: boolean }
  | { kind: 'draft'; text: string }
  | { kind: 'dictate'; on: boolean }
  | { kind: 'open'; open: boolean }
  | { kind: 'openInAsk' }
  | { kind: 'close' };

type Kind = DockAction['kind'];

/** Each kind's fields besides `kind`, and their types. */
const ACTION_FIELDS: Record<Kind, Record<string, 'string' | 'boolean'>> = {
  send: {},
  steer: {},
  cancel: {},
  approve: { id: 'string', allow: 'boolean' },
  draft: { text: 'string' },
  dictate: { on: 'boolean' },
  open: { open: 'boolean' },
  openInAsk: {},
  close: {},
};

/** An approval id: 1 to 128 printable ASCII characters, no spaces. */
const APPROVAL_ID = /^[\x21-\x7e]{1,128}$/;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** The dock's action, or null for anything that is not exactly one of `DockAction`. */
export function parseDockAction(value: unknown): DockAction | null {
  if (!isRecord(value) || typeof value.kind !== 'string') return null;
  if (!Object.prototype.hasOwnProperty.call(ACTION_FIELDS, value.kind)) return null;
  const kind = value.kind as Kind;
  const fields = ACTION_FIELDS[kind];
  const keys = Object.keys(value).filter(key => key !== 'kind');
  if (keys.length !== Object.keys(fields).length) return null;
  for (const key of keys) {
    if (!Object.prototype.hasOwnProperty.call(fields, key) || typeof value[key] !== fields[key]) return null;
  }
  if (kind === 'draft' && (value.text as string).length > MAX_DRAFT_CHARS) return null;
  if (kind === 'approve' && !APPROVAL_ID.test(value.id as string)) return null;
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
    && typeof s.isStreaming === 'boolean'
    && typeof s.busyElsewhere === 'boolean'
    && isStringOrNull(s.sideRunLabel)
    && isStringOrNull(s.readOnlyConnector)
    && typeof s.draft === 'string'
    && typeof s.dictating === 'boolean'
    && typeof s.voiceSupported === 'boolean'
    && typeof s.open === 'boolean'
    && typeof s.unread === 'number' && Number.isFinite(s.unread) && s.unread >= 0
    && (s.theme === 'light' || s.theme === 'dark')
    && typeof s.zoom === 'number' && Number.isFinite(s.zoom) && s.zoom > 0;
  if (!ok) return null;
  return {
    v: 1,
    conversationId: s.conversationId as string | null,
    title: s.title as string,
    lastUser: lastUser === null ? null : { content: (lastUser as { content: string }).content },
    transcript: s.transcript as TranscriptState | null,
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
    zoom: s.zoom as number,
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
  zoom: number;
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

/** What main publishes for the floating chat: only the active conversation's last exchange. */
export function snapshotOf(input: SnapshotInput): DockSnapshot {
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
    isStreaming: input.isStreaming,
    busyElsewhere,
    sideRunLabel: input.sideRun?.label ?? null,
    readOnlyConnector: input.readOnlyConnector,
    draft: input.draft.length > MAX_DRAFT_CHARS ? input.draft.slice(0, MAX_DRAFT_CHARS) : input.draft,
    dictating: input.dictating,
    voiceSupported: input.voiceSupported,
    open: input.open,
    unread: input.unread,
    theme: input.theme,
    zoom: input.zoom,
  };
  const transcript = lastAnswer?.transcript ?? null;
  if (!transcript) return base;
  const budget = MAX_SNAPSHOT_BYTES - jsonBytes(base) - SNAPSHOT_HEADROOM;
  return { ...base, transcript: fitTranscript(transcript, budget) };
}

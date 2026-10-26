/**
 * Connector conversations in the app: Shodh's exchanges in Slack and Telegram, kept by the
 * backend in the same store as Ask chats (`app/src-tauri/src/connectors/conversations.rs`)
 * with a `source`. The app shows them read-only, grouped per chat in the sidebar
 * ("Telegram · you", "Slack · #deals"), and "Continue in Ask" starts an Ask chat that
 * carries one of them as quoted context: the messages of anyone but the user are marked as
 * not theirs, and the backend introduces the whole as untrusted text
 * (`agent_session_commands.rs::CONNECTOR_LEAD`). It is attached to the first question,
 * never typed by the user, so it is never learned from.
 *
 * Pure, so `node --test` loads it (`app/tests/connectorConversations.test.ts`).
 */

/** Every connector conversation's id starts with this (the backend's `ID_PREFIX`). */
export const CONNECTOR_ID_PREFIX = 'connector-';

/** Where a connector conversation happened (`ConversationRecord.source`). */
export interface ConversationSource {
  /** `slack` or `telegram`. */
  connector: string;
  chat: string;
  /** "Slack · #deals", "Telegram · you". */
  label: string;
  link?: string;
}

/** What a chat started with "Continue in Ask" carries (`ConversationRecord.seed`). */
export interface ConversationSeed {
  /** The connector conversation it continues. */
  from: string;
  connector: string;
  label: string;
  /** Messages carried. */
  messages: number;
  /** The conversation as quoted text (see {@link seedText}). */
  text: string;
}

interface MessageLike {
  role: string;
  content: string;
  metadata?: unknown;
}

interface ConversationLike {
  id: string;
  updatedAt: string;
  source?: ConversationSource;
  messages: readonly MessageLike[];
}

/** Whether `conversation` happened in a connector (read-only in the app). */
export function isConnectorConversation(conversation: { id: string; source?: unknown } | null | undefined): boolean {
  if (!conversation) return false;
  return conversation.id.startsWith(CONNECTOR_ID_PREFIX) || conversation.source != null;
}

/** The connector's name as people know it. */
export function connectorName(connector: string): string {
  if (connector === 'slack') return 'Slack';
  if (connector === 'telegram') return 'Telegram';
  return 'a connector';
}

/** Who wrote a message of a connector conversation (its metadata `connector`). */
export interface ConnectorAuthor {
  name: string;
  /** The user themselves. */
  owner: boolean;
  /** Shodh's answer. */
  shodh: boolean;
  link?: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** The author of `message`, from what the connector stored with it. */
export function messageAuthor(message: MessageLike): ConnectorAuthor {
  const meta = isRecord(message.metadata) ? message.metadata.connector : undefined;
  const info = isRecord(meta) ? meta : {};
  if (message.role === 'assistant') return { name: 'Shodh', owner: false, shodh: true };
  const owner = info.owner === true;
  const name = typeof info.author === 'string' && info.author.trim() ? info.author.trim() : owner ? 'You' : 'Someone';
  const link = typeof info.link === 'string' && /^https:\/\//.test(info.link) ? info.link : undefined;
  return { name: owner ? 'You' : name, owner, shodh: false, ...(link ? { link } : {}) };
}

/** One sidebar group: the conversations of one connector chat label. */
export interface ConnectorGroup<T> {
  key: string;
  connector: string;
  label: string;
  items: T[];
}

/**
 * The connector conversations of `conversations`, grouped by connector and label, the
 * group with the newest conversation first, each group's newest first.
 */
export function connectorGroups<T extends ConversationLike>(conversations: readonly T[]): ConnectorGroup<T>[] {
  const groups = new Map<string, ConnectorGroup<T>>();
  for (const c of conversations) {
    if (!c.source) continue;
    const key = `${c.source.connector}:${c.source.label}`;
    let group = groups.get(key);
    if (!group) {
      group = { key, connector: c.source.connector, label: c.source.label, items: [] };
      groups.set(key, group);
    }
    group.items.push(c);
  }
  const newest = (g: ConnectorGroup<T>) => g.items.reduce((max, c) => (c.updatedAt > max ? c.updatedAt : max), '');
  for (const g of groups.values()) g.items.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  return [...groups.values()].sort((a, b) => newest(b).localeCompare(newest(a)));
}

/** The conversation to open when the app starts: the newest Ask chat, never a connector's. */
export function firstAskConversation<T extends { id: string; source?: unknown }>(conversations: readonly T[]): T | null {
  return conversations.find(c => !isConnectorConversation(c)) ?? null;
}

/** Most characters of a connector conversation carried into Ask (the backend takes 20 000). */
export const MAX_SEED_CHARS = 12_000;

/** Characters of one message carried. */
export const MAX_SEED_MESSAGE_CHARS = 2_000;

const LEFT_OUT = '(Earlier messages left out.)';

function oneMessage(text: string): string {
  const trimmed = text.trim();
  const chars = Array.from(trimmed);
  return chars.length <= MAX_SEED_MESSAGE_CHARS ? trimmed : `${chars.slice(0, MAX_SEED_MESSAGE_CHARS - 1).join('')}…`;
}

/** Longest author name carried. */
const MAX_NAME_CHARS = 60;

/**
 * A name as a block heading: one line, no control characters, no quote mark or colon, so a
 * name cannot look like a heading or a quoted line of its own.
 */
export function safeName(name: string): string {
  // eslint-disable-next-line no-control-regex
  const flat = name.replace(/[\u0000-\u001f\u007f-\u009f\u2028\u2029]/g, ' ').replace(/[>:]/g, ' ');
  const one = flat.split(/\s+/).filter(Boolean).join(' ');
  const chars = Array.from(one);
  const cut = chars.length <= MAX_NAME_CHARS ? one : `${chars.slice(0, MAX_NAME_CHARS - 1).join('')}…`;
  return cut || 'Someone';
}

/** `content` with every line quoted (`> `), so no line of it can pass for a heading. */
function quoted(content: string): string {
  return content
    .split(/\r\n|\r|\n|\u2028|\u2029/)
    .map(line => `> ${line}`)
    .join('\n');
}

/**
 * `messages` as quoted text: one block per message, a heading naming who wrote it ("I
 * wrote:", "Shodh answered:", or "Alex (not me) wrote:"), then every line of the message
 * quoted with `> `. A message cannot forge another author's block: none of its lines can
 * start a heading. The newest messages are kept when it is too long, with a line saying
 * earlier ones were left out.
 */
export function seedText(messages: readonly MessageLike[]): { text: string; messages: number } {
  const blocks: string[] = [];
  for (const m of messages) {
    if (m.role !== 'user' && m.role !== 'assistant') continue;
    const content = oneMessage(m.content);
    if (!content) continue;
    const author = messageAuthor(m);
    const heading = author.shodh ? 'Shodh answered:' : author.owner ? 'I wrote:' : `${safeName(author.name)} (not me) wrote:`;
    blocks.push(`${heading}\n${quoted(content)}`);
  }
  const kept: string[] = [];
  let size = 0;
  for (let i = blocks.length - 1; i >= 0; i--) {
    const length = Array.from(blocks[i]).length + 2;
    if (size + length > MAX_SEED_CHARS - LEFT_OUT.length - 2) break;
    kept.unshift(blocks[i]);
    size += length;
  }
  const text = kept.length < blocks.length ? [LEFT_OUT, ...kept].join('\n\n') : kept.join('\n\n');
  return { text, messages: kept.length };
}

/** What "Continue in Ask" carries from `conversation`; `null` when it has nothing to carry. */
export function continueSeed(conversation: ConversationLike): ConversationSeed | null {
  if (!conversation.source) return null;
  const { text, messages } = seedText(conversation.messages);
  if (messages === 0) return null;
  return {
    from: conversation.id,
    connector: conversation.source.connector,
    label: conversation.source.label,
    messages,
    text,
  };
}

/** The seed stored on a conversation, or `null` when absent or malformed. */
export function readSeed(value: unknown): ConversationSeed | null {
  if (!isRecord(value)) return null;
  const { from, connector, label, messages, text } = value;
  if (typeof from !== 'string' || typeof connector !== 'string' || typeof label !== 'string') return null;
  if (typeof messages !== 'number' || typeof text !== 'string' || !text.trim()) return null;
  return { from, connector, label, messages, text };
}

/**
 * Whether the next question in a chat carries its seed: only the first question (later
 * ones replay it with the conversation, as the first question's attached context).
 */
export function seedGoesWithNextQuestion(seed: ConversationSeed | null, messages: readonly { role: string }[]): boolean {
  return seed !== null && !messages.some(m => m.role === 'user');
}

/** The line under a read-only connector conversation. */
export function readOnlyLine(source: ConversationSource): string {
  const name = connectorName(source.connector);
  return `This conversation happened in ${name}. Shodh keeps it here to read; answer in ${name}, or continue it in Ask.`;
}

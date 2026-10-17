/**
 * Text read from an image (pasted into the composer, added with the image button, or
 * dropped on the window) waits as an attachment of the conversation's next question:
 * a removable chip in the composer. It goes out with that question as context the user
 * provided, separately from the words they typed (`agent_send`'s `attachedContext`):
 * the backend appends it, fenced, to the message to the model only, so learning, the
 * audit's question and the answer check keep the typed words, and it is never an
 * assistant turn. The user message keeps it (metadata `imageText`) so a replay into a
 * fresh session reads it the same way. Kept per conversation for the session, like the
 * draft. No imports: unit-tested under Node.
 */

export type ImageTextOrigin = 'pasted' | 'added' | 'dropped';

/** Text the user attached to a question; the wire shape of `agent_send`'s `attachedContext`. */
export interface AttachedContext {
  origin: ImageTextOrigin;
  /** The text read from the image (OCR). */
  text: string;
}

export interface ImageTextAttachment extends AttachedContext {
  id: string;
}

const ORIGINS: readonly ImageTextOrigin[] = ['pasted', 'added', 'dropped'];

const CHIP_NOUN: Record<ImageTextOrigin, string> = {
  pasted: 'pasted image',
  added: 'added image',
  dropped: 'dropped image',
};

const MESSAGE_LEAD: Record<ImageTextOrigin, string> = {
  pasted: 'Text from an image I pasted:',
  added: 'Text from an image I added:',
  dropped: 'Text from an image I dropped:',
};

/** The chip's label: "Text from pasted image · 1,204 characters". */
export function attachmentLabel(attachment: AttachedContext): string {
  const count = attachment.text.length;
  return `Text from ${CHIP_NOUN[attachment.origin]} · ${count.toLocaleString('en-US')} ${count === 1 ? 'character' : 'characters'}`;
}

/** What goes out with a question: the attachments with text, without their chip ids. */
export function attachedContextOf(attachments: readonly ImageTextAttachment[]): AttachedContext[] {
  return attachments.filter(a => a.text.trim().length > 0).map(a => ({ origin: a.origin, text: a.text }));
}

/** A code fence longer than any run of backticks in `text`, so the text cannot close it. */
function fenceFor(text: string): string {
  let longest = 0;
  for (const run of text.match(/`+/g) ?? []) longest = Math.max(longest, run.length);
  return '`'.repeat(Math.max(3, longest + 1));
}

/**
 * The question followed by each attachment, fenced as text the user provided. The
 * backend composes the message to the model the same way
 * (`agent_session_commands.rs::with_attached_context`); the app uses it to replay
 * such a turn into a fresh session.
 */
export function composeQuestion(question: string, attached: readonly AttachedContext[]): string {
  const parts = [question.trim()];
  for (const item of attached) {
    const text = item.text.trim();
    if (!text) continue;
    const fence = fenceFor(text);
    parts.push(`${MESSAGE_LEAD[item.origin]}\n${fence}\n${text}\n${fence}`);
  }
  return parts.filter(p => p.length > 0).join('\n\n');
}

/** The metadata key a user message keeps its attached image text under. */
export const IMAGE_TEXT_METADATA_KEY = 'imageText';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** The attached image text stored on a message ([] when none or malformed). */
export function readAttachedContext(metadata: unknown): AttachedContext[] {
  if (!isRecord(metadata)) return [];
  const value = metadata[IMAGE_TEXT_METADATA_KEY];
  if (!Array.isArray(value)) return [];
  const out: AttachedContext[] = [];
  for (const item of value) {
    if (!isRecord(item) || typeof item.text !== 'string' || !item.text.trim()) continue;
    const origin = ORIGINS.find(o => o === item.origin);
    if (origin) out.push({ origin, text: item.text });
  }
  return out;
}

/** Metadata with the attached image text set (removed when empty), keeping every other key. */
export function metadataWithAttachedContext(metadata: unknown, attached: readonly AttachedContext[]): Record<string, unknown> | undefined {
  const base: Record<string, unknown> = isRecord(metadata) ? { ...metadata } : {};
  delete base[IMAGE_TEXT_METADATA_KEY];
  if (attached.length > 0) base[IMAGE_TEXT_METADATA_KEY] = attached.map(a => ({ origin: a.origin, text: a.text }));
  return Object.keys(base).length > 0 ? base : undefined;
}

type Listener = () => void;

const NO_CONVERSATION = '';
const NONE: readonly ImageTextAttachment[] = [];

export class AttachmentStore {
  private readonly lists = new Map<string, readonly ImageTextAttachment[]>();
  private readonly listeners = new Set<Listener>();

  /** The attachments waiting for the next question of `conversationId` (the same array until it changes). */
  get(conversationId: string | null): readonly ImageTextAttachment[] {
    return this.lists.get(conversationId ?? NO_CONVERSATION) ?? NONE;
  }

  /** Attach text to the next question; empty text is not attached. */
  add(conversationId: string | null, attachment: ImageTextAttachment): void {
    if (attachment.text.trim().length === 0) return;
    this.put(conversationId, [...this.get(conversationId), attachment]);
  }

  /** Remove one attachment (its chip's remove button). */
  remove(conversationId: string | null, id: string): void {
    const current = this.get(conversationId);
    if (!current.some(a => a.id === id)) return;
    this.put(conversationId, current.filter(a => a.id !== id));
  }

  /**
   * Remove the attachments a question was sent with (read with `get` before sending);
   * any attached while it was being sent stay for the next one.
   */
  consume(conversationId: string | null, sent: readonly ImageTextAttachment[]): void {
    const ids = new Set(sent.map(a => a.id));
    const current = this.get(conversationId);
    if (!current.some(a => ids.has(a.id))) return;
    this.put(conversationId, current.filter(a => !ids.has(a.id)));
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  private put(conversationId: string | null, list: readonly ImageTextAttachment[]): void {
    const key = conversationId ?? NO_CONVERSATION;
    if (list.length === 0) this.lists.delete(key);
    else this.lists.set(key, list);
    for (const listener of [...this.listeners]) listener();
  }
}

/** The attachments the Ask composer and the dock share. */
export const composerAttachments = new AttachmentStore();

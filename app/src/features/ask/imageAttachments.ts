/**
 * Text read from an image (pasted into the composer, added with the image button, or
 * dropped on the window) waits as an attachment of the conversation's next question:
 * a removable chip in the composer. On send it becomes part of the user's own message,
 * fenced as context they provided, so the model sees it as the user's words and it is
 * never an assistant turn. Kept per conversation for the session, like the draft.
 * No imports: unit-tested under Node.
 */

export type ImageTextOrigin = 'pasted' | 'added' | 'dropped';

export interface ImageTextAttachment {
  id: string;
  origin: ImageTextOrigin;
  /** The text read from the image (OCR). */
  text: string;
}

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
export function attachmentLabel(attachment: ImageTextAttachment): string {
  const count = attachment.text.length;
  return `Text from ${CHIP_NOUN[attachment.origin]} · ${count.toLocaleString('en-US')} ${count === 1 ? 'character' : 'characters'}`;
}

/** A code fence longer than any run of backticks in `text`, so the text cannot close it. */
function fenceFor(text: string): string {
  let longest = 0;
  for (const run of text.match(/`+/g) ?? []) longest = Math.max(longest, run.length);
  return '`'.repeat(Math.max(3, longest + 1));
}

/** The message sent: the question, then each attachment fenced as user-provided context. */
export function composeQuestion(question: string, attachments: readonly ImageTextAttachment[]): string {
  const parts = [question.trim()];
  for (const attachment of attachments) {
    const text = attachment.text.trim();
    if (!text) continue;
    const fence = fenceFor(text);
    parts.push(`${MESSAGE_LEAD[attachment.origin]}\n${fence}\n${text}\n${fence}`);
  }
  return parts.filter(p => p.length > 0).join('\n\n');
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

  /** Take every attachment (they went out with a question). */
  take(conversationId: string | null): readonly ImageTextAttachment[] {
    const current = this.get(conversationId);
    if (current.length > 0) this.put(conversationId, NONE);
    return current;
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

/**
 * The unsent question of each conversation. Ask unmounts on every view switch
 * (also when the agent opens another view mid-answer), and the conversation dock
 * edits the same text, so the draft lives here rather than in either component;
 * "Open in Ask" shows what was typed in the dock. Kept for the session only.
 * No imports: unit-tested under Node.
 */

type Listener = () => void;

/** The key of a draft written while no conversation is open. */
const NO_CONVERSATION = '';

export class DraftStore {
  private readonly drafts = new Map<string, string>();
  private readonly listeners = new Set<Listener>();

  /** The draft of `conversationId` ('' when none). */
  get(conversationId: string | null): string {
    return this.drafts.get(conversationId ?? NO_CONVERSATION) ?? '';
  }

  /** Replace the draft of `conversationId`; an empty one is forgotten. */
  set(conversationId: string | null, text: string): void {
    const key = conversationId ?? NO_CONVERSATION;
    if ((this.drafts.get(key) ?? '') === text) return;
    if (text.length === 0) this.drafts.delete(key);
    else this.drafts.set(key, text);
    for (const listener of [...this.listeners]) listener();
  }

  /** Change the draft of `conversationId` from its current text (e.g. append a snippet). */
  update(conversationId: string | null, change: (current: string) => string): void {
    this.set(conversationId, change(this.get(conversationId)));
  }

  /** Called after every change; returns the unsubscribe function. */
  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }
}

/** The drafts the Ask composer and the dock share. */
export const composerDrafts = new DraftStore();

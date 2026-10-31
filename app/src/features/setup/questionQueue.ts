/**
 * Questions asked before Shodh is ready, waiting for setup: one per conversation,
 * kept outside any view so a question still goes out when the user has moved to
 * the Library or Settings while the download runs. Each waiting question carries
 * the send its composer prepared; it leaves the queue only once that send was
 * accepted, so a send refused for now (an answer still running) is tried again.
 *
 * No imports: unit-tested under Node (`app/tests/questionQueue.test.ts`).
 */

/** Sends a waiting question; true when it was accepted (the question is on its way). */
export type QueuedSend = () => boolean;

interface Entry {
  send: QueuedSend;
  /** Sends refused so far (the user is told once). */
  refused: number;
}

/** What one attempt to send a conversation's waiting question did. */
export type FlushResult = 'none' | 'sent' | 'refused';

type Listener = () => void;

export class QuestionQueue {
  private readonly entries = new Map<string, Entry>();
  private readonly listeners = new Set<Listener>();

  /** Hold `conversationId`'s question until setup is ready (a later hold replaces it). */
  hold(conversationId: string, send: QueuedSend): void {
    this.entries.set(conversationId, { send, refused: 0 });
    this.changed();
  }

  /** Don't send it (its text stays in the composer). */
  cancel(conversationId: string): void {
    if (this.entries.delete(conversationId)) this.changed();
  }

  isWaiting(conversationId: string | null): boolean {
    return conversationId !== null && this.entries.has(conversationId);
  }

  /** How many sends of the waiting question were refused. */
  refusals(conversationId: string): number {
    return this.entries.get(conversationId)?.refused ?? 0;
  }

  /**
   * Setup is ready and `activeId` is shown: send its waiting question. It leaves the
   * queue only when the send was accepted; a refused one stays for the next attempt.
   * Questions of other conversations wait until theirs is shown (a send goes to the
   * conversation on screen).
   */
  flush(activeId: string | null): FlushResult {
    if (activeId === null) return 'none';
    const entry = this.entries.get(activeId);
    if (!entry) return 'none';
    let accepted = false;
    try {
      accepted = entry.send();
    } catch {
      accepted = false;
    }
    if (accepted) {
      this.entries.delete(activeId);
      this.changed();
      return 'sent';
    }
    entry.refused += 1;
    this.changed();
    return 'refused';
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  private changed(): void {
    for (const listener of [...this.listeners]) listener();
  }
}

/** The app's queue (Ask and the dock hold questions in it; the app sends them). */
export const questionQueue = new QuestionQueue();

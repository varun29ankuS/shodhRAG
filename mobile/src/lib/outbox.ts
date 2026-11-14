/**
 * What the phone sends when the computer is back: questions asked, files
 * added and the user's own edits made while it was asleep or away. The queue
 * is kept sealed on the phone, in order (mobile/src-tauri/src/queue.rs,
 * crates/shodh-link/src/outbox.rs), so a kill of the app loses nothing.
 *
 * The sender works through it strictly in order, one item at a time: a
 * question is sent and its answer waited for before the next (the computer
 * answers one question at a time in a conversation), then the next item.
 * Every item keeps the ids of its first try. One found `sending` after a
 * restart may have reached the computer: the sender looks for its question
 * in the conversation before sending it again, so a question is never asked
 * twice.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { answerText, isLive, savedAnswer } from './answer';
import { ask, AskStopped, historyOf, type Attached, type CarriedPassage } from './ask';
import { CONVERSATION_CHANGED, loadConversation, type SavedConversation } from './desktop';
import type { Shim } from './invoke';
import { errorText } from './link';
import type { Subscriptions } from './subscriptions';

export type QueueState = 'waiting' | 'sending' | 'sent' | 'answered' | 'failed';

export interface QueuedQuestion {
  kind: 'question';
  conversationId: string;
  text: string;
  workspaceId: string | null;
  requestId: string;
  questionId: string;
  answerId: string;
  askedAt: number;
  attached: Attached[];
  passages: CarriedPassage[];
}

export interface QueuedFile {
  kind: 'file';
  localPath: string;
  name: string;
  size: number;
  sha256: string;
  workspaceId: string | null;
}

export interface QueuedEdit {
  kind: 'edit';
  command: string;
  args: Record<string, unknown>;
  label: string;
}

export type Work = QueuedQuestion | QueuedFile | QueuedEdit;

export interface QueuedItem {
  id: string;
  queuedAt: number;
  state: QueueState;
  work: Work;
  error: string | null;
  changedAt: number;
}

/** Where the queue is kept (the app's Rust side; a list in tests). */
export interface OutboxSource {
  list(): Promise<QueuedItem[]>;
  add(id: string, work: Work): Promise<QueuedItem>;
  mark(id: string, state: QueueState, error?: string | null): Promise<QueuedItem>;
  remove(id: string): Promise<void>;
  onChanged(handler: () => void): Promise<() => void>;
}

export const QUEUE_EVENT = 'outbox://changed';

export const tauriOutbox: OutboxSource = {
  list: () => invoke<QueuedItem[]>('outbox_list'),
  add: (id, work) => invoke<QueuedItem>('outbox_add', { id, work }),
  mark: (id, state, error = null) => invoke<QueuedItem>('outbox_mark', { id, state, error }),
  remove: id => invoke<void>('outbox_remove', { id }),
  onChanged: handler => listen(QUEUE_EVENT, () => handler()),
};

/** The subscription owner of the answer the sender waits for. */
export const OUTBOX_OWNER = 'outbox';

export function finished(state: QueueState): boolean {
  return state === 'answered' || state === 'failed';
}

/** A new question to queue, with the ids every try sends. */
export function queuedQuestion(
  question: { conversationId: string; text: string; workspaceId: string | null; attached?: Attached[]; passages?: CarriedPassage[] },
  newId: () => string = () => crypto.randomUUID(),
  now: () => number = Date.now,
): QueuedQuestion {
  return {
    kind: 'question',
    conversationId: question.conversationId,
    text: question.text,
    workspaceId: question.workspaceId,
    requestId: newId(),
    questionId: newId(),
    answerId: newId(),
    askedAt: now(),
    attached: question.attached ?? [],
    passages: question.passages ?? [],
  };
}

/** Words for one item, as the queue lists it. */
export function itemWords(item: QueuedItem): { title: string; state: string } {
  const title = item.work.kind === 'question' ? item.work.text : item.work.kind === 'file' ? item.work.name : item.work.label;
  const state =
    item.state === 'waiting'
      ? 'Sent when your computer wakes'
      : item.state === 'sending'
        ? 'Sending'
        : item.state === 'sent'
          ? item.work.kind === 'question'
            ? 'Sent. The answer is on its way.'
            : 'Sent'
          : item.state === 'answered'
            ? item.work.kind === 'question'
              ? 'Answered'
              : item.work.kind === 'file'
                ? 'Added to Shodh on your computer'
                : 'Done on your computer'
            : item.work.kind === 'file'
              ? `Not added: ${item.error ?? 'your computer did not add it.'}`
              : `Not sent: ${item.error ?? 'your computer refused it.'}`;
  return { title, state };
}

/** Whether `error` means the computer is not reachable now (try again later), not a refusal. */
export function isOffline(error: unknown): boolean {
  if (error instanceof AskStopped) return true;
  return typeof error === 'object' && error !== null && 'code' in error && (error as { code: unknown }).code === 'offline';
}

/** The question's answer in `conversation`, when it is there and finished. */
export function answerFinished(conversation: SavedConversation | null | undefined, answerId: string): boolean {
  const message = conversation?.messages.find(m => m.id === answerId);
  if (!message) return false;
  const answer = savedAnswer(message.content, message.transcript);
  return !isLive(answer) && (answerText(answer) !== '' || answer.status !== 'completed');
}

export interface SenderDeps {
  desktop: Shim;
  subscriptions: Subscriptions;
  /** Send one queued file (lib/transfer.ts); resolves once the computer has it. */
  sendFile?: (item: QueuedItem & { work: QueuedFile }, signal: AbortSignal) => Promise<void>;
}

/**
 * Sends the queue in order while the computer is connected. `run` works
 * until the queue is done or the connection ends (`signal`); calling it again
 * while it runs only asks it to look again.
 */
export class OutboxSender {
  private running = false;
  private again = false;

  constructor(
    private readonly source: OutboxSource,
    private readonly deps: SenderDeps,
  ) {}

  async run(signal: AbortSignal): Promise<void> {
    if (this.running) {
      this.again = true;
      return;
    }
    this.running = true;
    try {
      do {
        this.again = false;
        while (!signal.aborted) {
          const items = await this.source.list();
          const next = items.find(i => !finished(i.state));
          if (!next) break;
          const done = await this.send(next, signal);
          if (!done) return;
        }
      } while (this.again && !signal.aborted);
    } finally {
      this.running = false;
    }
  }

  /** Send one item. False when the computer went away (the item stays as it is). */
  private async send(item: QueuedItem, signal: AbortSignal): Promise<boolean> {
    try {
      switch (item.work.kind) {
        case 'question':
          await this.sendQuestion(item, item.work, signal);
          break;
        case 'edit':
          if (item.state === 'waiting') await this.source.mark(item.id, 'sending');
          await this.deps.desktop.invoke(item.work.command, item.work.args);
          await this.source.mark(item.id, 'sent');
          // An edit is done once it is sent: nothing comes back to wait for.
          await this.source.mark(item.id, 'answered');
          break;
        case 'file': {
          if (!this.deps.sendFile) throw new Error('Files cannot be sent from here.');
          if (item.state === 'waiting') await this.source.mark(item.id, 'sending');
          await this.deps.sendFile(item as QueuedItem & { work: QueuedFile }, signal);
          await this.source.mark(item.id, 'sent');
          await this.source.mark(item.id, 'answered');
          break;
        }
      }
      return true;
    } catch (error) {
      if (signal.aborted || isOffline(error)) return false;
      await this.source.mark(item.id, 'failed', errorText(error));
      return true;
    }
  }

  private async sendQuestion(item: QueuedItem, question: QueuedQuestion, signal: AbortSignal): Promise<void> {
    const { desktop, subscriptions } = this.deps;
    let state = item.state;
    if (state === 'sending') {
      // Killed while it was being sent: the conversation says whether the computer took it.
      const saved = await loadConversation(desktop, question.conversationId);
      if (saved?.messages.some(m => m.id === question.questionId)) {
        await this.source.mark(item.id, 'sent');
        state = 'sent';
      }
    }
    if (state === 'sent') {
      await this.waitForAnswer(item, question, signal);
      return;
    }
    if (state === 'waiting') await this.source.mark(item.id, 'sending');
    const saved = await loadConversation(desktop, question.conversationId);
    let marked: Promise<unknown> = Promise.resolve();
    await ask(
      desktop,
      subscriptions,
      {
        conversationId: question.conversationId,
        text: question.text,
        history: historyOf(saved),
        workspaceId: question.workspaceId,
        attached: question.attached,
        passages: question.passages,
      },
      {
        ids: { requestId: question.requestId, questionId: question.questionId, answerId: question.answerId },
        askedAt: question.askedAt,
        owner: OUTBOX_OWNER,
        signal,
        onAnswer: () => undefined,
        onSent: () => {
          marked = this.source.mark(item.id, 'sent');
        },
      },
    );
    await marked;
    await subscriptions.set(OUTBOX_OWNER, []);
    await this.source.mark(item.id, 'answered');
  }

  /** A question the computer took before: wait until its saved answer is finished. */
  private async waitForAnswer(item: QueuedItem, question: QueuedQuestion, signal: AbortSignal): Promise<void> {
    const { desktop } = this.deps;
    await new Promise<void>((resolve, reject) => {
      let stop: (() => void) | null = null;
      let settled = false;
      const done = (error?: unknown) => {
        if (settled) return;
        settled = true;
        stop?.();
        signal.removeEventListener('abort', aborted);
        if (error === undefined) resolve();
        else reject(error);
      };
      const aborted = () => done(new AskStopped('The connection ended.'));
      const check = () => {
        loadConversation(desktop, question.conversationId)
          .then(saved => {
            if (answerFinished(saved, question.answerId)) done();
          })
          .catch(done);
      };
      signal.addEventListener('abort', aborted, { once: true });
      // Listen first, then look: a change between the two is not missed.
      desktop
        .listen<{ id?: string }>(CONVERSATION_CHANGED, event => {
          if (event.payload?.id === question.conversationId) check();
        })
        .then(unlisten => {
          if (settled) unlisten();
          else stop = unlisten;
          check();
        })
        .catch(done);
    });
    await this.source.mark(item.id, 'answered');
  }
}

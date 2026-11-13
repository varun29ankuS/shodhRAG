/**
 * Conversations are saved by the app, not the window. The agent session writes each
 * question and answer itself (`agent_send` with its turn); a window writes only the
 * messages it changed (`save_conversation_messages`: a notice, an edited artifact or
 * citation, a side thread, a question and answer that failed before the session took
 * them), never a whole list. Whatever writes a conversation's messages, the backend then
 * sends `conversation_changed { id }`, and every open view reloads that conversation
 * (`load_conversation`), keeping its own changes still waiting to be written.
 *
 * Pure (type-only imports), so it is unit-tested directly with Node
 * (`app/tests/conversationSync.test.ts`).
 */

import type { Conversation, ConversationMessage } from './useConversations';

/** Sent by the backend after the messages of a conversation were written. */
export const CONVERSATION_CHANGED_EVENT = 'conversation_changed';

/** The title of a conversation nobody named yet. */
export const NEW_CHAT_TITLE = 'New Chat';

/** Messages a window changed in one conversation (the `id`), and messages it removed. */
export interface MessagePatch {
  id: string;
  messages: ConversationMessage[];
  removed: string[];
}

/** The conversation id of a `conversation_changed` payload; null when it is malformed. */
export function changedConversationId(payload: unknown): string | null {
  if (typeof payload !== 'object' || payload === null) return null;
  const id = (payload as Record<string, unknown>).id;
  return typeof id === 'string' && id.length > 0 ? id : null;
}

/** A conversation's title from its first question (the backend names it the same way). */
export function autoTitle(firstMessage: string): string {
  const words = firstMessage.trim().split(/\s+/).slice(0, 6);
  let title = words.join(' ');
  if (firstMessage.trim().split(/\s+/).length > 6) title += '...';
  return title || NEW_CHAT_TITLE;
}

/** `messages` with the patch applied: its messages in place by id (or at the end), then its removals. */
export function applyPatch(messages: readonly ConversationMessage[], patch: Pick<MessagePatch, 'messages' | 'removed'>): ConversationMessage[] {
  let next = messages.slice();
  for (const message of patch.messages) {
    const at = next.findIndex(m => m.id === message.id);
    if (at >= 0) next[at] = message;
    else next.push(message);
  }
  if (patch.removed.length > 0) {
    const gone = new Set(patch.removed);
    next = next.filter(m => !gone.has(m.id));
  }
  return next;
}

/**
 * Two patches of one conversation as one, `next` after `waiting`: its messages win by
 * id, a message it writes again is no longer removed, and one it removes is no longer
 * written.
 */
export function mergePatches(waiting: MessagePatch, next: MessagePatch): MessagePatch {
  const gone = new Set(next.removed);
  const written = new Set(next.messages.map(m => m.id));
  const messages = applyPatch(waiting.messages.filter(m => !gone.has(m.id)), { messages: next.messages, removed: [] });
  const removed = [...new Set([...waiting.removed.filter(id => !written.has(id)), ...next.removed])];
  return { id: next.id, messages, removed };
}

/**
 * The conversation with a window's patch applied, as the window shows it at once: named
 * from its first question while it is unnamed, changed now.
 */
export function withPatch(conversation: Conversation, patch: Pick<MessagePatch, 'messages' | 'removed'>, now: string): Conversation {
  const messages = applyPatch(conversation.messages, patch);
  const firstUser = messages.find(m => m.role === 'user');
  return {
    ...conversation,
    messages,
    updatedAt: now,
    ...(conversation.title === NEW_CHAT_TITLE && firstUser ? { title: autoTitle(firstUser.content) } : {}),
  };
}

/**
 * `stored`, with each message equal to the one the window already has kept as that same
 * object: a reload re-renders only what changed, not the whole history.
 */
export function unchangedKept(shown: readonly ConversationMessage[], stored: readonly ConversationMessage[]): ConversationMessage[] {
  const byId = new Map(shown.map(m => [m.id, m]));
  return stored.map(m => {
    const kept = byId.get(m.id);
    return kept && JSON.stringify(kept) === JSON.stringify(m) ? kept : m;
  });
}

/**
 * A conversation reloaded after `conversation_changed`: its messages and time as stored,
 * with the window's own changes still waiting to be written on top, and the name its
 * first question gave it while the window still calls it "New Chat". What the window
 * changes itself (a rename, a pin, its workspace, mode and other settings, whose saves
 * may still be on their way) stays as the window has it. A conversation the window did
 * not have (asked from another device) is taken as stored.
 */
export function reloadedConversation(
  existing: Conversation | undefined,
  stored: Conversation,
  waiting: MessagePatch | undefined,
): Conversation {
  const base: Conversation = existing
    ? {
        ...existing,
        messages: unchangedKept(existing.messages, stored.messages),
        title: existing.title === NEW_CHAT_TITLE ? stored.title : existing.title,
        updatedAt: stored.updatedAt > existing.updatedAt ? stored.updatedAt : existing.updatedAt,
      }
    : stored;
  return waiting ? { ...base, messages: applyPatch(base.messages, waiting) } : base;
}

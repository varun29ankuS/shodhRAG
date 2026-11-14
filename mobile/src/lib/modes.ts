/**
 * A conversation's kind as the phone shows it, from the desktop's saved
 * record (`ConversationRecord` in app/src-tauri/src/conversation_commands.rs):
 * Creator when its agent mode is Creator (the desktop builds in a creation
 * folder whatever the conversation's mode), Code when its mode is Code, and
 * Research otherwise, older records without either field included.
 */

export type ConversationKind = 'research' | 'code' | 'creator';

/** The Conversations filter: every kind, or one. */
export type KindFilter = 'all' | ConversationKind;

export const KIND_FILTERS: readonly KindFilter[] = ['all', 'research', 'code', 'creator'];

export const KIND_LABELS: Record<KindFilter, string> = {
  all: 'All',
  research: 'Research',
  code: 'Code',
  creator: 'Creator',
};

/** The record's fields the kind is read from; either may be absent or null. */
export interface ModeFields {
  mode?: string | null;
  agentMode?: string | null;
}

export function conversationKind(record: ModeFields): ConversationKind {
  if (record.agentMode === 'creator') return 'creator';
  if (record.mode === 'code') return 'code';
  return 'research';
}

export function matchesFilter(record: ModeFields, filter: KindFilter): boolean {
  return filter === 'all' || conversationKind(record) === filter;
}

export function isKindFilter(value: unknown): value is KindFilter {
  return typeof value === 'string' && (KIND_FILTERS as readonly string[]).includes(value);
}

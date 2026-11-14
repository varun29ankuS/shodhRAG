/**
 * What several screens show at once: the conversations (Home, the
 * Conversations list) and the approvals waiting (Approvals, the tab bar's
 * and the menu's count). Read from the computer, and read again when it
 * says they changed.
 */
import { createContext, useContext, useEffect, useMemo, type ReactNode } from 'react';
import { useLink } from './context';
import { CONVERSATION_CHANGED, conversationsList, INBOX_CHANGED, pendingApprovals, type ConversationSummary, type PendingApproval } from './desktop';
import { useRemote, type Remote } from './remote';

interface Data {
  conversations: Remote<ConversationSummary[]>;
  approvals: Remote<PendingApproval[]>;
}

const DataContext = createContext<Data | null>(null);

/** The subscription owner of the approvals listed. */
export const APPROVALS_OWNER = 'approvals';

const CONVERSATION_EVENTS = [CONVERSATION_CHANGED];
const APPROVAL_EVENTS = [INBOX_CHANGED];

export function DataProvider({ children }: { children: ReactNode }) {
  const { subscriptions } = useLink();
  const conversations = useRemote('conversations', conversationsList, CONVERSATION_EVENTS);
  const approvals = useRemote('approvals', pendingApprovals, APPROVAL_EVENTS);

  // The sessions of the approvals listed send their events, so an answered or
  // finished step is seen at once, and stay answerable from this phone.
  const sessions = useMemo(
    () => [...new Set((approvals.data ?? []).map(a => a.sessionId).filter((id): id is string => id !== null))].sort().join('\n'),
    [approvals.data],
  );
  useEffect(() => {
    subscriptions.set(APPROVALS_OWNER, sessions === '' ? [] : sessions.split('\n')).catch(() => undefined);
  }, [sessions, subscriptions]);

  const value = useMemo(() => ({ conversations, approvals }), [conversations, approvals]);
  return <DataContext.Provider value={value}>{children}</DataContext.Provider>;
}

export function useData(): Data {
  const data = useContext(DataContext);
  if (!data) throw new Error('useData outside DataProvider');
  return data;
}

/** Approvals still waiting (an answered one stays listed until the computer says so). */
export function waitingCount(approvals: PendingApproval[] | undefined): number {
  return approvals?.length ?? 0;
}

/** Newest first, pinned first. */
export function sortedConversations(list: ConversationSummary[]): ConversationSummary[] {
  return [...list].sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.updatedAt.localeCompare(a.updatedAt));
}

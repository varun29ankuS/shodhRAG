/** Every conversation on the computer, newest first, with search by title. */
import { useMemo, useState } from 'react';
import { motion } from 'framer-motion';
import { Pin, Search } from 'lucide-react';
import { BackBar } from '../components/Bars';
import { NotConnected } from '../components/Connection';
import { useLink, useNav } from '../lib/context';
import { sortedConversations, useData } from '../lib/data';
import type { ConversationSummary } from '../lib/desktop';
import { pushIn } from '../lib/motion';
import { ago, period, type Period } from '../lib/time';

const PERIODS: readonly Period[] = ['Today', 'Yesterday', 'This week', 'Earlier'];

/** Pinned first, then by when they were last used; only titles that contain every word searched. */
export function groupConversations(list: ConversationSummary[], query: string, now: Date): Array<{ label: string; items: ConversationSummary[] }> {
  const words = query.toLowerCase().split(/\s+/).filter(w => w !== '');
  const matching = sortedConversations(list).filter(c => words.every(w => c.title.toLowerCase().includes(w)));
  const groups: Array<{ label: string; items: ConversationSummary[] }> = [];
  const pinned = matching.filter(c => c.pinned);
  if (pinned.length > 0) groups.push({ label: 'Pinned', items: pinned });
  for (const p of PERIODS) {
    const items = matching.filter(c => !c.pinned && period(c.updatedAt, now) === p);
    if (items.length > 0) groups.push({ label: p, items });
  }
  return groups;
}

export function Conversations({ now = () => new Date() }: { now?: () => Date }) {
  const { connected } = useLink();
  const { dispatch } = useNav();
  const { conversations } = useData();
  const [query, setQuery] = useState('');
  const today = now();
  const groups = useMemo(() => (conversations.data ? groupConversations(conversations.data, query, today) : null), [conversations.data, query, today]);

  return (
    <motion.div className="view" variants={pushIn} initial="hidden" animate="shown" exit="gone">
      <BackBar label="Ask" />
      <div className="scroll">
        <h1 className="title serif pad" style={{ marginTop: 6 }}>
          Conversations
        </h1>
        <div className="search">
          <Search aria-hidden="true" className="icon-small" />
          <label htmlFor="conversation-search" className="visually-hidden">
            Search conversations
          </label>
          <input
            id="conversation-search"
            type="search"
            value={query}
            onChange={event => setQuery(event.target.value)}
            placeholder="Search conversations"
            autoComplete="off"
          />
        </div>
        {groups === null && !connected && <NotConnected what="your conversations" />}
        {groups === null && connected && (
          <div className="skeleton-lines" aria-busy="true" aria-label="Loading your conversations">
            {[88, 72, 80, 64].map(width => (
              <div key={width} className="skeleton" style={{ width: `${width}%`, height: 44, borderRadius: 14 }} />
            ))}
          </div>
        )}
        {groups !== null && conversations.data?.length === 0 && (
          <p className="panel-text pad" style={{ marginTop: 16 }}>
            No conversations yet. Ask a question to start one.
          </p>
        )}
        {groups !== null && groups.length === 0 && (conversations.data?.length ?? 0) > 0 && (
          <p className="panel-text pad" style={{ marginTop: 16 }}>
            {`No conversation title has "${query.trim()}".`}
          </p>
        )}
        {groups?.map(group => (
          <section key={group.label} aria-label={group.label}>
            <div className="label" style={{ paddingTop: 14, paddingBottom: 4 }}>
              {group.label}
            </div>
            <div className="rows">
              {group.items.map(c => (
                <button
                  key={c.id}
                  type="button"
                  className="row"
                  onClick={() => dispatch({ type: 'push', screen: { kind: 'answer', conversationId: c.id, question: null, workspaceId: c.workspaceId } })}
                >
                  <span className="row-text" style={{ gap: 5 }}>
                    <span className="row-title-serif">{c.title}</span>
                    <span className="row-sub">
                      {`${ago(c.updatedAt, today)}, ${c.messageCount} ${c.messageCount === 1 ? 'message' : 'messages'}`}
                    </span>
                  </span>
                  {c.pinned && <Pin aria-label="Pinned" className="icon-small faint" />}
                </button>
              ))}
            </div>
          </section>
        ))}
      </div>
    </motion.div>
  );
}

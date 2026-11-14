/**
 * Every conversation on the computer, newest first: filtered by kind
 * (Research, Code, Creator), grouped by when, and searched by title. Each
 * row shows its kind as an icon, its title, its workspace and its time.
 *
 * Search reads titles only: the computer offers the phone no search of
 * message text (its `search_conversations` is a tool of the assistant, not a
 * command on the phone link's allowlist), and the list carries no messages.
 */
import { useMemo, useState } from 'react';
import { motion } from 'framer-motion';
import { Pin, Search } from 'lucide-react';
import { BackBar } from '../components/Bars';
import { NotConnected } from '../components/Connection';
import { ModeIcon } from '../components/ModeIcon';
import { useLink, useNav } from '../lib/context';
import { sortedConversations, useData } from '../lib/data';
import { workspacesList, type ConversationSummary } from '../lib/desktop';
import { conversationKind, KIND_FILTERS, KIND_LABELS, matchesFilter, type KindFilter } from '../lib/modes';
import { press, pushIn } from '../lib/motion';
import { useRemote } from '../lib/remote';
import { ago, period, type Period } from '../lib/time';

const PERIODS: readonly Period[] = ['Today', 'Yesterday', 'This week', 'Earlier'];

export interface ConversationGroup {
  label: string;
  items: ConversationSummary[];
}

/**
 * Pinned first, then Today, Yesterday, This week and Earlier, newest first in each:
 * only conversations of `filter`'s kind whose title contains every word searched.
 */
export function groupConversations(list: ConversationSummary[], query: string, now: Date, filter: KindFilter = 'all'): ConversationGroup[] {
  const words = query.toLowerCase().split(/\s+/).filter(w => w !== '');
  const matching = sortedConversations(list).filter(c => matchesFilter(c, filter) && words.every(w => c.title.toLowerCase().includes(w)));
  const groups: ConversationGroup[] = [];
  const pinned = matching.filter(c => c.pinned);
  if (pinned.length > 0) groups.push({ label: 'Pinned', items: pinned });
  for (const p of PERIODS) {
    const items = matching.filter(c => !c.pinned && period(c.updatedAt, now) === p);
    if (items.length > 0) groups.push({ label: p, items });
  }
  return groups;
}

/** What an empty list says: nothing at all, nothing of this kind, or no title with the words searched. */
export function emptyWords(total: number, ofKind: number, query: string, filter: KindFilter): string {
  const kind = filter === 'all' ? '' : `${KIND_LABELS[filter]} `;
  if (total === 0) return 'No conversations yet. Ask a question to start one.';
  if (ofKind === 0) return `No ${kind}conversations yet.`;
  return `No ${kind}conversation title has "${query.trim()}".`;
}

export function Conversations({ filter = 'all', now = () => new Date() }: { filter?: KindFilter; now?: () => Date }) {
  const { connected } = useLink();
  const { dispatch } = useNav();
  const { conversations } = useData();
  const workspaces = useRemote('workspaces', workspacesList);
  const [query, setQuery] = useState('');
  const today = now();
  const list = conversations.data;
  const groups = useMemo(() => (list ? groupConversations(list, query, today, filter) : null), [list, query, today, filter]);
  const names = useMemo(() => new Map((workspaces.data ?? []).map(w => [w.id, w.name])), [workspaces.data]);
  const ofKind = list ? list.filter(c => matchesFilter(c, filter)).length : 0;

  // A workspace not read yet (or since deleted) shows no name rather than a wrong one.
  const workspaceName = (id: string | null): string | null => (id === null ? 'No workspace' : (names.get(id) ?? null));

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
            placeholder="Search conversation titles"
            autoComplete="off"
          />
        </div>
        <div className="filters" role="group" aria-label="Show conversations">
          {KIND_FILTERS.map(f => (
            <motion.button
              key={f}
              type="button"
              className="filter"
              aria-pressed={filter === f}
              whileTap={press}
              onClick={() => dispatch({ type: 'filter', filter: f })}
            >
              {f !== 'all' && <ModeIcon kind={f} decorative />}
              {KIND_LABELS[f]}
            </motion.button>
          ))}
        </div>
        {groups === null && !connected && <NotConnected what="your conversations" />}
        {groups === null && connected && (
          <div className="skeleton-lines" aria-busy="true" aria-label="Loading your conversations">
            {[88, 72, 80, 64].map(width => (
              <div key={width} className="skeleton" style={{ width: `${width}%`, height: 44, borderRadius: 14 }} />
            ))}
          </div>
        )}
        {groups !== null && groups.length === 0 && (
          <p className="panel-text pad" style={{ marginTop: 16 }}>
            {emptyWords(list?.length ?? 0, ofKind, query, filter)}
          </p>
        )}
        {groups?.map(group => (
          <section key={group.label} aria-label={group.label}>
            <div className="label" style={{ paddingTop: 14, paddingBottom: 4 }}>
              {group.label}
            </div>
            <div className="rows">
              {group.items.map(c => {
                const where = workspaceName(c.workspaceId);
                const when = ago(c.updatedAt, today);
                return (
                  <button
                    key={c.id}
                    type="button"
                    className="row"
                    onClick={() => dispatch({ type: 'push', screen: { kind: 'answer', conversationId: c.id, question: null, workspaceId: c.workspaceId } })}
                  >
                    <ModeIcon kind={conversationKind(c)} />
                    <span className="row-text" style={{ gap: 5 }}>
                      <span className="row-title-serif">{c.title}</span>
                      <span className="row-sub">{where === null ? when : `${where} · ${when}`}</span>
                    </span>
                    {c.pinned && <Pin aria-label="Pinned" className="icon-small faint" />}
                  </button>
                );
              })}
            </div>
          </section>
        ))}
      </div>
    </motion.div>
  );
}

import React, { useId, useState } from 'react';
import { ChevronDown } from 'lucide-react';
import { cn } from '../../lib/utils';
import type { PlanItem, PlanStatus } from './events';
import { planProgress } from './reducer';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

const GLYPH: Record<PlanStatus, string> = {
  pending: '☐',
  in_progress: '◐',
  done: '☑',
};

const STATUS_TEXT: Record<PlanStatus, string> = {
  pending: 'to do',
  in_progress: 'in progress',
  done: 'done',
};

interface PlanPanelProps {
  items: readonly PlanItem[];
  /** The plan belongs to a running answer. */
  live: boolean;
  /** `docked`: right-hand column on wide screens; `inline`: in the flow. */
  variant: 'docked' | 'inline';
}

/** The agent's task list: ☐ to do, ◐ in progress, ☑ done. */
export function PlanPanel({ items, live, variant }: PlanPanelProps) {
  const [collapsed, setCollapsed] = useState(false);
  const listId = useId();
  if (items.length === 0) return null;
  const { done, total } = planProgress(items);

  return (
    <section
      aria-label="Task list"
      className={cn(
        'ask-fade-in rounded-xl border border-shodh-border bg-shodh-surface text-[12.5px]',
        variant === 'docked' ? 'w-[260px]' : 'w-full',
      )}
    >
      <button
        type="button"
        onClick={() => setCollapsed(c => !c)}
        aria-expanded={!collapsed}
        aria-controls={listId}
        className={cn('w-full flex items-center gap-2 px-3 h-9 rounded-xl text-left', FOCUS_RING)}
      >
        <span className="font-semibold text-shodh-text">Tasks</span>
        <span className="text-shodh-text-muted tabular-nums" aria-live={live ? 'polite' : 'off'}>
          {`${done} of ${total} done`}
        </span>
        <ChevronDown
          className={cn('ml-auto w-3.5 h-3.5 text-shodh-text-faint transition-transform duration-micro', collapsed && '-rotate-90')}
          aria-hidden="true"
        />
      </button>
      {!collapsed && (
        <ol id={listId} className="list-none m-0 px-3 pb-2.5 flex flex-col gap-1">
          {items.map(item => (
            <li key={item.id} className="flex items-start gap-2 min-w-0">
              <span
                aria-hidden="true"
                className={cn(
                  'font-mono leading-[1.45] shrink-0',
                  item.status === 'in_progress' ? 'text-shodh-accent-text' : item.status === 'done' ? 'text-shodh-success' : 'text-shodh-text-faint',
                )}
              >
                {GLYPH[item.status]}
              </span>
              <span
                className={cn(
                  'min-w-0 break-words leading-[1.45]',
                  item.status === 'done' && 'text-shodh-text-muted line-through decoration-shodh-text-faint',
                  item.status === 'in_progress' && 'text-shodh-text font-medium',
                  item.status === 'pending' && 'text-shodh-text-secondary',
                )}
              >
                {item.text}
                <span className="sr-only">{` (${STATUS_TEXT[item.status]})`}</span>
              </span>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

export default PlanPanel;

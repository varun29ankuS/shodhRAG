import React, { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Bell, Check, ExternalLink, Inbox as InboxIcon, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { removeWithUndo, undoLast } from '../../lib/undoToast';
import { relativeTime } from '../../utils/time';
import { acceptSuggestion, errorText, rejectSuggestion } from '../memory/api';
import { parseTarget, publishTarget } from '../agent/navigation';
import { announceApproval } from './approvalBus';
import { normalizeViewTab } from '../../lib/viewTabs';
import type { ViewTab } from '../../lib/viewTabs';
import {
  STATUS_LABEL,
  STATUS_TONE,
  actionsFor,
  inboxCommand,
  keepSelection,
  moveSelection,
  parseItems,
  waitingCount,
} from './model';
import type { InboxItem } from './model';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';
const ACTION =
  'h-7 px-2 inline-flex items-center gap-1 rounded-md text-[12px] font-medium transition-colors duration-micro disabled:opacity-50';

/** Opened from the command palette ("Inbox"). */
export const OPEN_INBOX_EVENT = 'shodh:open-inbox';

interface InboxButtonProps {
  onNavigate: (view: ViewTab) => void;
  onOpenConversation: (id: string) => void;
}

function str(value: unknown): string | null {
  return typeof value === 'string' && value.length > 0 ? value : null;
}

/**
 * The bell in the header and its Inbox: everything waiting on the user
 * (approvals, memory suggestions, reminders) and background work that finished
 * or failed. J/K move, Enter opens, A approves or accepts, D denies or
 * dismisses, U undoes the last dismissal.
 */
export function InboxButton({ onNavigate, onOpenConversation }: InboxButtonProps) {
  const [open, setOpen] = useState(false);
  const [items, setItems] = useState<InboxItem[]>([]);
  const [hidden, setHidden] = useState<ReadonlySet<string>>(() => new Set());
  const [selected, setSelected] = useState(0);
  const [busy, setBusy] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const rowRefs = useRef(new Map<string, HTMLButtonElement>());
  const panelId = useId();
  const headingId = useId();

  const load = useCallback(async () => {
    try {
      setItems(parseItems(await invoke<unknown>('inbox_list')));
      setLoadError(null);
    } catch (err) {
      setLoadError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
    const unlisten = [listen('inbox-changed', () => void load()), listen('memory-suggestions-changed', () => void load())];
    const openFromPalette = () => setOpen(true);
    window.addEventListener(OPEN_INBOX_EVENT, openFromPalette);
    return () => {
      window.removeEventListener(OPEN_INBOX_EVENT, openFromPalette);
      for (const off of unlisten) void off.then(fn => fn());
    };
  }, [load]);

  const visible = useMemo(() => items.filter(i => !hidden.has(i.id)), [items, hidden]);
  const waiting = waitingCount(visible);
  const current = visible[selected] ?? null;

  // The selection follows its item when the list changes.
  const selectedIdRef = useRef<string | null>(null);
  useEffect(() => {
    setSelected(index => keepSelection(selectedIdRef.current, index, visible));
  }, [visible]);
  useEffect(() => {
    selectedIdRef.current = current?.id ?? null;
  }, [current]);

  // Close on a click outside; give focus back to the bell on Escape.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  useEffect(() => {
    if (open && current) rowRefs.current.get(current.id)?.focus();
    // Focus moves only when the panel opens or the selection changes.
  }, [open, current?.id]);

  const hide = (id: string, on: boolean) =>
    setHidden(prev => {
      const next = new Set(prev);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });

  const openItem = (item: InboxItem) => {
    const link = item.link;
    if (!link) return;
    const tab = normalizeViewTab(link.view);
    const target = parseTarget(link.target);
    setOpen(false);
    if (target?.kind === 'conversation') {
      onOpenConversation(target.conversationId);
      return;
    }
    if (target) publishTarget(target);
    if (tab) onNavigate(tab);
  };

  const run = async (item: InboxItem, action: () => Promise<unknown>, failure: string) => {
    setBusy(item.id);
    try {
      await action();
      await load();
    } catch (err) {
      notify.error(failure, { description: errorText(err) });
    } finally {
      setBusy(null);
    }
  };

  // Answer an approval, and let the chat that shows the step know.
  const decide = (item: InboxItem, approved: boolean) => {
    const sessionId = str(item.data.sessionId);
    const stepId = str(item.data.stepId);
    if (!sessionId || !stepId) return;
    void run(item, async () => {
      await invoke('agent_approve', { sessionId, stepId, approved });
      announceApproval({ sessionId, stepId, approved });
    }, approved ? 'The step was not approved' : 'The step was not denied');
  };

  const primary = (item: InboxItem) => {
    const { primary: action } = actionsFor(item);
    if (action === 'approve') {
      decide(item, true);
    } else if (action === 'accept') {
      const id = str(item.data.suggestionId);
      if (id) void run(item, () => acceptSuggestion(id), 'The suggestion was not accepted');
    }
  };

  const secondary = (item: InboxItem) => {
    const { secondary: action } = actionsFor(item);
    if (action === 'deny') {
      decide(item, false);
      return;
    }
    if (action !== 'dismiss') return;
    const suggestionId = item.kind === 'memory' ? str(item.data.suggestionId) : null;
    removeWithUndo({
      message: item.kind === 'memory' ? 'Suggestion dismissed' : 'Removed from the Inbox',
      description: item.title,
      hide: () => hide(item.id, true),
      restore: () => hide(item.id, false),
      commit: async () => {
        if (suggestionId) await rejectSuggestion(suggestionId);
        else await invoke('inbox_dismiss', { id: item.id });
        await load();
        hide(item.id, false);
      },
      onError: err => notify.error('It was not dismissed', { description: errorText(err) }),
    });
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      setOpen(false);
      buttonRef.current?.focus();
      return;
    }
    const target = e.target as HTMLElement;
    const command = inboxCommand(e, target.tagName === 'INPUT' || target.tagName === 'TEXTAREA');
    if (!command) return;
    // Enter on an action button activates that button, not the row.
    if (command === 'open' && target.dataset.inboxRow === undefined) return;
    e.preventDefault();
    if (command === 'next' || command === 'previous') {
      setSelected(i => moveSelection(i, visible.length, command === 'next' ? 1 : -1));
    } else if (command === 'undo') {
      if (!undoLast()) notify.info('Nothing to undo');
    } else if (current && busy === null) {
      if (command === 'open') openItem(current);
      else if (command === 'primary') primary(current);
      else secondary(current);
    }
  };

  return (
    <div className="relative" ref={rootRef}>
      <button
        ref={buttonRef}
        type="button"
        onClick={() => setOpen(o => !o)}
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        aria-label={waiting > 0 ? `Inbox, ${waiting} waiting for you` : 'Inbox'}
        title="Inbox"
        className={cn(
          'relative w-7 h-7 rounded-md inline-flex items-center justify-center text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
          FOCUS_RING,
        )}
      >
        <Bell className="w-4 h-4" aria-hidden="true" />
        {waiting > 0 && (
          <span
            aria-hidden="true"
            className="absolute -top-0.5 -right-0.5 min-w-[14px] h-[14px] px-0.5 rounded-full bg-shodh-warning-soft text-shodh-text ring-1 ring-shodh-warning text-[9px] font-bold leading-[14px] text-center"
          >
            {waiting > 9 ? '9+' : waiting}
          </span>
        )}
      </button>

      {open && (
        <section
          id={panelId}
          aria-labelledby={headingId}
          onKeyDown={onKeyDown}
          className="absolute right-0 top-9 z-50 w-[380px] max-w-[calc(100vw-24px)] rounded-xl border border-shodh-border bg-shodh-surface shadow-lg overflow-hidden"
        >
          <header className="flex items-baseline justify-between gap-2 px-3.5 py-2.5 border-b border-shodh-border-subtle">
            <h2 id={headingId} className="m-0 text-[13px] font-semibold text-shodh-text">Inbox</h2>
            <p className="m-0 text-[11px] text-shodh-text-muted">
              <kbd>J</kbd>/<kbd>K</kbd> move · <kbd>A</kbd> approve · <kbd>D</kbd> deny or dismiss · <kbd>U</kbd> undo
            </p>
          </header>
          {loadError ? (
            <p role="alert" className="m-0 px-3.5 py-6 text-[12.5px] text-shodh-error">{loadError}</p>
          ) : visible.length === 0 ? (
            <div className="px-3.5 py-8 flex flex-col items-center gap-1.5 text-center">
              <InboxIcon className="w-5 h-5 text-shodh-text-faint" aria-hidden="true" />
              <p className="m-0 text-[12.5px] text-shodh-text-secondary">Nothing is waiting for you.</p>
              <p className="m-0 text-[11.5px] text-shodh-text-muted">Approvals, suggestions and finished work show up here.</p>
            </div>
          ) : (
            <ul className="m-0 p-1 list-none max-h-[420px] overflow-y-auto" aria-label="Inbox items">
              {visible.map((item, index) => {
                const tone = STATUS_TONE[item.status];
                const actions = actionsFor(item);
                const isSelected = index === selected;
                const disabled = busy === item.id;
                return (
                  <li
                    key={item.id}
                    className={cn('rounded-lg px-2.5 py-2 flex flex-col gap-1.5', isSelected && 'bg-shodh-raised')}
                  >
                    <button
                      ref={el => {
                        if (el) rowRefs.current.set(item.id, el);
                        else rowRefs.current.delete(item.id);
                      }}
                      type="button"
                      data-inbox-row=""
                      tabIndex={isSelected ? 0 : -1}
                      aria-current={isSelected ? 'true' : undefined}
                      onFocus={() => setSelected(index)}
                      onClick={() => openItem(item)}
                      className={cn('text-left flex items-start gap-2 rounded-md min-w-0', FOCUS_RING)}
                    >
                      <span className={cn('mt-[5px] w-2 h-2 rounded-full shrink-0', tone.dot)} aria-hidden="true" />
                      <span className="min-w-0 flex-1 flex flex-col gap-0.5">
                        <span className="text-[12.5px] font-medium text-shodh-text break-words">{item.title}</span>
                        <span className="text-[11.5px] text-shodh-text-muted break-words">
                          <span className={tone.text}>{STATUS_LABEL[item.status]}</span>
                          {item.detail ? ` · ${item.detail}` : ''}
                          {item.updatedAt ? ` · ${relativeTime(item.updatedAt)}` : ''}
                        </span>
                      </span>
                    </button>
                    {(actions.primary || actions.secondary || item.link) && (
                      <div className="flex items-center gap-1.5 pl-4">
                        {actions.primary && (
                          <button
                            type="button"
                            tabIndex={isSelected ? 0 : -1}
                            disabled={disabled}
                            onClick={() => primary(item)}
                            className={cn(ACTION, 'bg-shodh-accent text-shodh-on-accent hover:bg-shodh-accent-hover', FOCUS_RING)}
                          >
                            <Check className="w-3.5 h-3.5" aria-hidden="true" />
                            {actions.primary === 'approve' ? 'Approve' : 'Accept'}
                          </button>
                        )}
                        {actions.secondary && (
                          <button
                            type="button"
                            tabIndex={isSelected ? 0 : -1}
                            disabled={disabled}
                            onClick={() => secondary(item)}
                            className={cn(ACTION, 'border border-shodh-border text-shodh-text-secondary hover:bg-shodh-raised-2 hover:text-shodh-text', FOCUS_RING)}
                          >
                            <X className="w-3.5 h-3.5" aria-hidden="true" />
                            {actions.secondary === 'deny' ? 'Deny' : 'Dismiss'}
                          </button>
                        )}
                        {item.link && (
                          <button
                            type="button"
                            tabIndex={isSelected ? 0 : -1}
                            onClick={() => openItem(item)}
                            className={cn(ACTION, 'text-shodh-text-secondary hover:bg-shodh-raised-2 hover:text-shodh-text', FOCUS_RING)}
                          >
                            <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" />
                            Open
                          </button>
                        )}
                      </div>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </section>
      )}
    </div>
  );
}

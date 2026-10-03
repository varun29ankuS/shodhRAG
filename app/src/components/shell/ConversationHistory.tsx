import React, { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { MoreHorizontal, Pencil, Pin, PinOff, Trash2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { groupConversations } from '../../lib/conversationGroups';
import { relativeTime } from '../../utils/time';
import type { Conversation } from '../../hooks/useConversations';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-sidebar';

/** Re-group at least this often so "Today" rolls over at midnight. */
const REGROUP_INTERVAL_MS = 60_000;

interface ConversationHistoryProps {
  conversations: Conversation[];
  activeConversationId: string | null;
  /** The Ask view is showing, so the active conversation is the current page. */
  askActive: boolean;
  onOpen: (id: string) => void;
  onRename: (id: string, title: string) => void;
  onPin: (id: string) => void;
  onDelete: (id: string) => void;
}

function useNow(intervalMs: number): Date {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), intervalMs);
    return () => window.clearInterval(timer);
  }, [intervalMs]);
  return now;
}

/**
 * Conversation history under Ask: Pinned, Today, Yesterday, Previous 7 days,
 * Older. Arrow keys move between conversations, Home/End jump to the ends.
 */
export function ConversationHistory({
  conversations,
  activeConversationId,
  askActive,
  onOpen,
  onRename,
  onPin,
  onDelete,
}: ConversationHistoryProps) {
  const now = useNow(REGROUP_INTERVAL_MS);
  const groups = useMemo(() => groupConversations(conversations, now), [conversations, now]);
  const listRef = useRef<HTMLDivElement>(null);
  const headingId = useId();

  const handleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return;
    const target = e.target as HTMLElement;
    if (!target.matches('[data-history-item]')) return;
    const items = Array.from(listRef.current?.querySelectorAll<HTMLButtonElement>('[data-history-item]') ?? []);
    const index = items.indexOf(target as HTMLButtonElement);
    if (index < 0 || items.length === 0) return;
    e.preventDefault();
    const next =
      e.key === 'Home' ? 0
      : e.key === 'End' ? items.length - 1
      : e.key === 'ArrowDown' ? Math.min(items.length - 1, index + 1)
      : Math.max(0, index - 1);
    items[next]?.focus();
  };

  return (
    <section aria-labelledby={headingId} className="flex-1 min-h-0 flex flex-col">
      <h2
        id={headingId}
        className="px-5 pt-4 pb-1 text-[11px] font-semibold tracking-[0.08em] uppercase text-shodh-text-faint shrink-0"
      >
        Chats
      </h2>
      {groups.length === 0 ? (
        // The list always holds at least one conversation once loaded, so empty means loading.
        <div className="px-5 py-2 flex flex-col gap-2.5" role="status" aria-label="Loading conversations">
          {[72, 54, 64].map(w => (
            <span key={w} className="shell-skeleton h-3.5 rounded" style={{ width: `${w}%` }} aria-hidden="true" />
          ))}
        </div>
      ) : (
        <div
          ref={listRef}
          onKeyDown={handleKeyDown}
          className="flex-1 min-h-0 overflow-y-auto scrollbar-thin px-2 pb-2"
        >
          {groups.map(group => (
            <div key={group.id} role="group" aria-labelledby={`${headingId}-${group.id}`} className="pt-2 first:pt-0">
              <h3
                id={`${headingId}-${group.id}`}
                className="px-3 pt-1 pb-1 text-[11px] font-medium text-shodh-text-faint"
              >
                {group.label}
              </h3>
              <ul className="flex flex-col gap-px">
                {group.items.map(conv => (
                  <ConversationRow
                    key={conv.id}
                    conversation={conv}
                    isActive={conv.id === activeConversationId}
                    isCurrentPage={askActive && conv.id === activeConversationId}
                    onOpen={onOpen}
                    onRename={onRename}
                    onPin={onPin}
                    onDelete={onDelete}
                  />
                ))}
              </ul>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

interface ConversationRowProps {
  conversation: Conversation;
  isActive: boolean;
  isCurrentPage: boolean;
  onOpen: (id: string) => void;
  onRename: (id: string, title: string) => void;
  onPin: (id: string) => void;
  onDelete: (id: string) => void;
}

function ConversationRow({
  conversation,
  isActive,
  isCurrentPage,
  onOpen,
  onRename,
  onPin,
  onDelete,
}: ConversationRowProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [draftTitle, setDraftTitle] = useState(conversation.title);
  const rowRef = useRef<HTMLLIElement>(null);
  const openButtonRef = useRef<HTMLButtonElement>(null);
  const menuButtonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const menuId = useId();

  const closeMenu = useCallback((restoreFocus: boolean) => {
    setMenuOpen(false);
    if (restoreFocus) menuButtonRef.current?.focus();
  }, []);

  // Close the menu on outside pointer-down.
  useEffect(() => {
    if (!menuOpen) return;
    const handlePointerDown = (e: PointerEvent) => {
      if (rowRef.current && !rowRef.current.contains(e.target as Node)) closeMenu(false);
    };
    document.addEventListener('pointerdown', handlePointerDown);
    return () => document.removeEventListener('pointerdown', handlePointerDown);
  }, [menuOpen, closeMenu]);

  // Move focus into the menu when it opens.
  useEffect(() => {
    if (menuOpen) menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
  }, [menuOpen]);

  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  const startRename = () => {
    setDraftTitle(conversation.title);
    setMenuOpen(false);
    setEditing(true);
  };

  const finishEditing = () => {
    setEditing(false);
    // Return focus to the row the user was renaming.
    requestAnimationFrame(() => openButtonRef.current?.focus());
  };

  const commitRename = () => {
    if (!editing) return;
    const title = draftTitle.trim();
    if (title && title !== conversation.title) onRename(conversation.id, title);
    finishEditing();
  };

  const cancelRename = () => {
    setDraftTitle(conversation.title);
    finishEditing();
  };

  const handleMenuKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? []);
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      closeMenu(true);
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      e.stopPropagation();
      items[(index + 1) % items.length]?.focus();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      e.stopPropagation();
      items[(index - 1 + items.length) % items.length]?.focus();
    } else if (e.key === 'Tab') {
      closeMenu(false);
    }
  };

  const meta = [conversation.spaceName, relativeTime(conversation.updatedAt)].filter(Boolean).join(' · ');

  return (
    <li ref={rowRef} className="relative group">
      {editing ? (
        <div className="px-3 py-2 rounded-lg bg-shodh-raised">
          <label className="sr-only" htmlFor={`${menuId}-rename`}>Conversation title</label>
          <input
            id={`${menuId}-rename`}
            ref={inputRef}
            type="text"
            value={draftTitle}
            onChange={e => setDraftTitle(e.target.value)}
            onKeyDown={e => {
              if (e.key === 'Enter') {
                e.preventDefault();
                commitRename();
              } else if (e.key === 'Escape') {
                e.preventDefault();
                cancelRename();
              }
            }}
            onBlur={commitRename}
            className="w-full bg-transparent text-[13px] text-shodh-text border-b border-shodh-accent-text outline-none py-0.5"
          />
        </div>
      ) : (
        <button
          ref={openButtonRef}
          type="button"
          data-history-item=""
          onClick={() => onOpen(conversation.id)}
          onKeyDown={e => {
            if (e.key === 'F2') {
              e.preventDefault();
              startRename();
            }
          }}
          aria-current={isCurrentPage ? 'page' : undefined}
          aria-label={meta ? `${conversation.title}, ${meta}` : conversation.title}
          className={cn(
            'w-full flex flex-col gap-0.5 py-1.5 pl-3 pr-9 rounded-lg text-left transition-colors duration-micro',
            isActive ? 'bg-shodh-raised text-shodh-text' : 'text-shodh-text-secondary hover:bg-shodh-raised/60 hover:text-shodh-text',
            FOCUS_RING
          )}
        >
          <span className="flex items-center gap-1.5 min-w-0">
            {conversation.pinned && <Pin className="w-3 h-3 shrink-0 text-shodh-text-faint" aria-hidden="true" />}
            <span className="text-[13px] truncate">{conversation.title}</span>
          </span>
          {meta && <span className="text-[11px] text-shodh-text-faint truncate" aria-hidden="true">{meta}</span>}
        </button>
      )}

      {!editing && (
        <button
          ref={menuButtonRef}
          type="button"
          onClick={() => setMenuOpen(open => !open)}
          aria-label={`Options for ${conversation.title}`}
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          aria-controls={menuOpen ? menuId : undefined}
          className={cn(
            'absolute right-1.5 top-1 w-7 h-7 rounded-md inline-flex items-center justify-center text-shodh-text-muted hover:bg-shodh-raised-2 hover:text-shodh-text transition-opacity duration-micro',
            menuOpen ? 'opacity-100' : 'opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100',
            FOCUS_RING
          )}
        >
          <MoreHorizontal className="w-4 h-4" aria-hidden="true" />
        </button>
      )}

      {menuOpen && (
        <div
          id={menuId}
          ref={menuRef}
          role="menu"
          aria-label={`Options for ${conversation.title}`}
          onKeyDown={handleMenuKeyDown}
          className="shell-pop absolute right-1.5 top-8 z-50 min-w-[152px] py-1 rounded-lg border border-shodh-border-strong bg-shodh-raised shadow-lg"
        >
          <MenuItem onSelect={startRename} icon={<Pencil className="w-3.5 h-3.5" aria-hidden="true" />} hint="F2">
            Rename
          </MenuItem>
          <MenuItem
            onSelect={() => {
              onPin(conversation.id);
              closeMenu(true);
            }}
            icon={conversation.pinned ? <PinOff className="w-3.5 h-3.5" aria-hidden="true" /> : <Pin className="w-3.5 h-3.5" aria-hidden="true" />}
          >
            {conversation.pinned ? 'Unpin' : 'Pin'}
          </MenuItem>
          <div role="separator" className="my-1 border-t border-shodh-border" />
          <MenuItem
            destructive
            onSelect={() => {
              setMenuOpen(false);
              onDelete(conversation.id);
            }}
            icon={<Trash2 className="w-3.5 h-3.5" aria-hidden="true" />}
          >
            Delete
          </MenuItem>
        </div>
      )}
    </li>
  );
}

function MenuItem({
  onSelect,
  icon,
  hint,
  destructive = false,
  children,
}: {
  onSelect: () => void;
  icon: React.ReactNode;
  hint?: string;
  destructive?: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      role="menuitem"
      onClick={onSelect}
      className={cn(
        'w-full flex items-center gap-2 px-3 py-1.5 text-[12.5px] text-left hover:bg-shodh-raised-2 focus-visible:bg-shodh-raised-2 focus-visible:outline-none',
        destructive ? 'text-shodh-error' : 'text-shodh-text-secondary'
      )}
    >
      {icon}
      <span className="flex-1">{children}</span>
      {hint && <kbd className="font-mono text-[10.5px] text-shodh-text-faint">{hint}</kbd>}
    </button>
  );
}

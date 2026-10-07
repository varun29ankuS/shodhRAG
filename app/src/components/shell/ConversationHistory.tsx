import React, { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { ChevronRight, FolderInput, MoreHorizontal, Pencil, Pin, PinOff, Plus, Trash2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { groupConversations } from '../../lib/conversationGroups';
import { relativeTime } from '../../utils/time';
import type { Conversation } from '../../hooks/useConversations';
import { groupChatsByWorkspace } from '../../features/workspaces/model';
import type { ChatGroup, GroupableWorkspace } from '../../features/workspaces/model';
import { WorkspaceIcon } from '../../features/workspaces/WorkspaceIcon';
import { ConversationPreviewCard, usePreviewVisibility } from './ConversationPreviewCard';

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
  /** Every workspace (chats are grouped under them; archived ones' chats are hidden). */
  workspaces: GroupableWorkspace[];
  onOpenWorkspace: (id: string) => void;
  onNewChatInWorkspace: (id: string) => void;
  /** Move a chat to a workspace, or out of one (null). */
  onMoveToWorkspace: (conversationId: string, workspaceId: string | null) => void;
}

/** Collapsed workspace groups (per viewer; the sidebar works without it). */
const COLLAPSED_KEY = 'shodh.sidebar.collapsedWorkspaces';

function readCollapsed(): Set<string> {
  try {
    const raw = window.localStorage.getItem(COLLAPSED_KEY);
    const list: unknown = raw ? JSON.parse(raw) : [];
    return new Set(Array.isArray(list) ? list.filter((x): x is string => typeof x === 'string') : []);
  } catch {
    return new Set();
  }
}

function writeCollapsed(ids: Set<string>) {
  try {
    window.localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...ids]));
  } catch {
    // Not remembered (private window or blocked storage); the toggle still works.
  }
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
 * Conversation history under Ask, grouped by workspace (each collapsible, with a "new
 * chat" button), then the chats in no workspace by date: Pinned, Today, Yesterday,
 * Previous 7 days, Older. Arrow keys move between conversations, Home/End jump to the
 * ends.
 */
export function ConversationHistory({
  conversations,
  activeConversationId,
  askActive,
  onOpen,
  onRename,
  onPin,
  onDelete,
  workspaces,
  onOpenWorkspace,
  onNewChatInWorkspace,
  onMoveToWorkspace,
}: ConversationHistoryProps) {
  const now = useNow(REGROUP_INTERVAL_MS);
  const workspaceGroups = useMemo(() => groupChatsByWorkspace(conversations, workspaces), [conversations, workspaces]);
  const looseChats = useMemo(
    () => workspaceGroups.find(g => g.workspace === null)?.items ?? [],
    [workspaceGroups],
  );
  const groups = useMemo(() => groupConversations(looseChats, now), [looseChats, now]);
  const named = workspaceGroups.filter((g): g is ChatGroup<Conversation> & { workspace: GroupableWorkspace } => g.workspace !== null);
  const [collapsed, setCollapsed] = useState<Set<string>>(readCollapsed);
  const toggle = useCallback((id: string) => {
    setCollapsed(prev => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      writeCollapsed(next);
      return next;
    });
  }, []);
  const listRef = useRef<HTMLDivElement>(null);
  const headingId = useId();
  const rowProps = { activeConversationId, askActive, onOpen, onRename, onPin, onDelete, workspaces, onMoveToWorkspace };

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
      {groups.length === 0 && named.length === 0 ? (
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
          {named.map(group => {
            const id = group.workspace.id;
            const open = !collapsed.has(id);
            const listId = `${headingId}-ws-${id}`;
            return (
              <div key={id} role="group" aria-label={`Workspace ${group.workspace.name}`} className="pt-2 first:pt-0">
                <div className="group/ws flex items-center gap-0.5 pr-1">
                  <button
                    type="button"
                    onClick={() => toggle(id)}
                    aria-expanded={open}
                    aria-controls={listId}
                    aria-label={`${group.workspace.name}, ${group.items.length} ${group.items.length === 1 ? 'chat' : 'chats'}`}
                    className={cn(
                      'flex-1 min-w-0 flex items-center gap-1.5 h-7 pl-1.5 pr-2 rounded-md text-left text-[12px] font-medium text-shodh-text-secondary hover:bg-shodh-raised/60 hover:text-shodh-text transition-colors duration-micro',
                      FOCUS_RING,
                    )}
                  >
                    <ChevronRight
                      className={cn('w-3.5 h-3.5 shrink-0 text-shodh-text-faint transition-transform duration-micro motion-reduce:transition-none', open && 'rotate-90')}
                      aria-hidden="true"
                    />
                    <WorkspaceIcon icon={group.workspace.icon} color={group.workspace.color} className="w-3.5 h-3.5" />
                    <span className="truncate">{group.workspace.name}</span>
                    <span className="ml-auto text-[11px] font-normal text-shodh-text-faint" aria-hidden="true">{group.items.length}</span>
                  </button>
                  <button
                    type="button"
                    onClick={() => onOpenWorkspace(id)}
                    aria-label={`Open workspace ${group.workspace.name}`}
                    title="Open workspace"
                    className={cn(
                      'w-6 h-6 rounded-md inline-flex items-center justify-center text-shodh-text-muted hover:bg-shodh-raised-2 hover:text-shodh-text opacity-0 group-hover/ws:opacity-100 group-focus-within/ws:opacity-100 focus-visible:opacity-100 transition-opacity duration-micro',
                      FOCUS_RING,
                    )}
                  >
                    <FolderInput className="w-3.5 h-3.5" aria-hidden="true" />
                  </button>
                  <button
                    type="button"
                    onClick={() => onNewChatInWorkspace(id)}
                    aria-label={`New chat in ${group.workspace.name}`}
                    title="New chat in this workspace"
                    className={cn(
                      'w-6 h-6 rounded-md inline-flex items-center justify-center text-shodh-text-muted hover:bg-shodh-raised-2 hover:text-shodh-text opacity-0 group-hover/ws:opacity-100 group-focus-within/ws:opacity-100 focus-visible:opacity-100 transition-opacity duration-micro',
                      FOCUS_RING,
                    )}
                  >
                    <Plus className="w-3.5 h-3.5" aria-hidden="true" />
                  </button>
                </div>
                {open && (
                  <ul id={listId} className="flex flex-col gap-px pl-2">
                    {group.items.map(conv => (
                      <ConversationRow key={conv.id} conversation={conv} {...rowProps} />
                    ))}
                  </ul>
                )}
              </div>
            );
          })}
          {named.length > 0 && groups.length > 0 && (
            <h3 className="px-3 pt-3 pb-0.5 text-[11px] font-semibold tracking-[0.06em] uppercase text-shodh-text-faint">
              No workspace
            </h3>
          )}
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
                  <ConversationRow key={conv.id} conversation={conv} {...rowProps} />
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
  activeConversationId: string | null;
  askActive: boolean;
  onOpen: (id: string) => void;
  onRename: (id: string, title: string) => void;
  onPin: (id: string) => void;
  onDelete: (id: string) => void;
  workspaces: GroupableWorkspace[];
  onMoveToWorkspace: (conversationId: string, workspaceId: string | null) => void;
}

function ConversationRow({
  conversation,
  activeConversationId,
  askActive,
  onOpen,
  onRename,
  onPin,
  onDelete,
  workspaces,
  onMoveToWorkspace,
}: ConversationRowProps) {
  const isActive = conversation.id === activeConversationId;
  const isCurrentPage = askActive && isActive;
  const [menuOpen, setMenuOpen] = useState(false);
  // The menu shows the workspaces to move the chat to instead of its actions.
  const [moving, setMoving] = useState(false);
  const [editing, setEditing] = useState(false);
  const [draftTitle, setDraftTitle] = useState(conversation.title);
  const rowRef = useRef<HTMLLIElement>(null);
  const openButtonRef = useRef<HTMLButtonElement>(null);
  const menuButtonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const menuId = useId();
  const previewId = `${menuId}-preview`;
  const preview = usePreviewVisibility();
  const showPreview = !editing && !menuOpen && preview.open && openButtonRef.current !== null;

  const closeMenu = useCallback((restoreFocus: boolean) => {
    setMenuOpen(false);
    setMoving(false);
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
  }, [menuOpen, moving]);

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

  const meta = relativeTime(conversation.updatedAt);
  const currentWorkspace = conversation.workspaceId ?? null;

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
          onClick={() => { preview.hide(); onOpen(conversation.id); }}
          onMouseEnter={preview.show}
          onMouseLeave={preview.hide}
          onFocus={preview.show}
          onBlur={preview.hide}
          aria-describedby={showPreview ? previewId : undefined}
          onKeyDown={e => {
            if (e.key === 'F2') {
              e.preventDefault();
              preview.hide();
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

      {showPreview && openButtonRef.current && (
        <ConversationPreviewCard id={previewId} conversation={conversation} anchor={openButtonRef.current} />
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
          {moving ? (
            <>
              {workspaces.filter(w => !w.archived && w.id !== currentWorkspace).map(w => (
                <MenuItem
                  key={w.id}
                  onSelect={() => {
                    onMoveToWorkspace(conversation.id, w.id);
                    closeMenu(true);
                  }}
                  icon={<WorkspaceIcon icon={w.icon} color={w.color} className="w-3.5 h-3.5" />}
                >
                  {w.name}
                </MenuItem>
              ))}
              {currentWorkspace !== null && (
                <MenuItem
                  onSelect={() => {
                    onMoveToWorkspace(conversation.id, null);
                    closeMenu(true);
                  }}
                  icon={<FolderInput className="w-3.5 h-3.5" aria-hidden="true" />}
                >
                  No workspace
                </MenuItem>
              )}
            </>
          ) : (
            <>
          <MenuItem onSelect={startRename} icon={<Pencil className="w-3.5 h-3.5" aria-hidden="true" />} hint="F2">
            Rename
          </MenuItem>
          {(workspaces.some(w => !w.archived) || currentWorkspace !== null) && (
            <MenuItem onSelect={() => setMoving(true)} icon={<FolderInput className="w-3.5 h-3.5" aria-hidden="true" />}>
              Move to workspace…
            </MenuItem>
          )}
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
            </>
          )}
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

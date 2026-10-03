import React, { useCallback, useEffect, useId, useRef, useState } from 'react';
import {
  Bug,
  CalendarDays,
  Folder,
  MessageCircle,
  Moon,
  MoreHorizontal,
  PanelLeftClose,
  PanelLeftOpen,
  Pencil,
  Pin,
  PinOff,
  Plus,
  Search,
  Settings,
  Sun,
  Trash2,
} from 'lucide-react';
import { useSidebar } from '../../contexts/SidebarContext';
import { useTheme } from '../../contexts/ThemeContext';
import { cn } from '../../lib/utils';
import { VIEW_TABS, VIEW_TAB_LABELS } from '../../lib/viewTabs';
import type { ViewTab } from '../../lib/viewTabs';
import { relativeTime } from '../../utils/time';
import type { Conversation } from '../../hooks/useConversations';
import { ActivityTray } from './ActivityTray';

export interface SidebarSource {
  id: string;
  name: string;
  status: string;
  fileCount?: number;
  processedCount?: number;
  progress?: number;
}

export interface SidebarLLMStatus {
  connected: boolean;
  model: string;
  provider: string;
}

interface SidebarProps {
  activeView: ViewTab;
  onNavigate: (view: ViewTab) => void;
  conversations: Conversation[];
  activeConversationId: string | null;
  onOpenConversation: (id: string) => void;
  onNewConversation: () => void;
  onRenameConversation: (id: string, title: string) => void;
  onPinConversation: (id: string) => void;
  onDeleteConversation: (id: string) => void;
  sources: SidebarSource[];
  llmStatus: SidebarLLMStatus;
  onOpenCommandPalette: () => void;
  onShowFeedback: () => void;
}

const NAV_ICONS: Record<ViewTab, React.ElementType> = {
  ask: MessageCircle,
  library: Folder,
  calendar: CalendarDays,
  settings: Settings,
};

const EXPANDED_WIDTH = 248;
const COLLAPSED_WIDTH = 64;

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-sidebar';

const IS_MAC = typeof navigator !== 'undefined' && /Mac/i.test(navigator.platform);
const NEW_CONVERSATION_HINT = IS_MAC ? '⌘N' : 'Ctrl N';
const NEW_CONVERSATION_LABEL = `New conversation (${IS_MAC ? '⌘N' : 'Ctrl+N'})`;
const SEARCH_LABEL = `Search and commands (${IS_MAC ? '⌘K' : 'Ctrl+K'})`;

export default function Sidebar({
  activeView,
  onNavigate,
  conversations,
  activeConversationId,
  onOpenConversation,
  onNewConversation,
  onRenameConversation,
  onPinConversation,
  onDeleteConversation,
  sources,
  llmStatus,
  onOpenCommandPalette,
  onShowFeedback,
}: SidebarProps) {
  const { collapsed, toggleSidebar } = useSidebar();
  const { theme, toggleTheme } = useTheme();

  // Ctrl/Cmd+N starts a new conversation. preventDefault stops the WebView
  // from opening a new window.
  const newConversationRef = useRef(onNewConversation);
  newConversationRef.current = onNewConversation;
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'n') {
        e.preventDefault();
        if (!e.repeat) newConversationRef.current();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, []);

  const pinned = conversations.filter(c => c.pinned);
  const recent = [...pinned, ...conversations.filter(c => !c.pinned)];
  const indexingSource = sources.find(s => s.status === 'indexing') ?? null;
  const themeLabel = theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme';

  return (
    <nav
      aria-label="Primary"
      className="h-full shrink-0 flex flex-col border-r border-shodh-border bg-shodh-sidebar text-shodh-text select-none overflow-hidden transition-[width] duration-panel ease-standard"
      style={{ width: collapsed ? COLLAPSED_WIDTH : EXPANDED_WIDTH }}
    >
      {/* Brand */}
      <div
        className={cn(
          'flex items-center gap-2.5 shrink-0',
          collapsed ? 'flex-col px-3 pt-[18px] pb-3' : 'px-[18px] pt-[18px] pb-3.5'
        )}
      >
        <img
          src="/shodh_logo_nobackground.svg"
          alt=""
          aria-hidden="true"
          className="w-[30px] h-[30px] shrink-0"
        />
        {!collapsed && (
          <div className="flex flex-col min-w-0 flex-1 leading-tight">
            <span className="text-[15px] font-bold tracking-[0.02em]">Shodh</span>
            <span className="text-[11px] text-shodh-text-faint" lang="hi">शोध</span>
          </div>
        )}
        <button
          type="button"
          onClick={toggleSidebar}
          aria-label={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
          aria-expanded={!collapsed}
          title={collapsed ? 'Expand sidebar (Ctrl+B)' : 'Collapse sidebar (Ctrl+B)'}
          className={cn(
            'w-8 h-8 rounded-lg shrink-0 inline-flex items-center justify-center text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
            FOCUS_RING
          )}
        >
          {collapsed ? <PanelLeftOpen className="w-4 h-4" aria-hidden="true" /> : <PanelLeftClose className="w-4 h-4" aria-hidden="true" />}
        </button>
      </div>

      {/* New conversation */}
      <div className={cn('shrink-0', collapsed ? 'px-3 pb-2.5 flex justify-center' : 'px-3 pb-2.5')}>
        <button
          type="button"
          onClick={onNewConversation}
          aria-label={collapsed ? NEW_CONVERSATION_LABEL : undefined}
          title={collapsed ? NEW_CONVERSATION_LABEL : undefined}
          aria-keyshortcuts={IS_MAC ? 'Meta+N' : 'Control+N'}
          className={cn(
            'flex items-center gap-2 h-9 rounded-[9px] bg-shodh-accent text-shodh-on-accent text-[13px] font-semibold hover:bg-shodh-accent-hover transition-colors duration-micro',
            collapsed ? 'w-10 justify-center' : 'w-full px-3',
            FOCUS_RING
          )}
        >
          <Plus className="w-[15px] h-[15px] shrink-0" strokeWidth={2.2} aria-hidden="true" />
          {!collapsed && (
            <>
              <span>New conversation</span>
              <kbd className="ml-auto font-mono text-[11px] font-normal">{NEW_CONVERSATION_HINT}</kbd>
            </>
          )}
        </button>
      </div>

      {/* Views */}
      <ul className={cn('flex flex-col gap-0.5 py-1 shrink-0', collapsed ? 'px-3 items-center' : 'px-2')}>
        {VIEW_TABS.map(view => {
          const Icon = NAV_ICONS[view];
          const label = VIEW_TAB_LABELS[view];
          const isActive = activeView === view;
          return (
            <li key={view} className={collapsed ? undefined : 'w-full'}>
              <button
                type="button"
                onClick={() => onNavigate(view)}
                aria-current={isActive ? 'page' : undefined}
                aria-label={collapsed ? label : undefined}
                title={collapsed ? label : undefined}
                className={cn(
                  'flex items-center h-9 rounded-lg text-[13.5px] transition-colors duration-micro',
                  collapsed ? 'w-10 justify-center' : 'w-full gap-2.5 px-3',
                  isActive
                    ? 'bg-shodh-raised-2 text-shodh-text font-semibold'
                    : 'text-shodh-text-secondary font-medium hover:bg-shodh-raised hover:text-shodh-text',
                  FOCUS_RING
                )}
              >
                <Icon
                  className={cn('w-[17px] h-[17px] shrink-0', isActive ? 'text-shodh-accent-text' : 'text-shodh-text-muted')}
                  strokeWidth={1.9}
                  aria-hidden="true"
                />
                {!collapsed && <span className="truncate">{label}</span>}
              </button>
            </li>
          );
        })}
      </ul>

      {/* Recent conversations */}
      {collapsed ? (
        <div className="flex-1 min-h-0" />
      ) : (
        <section aria-labelledby="sidebar-recent-heading" className="flex-1 min-h-0 flex flex-col">
          <h2
            id="sidebar-recent-heading"
            className="px-5 pt-[18px] pb-1.5 text-[11px] font-semibold tracking-[0.08em] text-shodh-text-faint shrink-0"
          >
            RECENT
          </h2>
          {recent.length === 0 ? (
            <p className="px-5 py-2 text-[12px] text-shodh-text-faint">No conversations yet</p>
          ) : (
            <ul className="flex-1 min-h-0 overflow-y-auto scrollbar-thin flex flex-col gap-px px-2 pb-2">
              {recent.map(conv => (
                <RecentConversationRow
                  key={conv.id}
                  conversation={conv}
                  isActive={conv.id === activeConversationId && activeView === 'ask'}
                  onOpen={onOpenConversation}
                  onRename={onRenameConversation}
                  onPin={onPinConversation}
                  onDelete={onDeleteConversation}
                />
              ))}
            </ul>
          )}
        </section>
      )}

      {/* Footer */}
      <div
        className={cn(
          'shrink-0 border-t border-shodh-border flex flex-col gap-2',
          collapsed ? 'p-3 items-center' : 'p-3'
        )}
      >
        {indexingSource ? (
          <IndexingStatus source={indexingSource} collapsed={collapsed} onOpen={() => onNavigate('library')} />
        ) : (
          <ModelStatusChip status={llmStatus} collapsed={collapsed} onOpen={() => onNavigate('settings')} />
        )}

        <div className={cn('flex gap-1', collapsed ? 'flex-col items-center' : 'items-center')}>
          <ActivityTray jobs={sources.filter(s => s.status === 'indexing')} onOpenConversation={onOpenConversation} />
          <FooterIconButton label={SEARCH_LABEL} onClick={onOpenCommandPalette}>
            <Search className="w-4 h-4" aria-hidden="true" />
          </FooterIconButton>
          <FooterIconButton label="Send feedback" onClick={onShowFeedback}>
            <Bug className="w-4 h-4" aria-hidden="true" />
          </FooterIconButton>
          <FooterIconButton label={themeLabel} onClick={toggleTheme}>
            {theme === 'dark' ? <Sun className="w-4 h-4" aria-hidden="true" /> : <Moon className="w-4 h-4" aria-hidden="true" />}
          </FooterIconButton>
        </div>
      </div>
    </nav>
  );
}

function FooterIconButton({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      title={label}
      className={cn(
        'w-8 h-8 rounded-lg inline-flex items-center justify-center text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
        FOCUS_RING
      )}
    >
      {children}
    </button>
  );
}

function IndexingStatus({
  source,
  collapsed,
  onOpen,
}: {
  source: SidebarSource;
  collapsed: boolean;
  onOpen: () => void;
}) {
  const percent = Math.max(0, Math.min(100, Math.round(source.progress ?? 0)));
  const total = source.fileCount ?? 0;
  const processed = source.processedCount ?? 0;
  const detail = total > 0 ? `${processed.toLocaleString()} of ${total.toLocaleString()} files` : 'Preparing files';
  const label = `Indexing ${source.name}: ${detail}, ${percent}%. Open Library`;

  if (collapsed) {
    return (
      <button
        type="button"
        onClick={onOpen}
        aria-label={label}
        title={label}
        className={cn(
          'w-10 h-10 rounded-[10px] bg-shodh-surface-2 border border-shodh-border inline-flex items-center justify-center',
          FOCUS_RING
        )}
      >
        <span className="w-2 h-2 rounded-full bg-shodh-warning motion-safe:animate-pulse" aria-hidden="true" />
      </button>
    );
  }

  return (
    <div className="flex flex-col gap-1">
      <button
        type="button"
        onClick={onOpen}
        aria-label={label}
        className={cn(
          'flex items-center gap-2.5 px-3 py-2.5 rounded-[10px] bg-shodh-surface-2 border border-shodh-border text-left hover:border-shodh-border-strong transition-colors duration-micro',
          FOCUS_RING
        )}
      >
        <span className="w-2 h-2 rounded-full bg-shodh-warning shrink-0" aria-hidden="true" />
        <span className="flex flex-col gap-0.5 flex-1 min-w-0">
          <span className="text-[12px] font-semibold truncate">Indexing {source.name}</span>
          <span className="text-[11px] text-shodh-text-faint">{detail}</span>
        </span>
        <span className="font-mono text-[11px] text-shodh-text-secondary">{percent}%</span>
      </button>
      <div
        role="progressbar"
        aria-label={`Indexing ${source.name}`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        className="h-[3px] mx-1 rounded-[3px] bg-shodh-border overflow-hidden"
      >
        <div
          className="h-full bg-shodh-warning origin-left transition-transform duration-panel ease-standard"
          style={{ transform: `scaleX(${percent / 100})` }}
        />
      </div>
    </div>
  );
}

function ModelStatusChip({
  status,
  collapsed,
  onOpen,
}: {
  status: SidebarLLMStatus;
  collapsed: boolean;
  onOpen: () => void;
}) {
  const name = status.connected ? status.model : 'No model configured';
  const label = status.connected
    ? `Model: ${status.model}${status.provider && status.provider !== 'none' ? ` via ${status.provider}` : ''}. Open model settings`
    : 'No model configured. Open model settings';
  const dotClass = status.connected ? 'bg-shodh-success' : 'bg-shodh-warning';

  if (collapsed) {
    return (
      <button
        type="button"
        onClick={onOpen}
        aria-label={label}
        title={label}
        className={cn(
          'w-10 h-10 rounded-[10px] inline-flex items-center justify-center hover:bg-shodh-raised transition-colors duration-micro',
          FOCUS_RING
        )}
      >
        <span className={cn('w-2 h-2 rounded-full', dotClass)} aria-hidden="true" />
      </button>
    );
  }

  return (
    <button
      type="button"
      onClick={onOpen}
      aria-label={label}
      title={label}
      className={cn(
        'flex items-center gap-2 h-8 px-3 rounded-full border border-shodh-border text-left hover:bg-shodh-raised transition-colors duration-micro min-w-0',
        FOCUS_RING
      )}
    >
      <span className={cn('w-2 h-2 rounded-full shrink-0', dotClass)} aria-hidden="true" />
      <span className="text-[12px] font-medium text-shodh-text-secondary truncate">{name}</span>
    </button>
  );
}

interface RecentConversationRowProps {
  conversation: Conversation;
  isActive: boolean;
  onOpen: (id: string) => void;
  onRename: (id: string, title: string) => void;
  onPin: (id: string) => void;
  onDelete: (id: string) => void;
}

function RecentConversationRow({
  conversation,
  isActive,
  onOpen,
  onRename,
  onPin,
  onDelete,
}: RecentConversationRowProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [draftTitle, setDraftTitle] = useState(conversation.title);
  const rowRef = useRef<HTMLLIElement>(null);
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
    if (menuOpen) {
      menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
    }
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

  const commitRename = () => {
    const title = draftTitle.trim();
    if (title && title !== conversation.title) onRename(conversation.id, title);
    setEditing(false);
  };

  const cancelRename = () => {
    setEditing(false);
    setDraftTitle(conversation.title);
  };

  const handleMenuKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? []);
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === 'Escape') {
      e.preventDefault();
      closeMenu(true);
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      items[(index + 1) % items.length]?.focus();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
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
          type="button"
          onClick={() => onOpen(conversation.id)}
          aria-current={isActive ? 'page' : undefined}
          className={cn(
            'w-full flex flex-col gap-0.5 py-2 pl-3 pr-9 rounded-lg text-left transition-colors duration-micro',
            isActive ? 'bg-shodh-raised text-shodh-text' : 'text-shodh-text-secondary hover:bg-shodh-raised/60 hover:text-shodh-text',
            FOCUS_RING
          )}
        >
          <span className="flex items-center gap-1.5 min-w-0">
            {conversation.pinned && (
              <Pin className="w-3 h-3 shrink-0 text-shodh-text-faint" aria-label="Pinned" />
            )}
            <span className="text-[13px] truncate">{conversation.title}</span>
          </span>
          {meta && <span className="text-[11px] text-shodh-text-faint truncate">{meta}</span>}
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
            'absolute right-1.5 top-1.5 w-7 h-7 rounded-md inline-flex items-center justify-center text-shodh-text-muted hover:bg-shodh-raised-2 hover:text-shodh-text transition-opacity duration-micro',
            menuOpen ? 'opacity-100' : 'opacity-0 group-hover:opacity-100 focus-visible:opacity-100',
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
          className="absolute right-1.5 top-9 z-50 min-w-[152px] py-1 rounded-lg border border-shodh-border-strong bg-shodh-raised shadow-lg"
        >
          <MenuItem onSelect={startRename} icon={<Pencil className="w-3.5 h-3.5" aria-hidden="true" />}>
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
  destructive = false,
  children,
}: {
  onSelect: () => void;
  icon: React.ReactNode;
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
      {children}
    </button>
  );
}

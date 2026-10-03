import React, { useEffect, useRef } from 'react';
import {
  Activity,
  Bug,
  Folder,
  ListChecks,
  MessageCircle,
  Moon,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  Search,
  Settings,
  Sun,
} from 'lucide-react';
import { useSidebar } from '../../contexts/SidebarContext';
import { useTheme } from '../../contexts/ThemeContext';
import { cn } from '../../lib/utils';
import { VIEW_TABS, VIEW_TAB_LABELS } from '../../lib/viewTabs';
import type { ViewTab } from '../../lib/viewTabs';
import type { Conversation } from '../../hooks/useConversations';
import { ActivityTray } from './ActivityTray';
import { ConversationHistory } from './ConversationHistory';

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

/** Icon for each view; shared with the command palette. */
export const NAV_ICONS: Record<ViewTab, React.ElementType> = {
  ask: MessageCircle,
  library: Folder,
  tasks: ListChecks,
  activity: Activity,
  settings: Settings,
};

/** Views in the main navigation; Settings sits in the footer. */
const PRIMARY_VIEWS = VIEW_TABS.filter((v): v is Exclude<ViewTab, 'settings'> => v !== 'settings');

const EXPANDED_WIDTH = 248;
const COLLAPSED_WIDTH = 64;

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-sidebar';

const IS_MAC = typeof navigator !== 'undefined' && /Mac/i.test(navigator.platform);
const NEW_CHAT_HINT = IS_MAC ? '⌘N' : 'Ctrl N';
const NEW_CHAT_LABEL = `New chat (${IS_MAC ? '⌘N' : 'Ctrl+N'})`;
const SEARCH_LABEL = `Search and commands (${IS_MAC ? '⌘K' : 'Ctrl+K'})`;
const TOGGLE_HINT = IS_MAC ? '⌘B' : 'Ctrl+B';

/** Arrow-key movement between the buttons of a vertical list. */
function moveFocusInList(e: React.KeyboardEvent<HTMLElement>, selector: string) {
  if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return;
  const items = Array.from(e.currentTarget.querySelectorAll<HTMLElement>(selector));
  const index = items.indexOf(document.activeElement as HTMLElement);
  if (index < 0) return;
  e.preventDefault();
  const next =
    e.key === 'Home' ? 0
    : e.key === 'End' ? items.length - 1
    : e.key === 'ArrowDown' ? (index + 1) % items.length
    : (index - 1 + items.length) % items.length;
  items[next]?.focus();
}

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

  // Ctrl/Cmd+N starts a new chat. preventDefault stops the WebView from
  // opening a new window.
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
          aria-keyshortcuts={IS_MAC ? 'Meta+B' : 'Control+B'}
          title={`${collapsed ? 'Expand' : 'Collapse'} sidebar (${TOGGLE_HINT})`}
          className={cn(
            'w-8 h-8 rounded-lg shrink-0 inline-flex items-center justify-center text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
            FOCUS_RING
          )}
        >
          {collapsed ? <PanelLeftOpen className="w-4 h-4" aria-hidden="true" /> : <PanelLeftClose className="w-4 h-4" aria-hidden="true" />}
        </button>
      </div>

      {/* New chat */}
      <div className={cn('shrink-0', collapsed ? 'px-3 pb-2.5 flex justify-center' : 'px-3 pb-2.5')}>
        <button
          type="button"
          onClick={onNewConversation}
          aria-label={collapsed ? NEW_CHAT_LABEL : undefined}
          title={collapsed ? NEW_CHAT_LABEL : undefined}
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
              <span>New chat</span>
              <kbd className="ml-auto font-mono text-[11px] font-normal">{NEW_CHAT_HINT}</kbd>
            </>
          )}
        </button>
      </div>

      {/* Views */}
      <ul
        className={cn('flex flex-col gap-0.5 py-1 shrink-0', collapsed ? 'px-3 items-center' : 'px-2')}
        onKeyDown={e => moveFocusInList(e, '[data-nav-item]')}
      >
        {PRIMARY_VIEWS.map(view => (
          <li key={view} className={collapsed ? undefined : 'w-full'}>
            <NavButton view={view} active={activeView === view} collapsed={collapsed} onNavigate={onNavigate} />
          </li>
        ))}
      </ul>

      {/* Conversation history (under Ask) */}
      {collapsed ? (
        <div className="flex-1 min-h-0" />
      ) : (
        <ConversationHistory
          conversations={conversations}
          activeConversationId={activeConversationId}
          askActive={activeView === 'ask'}
          onOpen={onOpenConversation}
          onRename={onRenameConversation}
          onPin={onPinConversation}
          onDelete={onDeleteConversation}
        />
      )}

      {/* Footer */}
      <div
        className={cn(
          'shrink-0 border-t border-shodh-border flex flex-col gap-2',
          collapsed ? 'p-3 items-center' : 'p-3'
        )}
      >
        <div className={collapsed ? undefined : 'w-full'}>
          <NavButton view="settings" active={activeView === 'settings'} collapsed={collapsed} onNavigate={onNavigate} />
        </div>

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

function NavButton({
  view,
  active,
  collapsed,
  onNavigate,
}: {
  view: ViewTab;
  active: boolean;
  collapsed: boolean;
  onNavigate: (view: ViewTab) => void;
}) {
  const Icon = NAV_ICONS[view];
  const label = VIEW_TAB_LABELS[view];
  return (
    <button
      type="button"
      data-nav-item=""
      onClick={() => onNavigate(view)}
      aria-current={active ? 'page' : undefined}
      aria-label={collapsed ? label : undefined}
      title={collapsed ? label : undefined}
      className={cn(
        'flex items-center h-9 rounded-lg text-[13.5px] transition-colors duration-micro',
        collapsed ? 'w-10 justify-center' : 'w-full gap-2.5 px-3',
        active
          ? 'bg-shodh-raised-2 text-shodh-text font-semibold'
          : 'text-shodh-text-secondary font-medium hover:bg-shodh-raised hover:text-shodh-text',
        FOCUS_RING
      )}
    >
      <Icon
        className={cn('w-[17px] h-[17px] shrink-0', active ? 'text-shodh-accent-text' : 'text-shodh-text-muted')}
        strokeWidth={1.9}
        aria-hidden="true"
      />
      {!collapsed && <span className="truncate">{label}</span>}
    </button>
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

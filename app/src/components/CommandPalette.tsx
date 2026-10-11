import React, { useEffect, useId, useMemo, useRef, useState } from 'react';
import { AnimatePresence, motion } from 'framer-motion';
import { ArrowRight, FileText, Layers, MessageCircle, MessageSquarePlus, Search } from 'lucide-react';
import { cn } from '../lib/utils';
import { ENTER_TRANSITION, EXIT_TRANSITION } from '../lib/motion';
import { filterEntries, groupBySection } from '../lib/paletteFilter';
import type { PaletteEntry } from '../lib/paletteFilter';
import { VIEW_TABS, VIEW_TAB_DESCRIPTIONS, VIEW_TAB_KEYWORDS, VIEW_TAB_LABELS } from '../lib/viewTabs';
import type { ViewTab } from '../lib/viewTabs';
import { relativeTime } from '../utils/time';
import { NAV_ICONS } from './shell/Sidebar';

/** A primary action offered by the shell (new chat, add folder, …). */
export interface PaletteAction {
  id: string;
  label: string;
  description?: string;
  icon: React.ElementType;
  keywords?: string;
  /** Shown as a keyboard hint, e.g. "Ctrl+N". */
  shortcut?: string;
  run: () => void;
}

export interface PaletteConversation {
  id: string;
  title: string;
  updatedAt: string;
  /** Name of the chat's workspace, if any. */
  workspaceName?: string;
}

export interface PaletteWorkspace {
  id: string;
  name: string;
  /** "2 folders · 5 files", shown under the name. */
  summary: string;
}

export interface PaletteSource {
  id: string;
  name: string;
  path?: string;
}

interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  onNavigate: (view: ViewTab) => void;
  actions: PaletteAction[];
  conversations: PaletteConversation[];
  onOpenConversation: (id: string) => void;
  sources: PaletteSource[];
  workspaces: PaletteWorkspace[];
  onOpenWorkspace: (id: string) => void;
  onNewChatInWorkspace: (id: string) => void;
}

type Section = 'Actions' | 'Go to' | 'Workspaces' | 'Chats' | 'Library';
const SECTION_ORDER: readonly Section[] = ['Actions', 'Go to', 'Workspaces', 'Chats', 'Library'];

/** Chats listed before anything is typed. */
const RECENT_CHATS = 5;
/** Chats listed for a query. */
const MATCHING_CHATS = 30;

interface Entry extends PaletteEntry {
  section: Section;
  description?: string;
  icon: React.ElementType;
  shortcut?: string;
  run: () => void;
}

/**
 * Ctrl+K: every view, the primary actions, workspaces (open one, or start a chat in it),
 * chats (recent first, all of them searchable by title) and Library folders.
 */
export default function CommandPalette({
  open,
  onClose,
  onNavigate,
  actions,
  conversations,
  onOpenConversation,
  sources,
  workspaces,
  onOpenWorkspace,
  onNewChatInWorkspace,
}: CommandPaletteProps) {
  const [query, setQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  const baseId = useId();
  const listId = `${baseId}-list`;

  // Reset on open; give focus back to whatever had it on close.
  useEffect(() => {
    if (open) {
      restoreFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      setQuery('');
      setSelectedIndex(0);
    } else if (restoreFocusRef.current) {
      const target = restoreFocusRef.current;
      restoreFocusRef.current = null;
      if (target.isConnected) target.focus();
    }
  }, [open]);

  const entries = useMemo<Entry[]>(() => {
    const close = (fn: () => void) => () => { onClose(); fn(); };
    const items: Entry[] = [];

    for (const action of actions) {
      items.push({
        id: `action-${action.id}`,
        section: 'Actions',
        label: action.label,
        description: action.description,
        keywords: action.keywords,
        icon: action.icon,
        shortcut: action.shortcut,
        run: close(action.run),
      });
    }

    for (const view of VIEW_TABS) {
      items.push({
        id: `view-${view}`,
        section: 'Go to',
        label: VIEW_TAB_LABELS[view],
        description: VIEW_TAB_DESCRIPTIONS[view],
        keywords: `go open view ${VIEW_TAB_KEYWORDS[view]}`,
        icon: NAV_ICONS[view],
        run: close(() => onNavigate(view)),
      });
    }

    for (const workspace of workspaces) {
      items.push({
        id: `workspace-${workspace.id}`,
        section: 'Workspaces',
        label: workspace.name,
        description: workspace.summary,
        keywords: `workspace open switch project ${workspace.name}`,
        icon: Layers,
        run: close(() => onOpenWorkspace(workspace.id)),
      });
      items.push({
        id: `workspace-chat-${workspace.id}`,
        section: 'Workspaces',
        label: `New chat in ${workspace.name}`,
        description: 'Searches only this workspace’s sources',
        keywords: `new chat ask switch workspace ${workspace.name}`,
        icon: MessageSquarePlus,
        run: close(() => onNewChatInWorkspace(workspace.id)),
      });
    }

    const byRecency = [...conversations].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
    for (const conv of byRecency) {
      items.push({
        id: `chat-${conv.id}`,
        section: 'Chats',
        label: conv.title,
        description: [conv.workspaceName, relativeTime(conv.updatedAt)].filter(Boolean).join(' · '),
        keywords: `chat conversation ${conv.workspaceName ?? ''}`,
        icon: MessageCircle,
        run: close(() => onOpenConversation(conv.id)),
      });
    }

    for (const source of sources) {
      items.push({
        id: `source-${source.id}`,
        section: 'Library',
        label: source.name,
        description: source.path,
        keywords: `folder source library ${source.path ?? ''}`,
        icon: FileText,
        run: close(() => onNavigate('library')),
      });
    }
    return items;
  }, [actions, conversations, sources, workspaces, onNavigate, onOpenConversation, onOpenWorkspace, onNewChatInWorkspace, onClose]);

  const groups = useMemo(() => {
    const matched = filterEntries(entries, query, SECTION_ORDER);
    const limit = query.trim() ? MATCHING_CHATS : RECENT_CHATS;
    let chats = 0;
    let spaces = 0;
    // Without a query, "new chat in …" entries would crowd the list: workspaces only.
    const typed = query.trim().length > 0;
    return groupBySection(matched.filter(e => {
      if (e.section === 'Chats') return chats++ < limit;
      if (e.section === 'Workspaces') return (typed || !e.id.startsWith('workspace-chat-')) && spaces++ < (typed ? MATCHING_CHATS : RECENT_CHATS);
      return true;
    }));
  }, [entries, query]);

  const flat = useMemo(() => groups.flatMap(g => g.items), [groups]);

  useEffect(() => {
    setSelectedIndex(0);
  }, [query]);

  const activeIndex = Math.min(selectedIndex, Math.max(0, flat.length - 1));
  const activeId = flat[activeIndex] ? `${baseId}-${flat[activeIndex].id}` : undefined;

  // Keep the active option in view.
  useEffect(() => {
    if (!open || !activeId) return;
    document.getElementById(activeId)?.scrollIntoView({ block: 'nearest' });
  }, [open, activeId]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (flat.length > 0) setSelectedIndex((activeIndex + 1) % flat.length);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (flat.length > 0) setSelectedIndex((activeIndex - 1 + flat.length) % flat.length);
    } else if (e.key === 'PageDown') {
      e.preventDefault();
      setSelectedIndex(Math.min(flat.length - 1, activeIndex + 8));
    } else if (e.key === 'PageUp') {
      e.preventDefault();
      setSelectedIndex(Math.max(0, activeIndex - 8));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      flat[activeIndex]?.run();
    } else if (e.key === 'Tab') {
      // The palette is modal and its input is the only tab stop.
      e.preventDefault();
    }
  };

  let optionIndex = -1;

  return (
    <AnimatePresence>
      {open && (
        <div className="fixed inset-0 z-[9999] flex items-start justify-center pt-[14vh] px-4" onKeyDown={handleKeyDown}>
          <motion.div
            className="absolute inset-0 bg-black/50"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1, transition: ENTER_TRANSITION }}
            exit={{ opacity: 0, transition: EXIT_TRANSITION }}
            onClick={onClose}
            aria-hidden="true"
          />
          <motion.div
            role="dialog"
            aria-modal="true"
            aria-label="Search and commands"
            className="relative w-full max-w-[600px] rounded-xl border border-shodh-border-strong bg-shodh-raised shadow-2xl overflow-hidden flex flex-col max-h-[min(480px,70vh)]"
            initial={{ opacity: 0, scale: 0.97, y: -6 }}
            animate={{ opacity: 1, scale: 1, y: 0, transition: ENTER_TRANSITION }}
            exit={{ opacity: 0, scale: 0.98, y: -4, transition: EXIT_TRANSITION }}
          >
            <div className="flex items-center gap-3 px-4 h-12 border-b border-shodh-border shrink-0">
              <Search className="w-4 h-4 shrink-0 text-shodh-text-muted" aria-hidden="true" />
              <input
                autoFocus
                type="text"
                role="combobox"
                aria-expanded="true"
                aria-controls={listId}
                aria-activedescendant={activeId}
                aria-autocomplete="list"
                aria-label="Search views, actions, chats and folders"
                value={query}
                onChange={e => setQuery(e.target.value)}
                placeholder="Search views, actions, chats and folders…"
                className="flex-1 bg-transparent text-[14px] text-shodh-text placeholder:text-shodh-text-faint outline-none"
              />
              <kbd className="text-[10.5px] px-1.5 py-0.5 rounded border border-shodh-border font-mono text-shodh-text-muted shrink-0">
                Esc
              </kbd>
            </div>

            <div id={listId} role="listbox" aria-label="Results" className="overflow-y-auto scrollbar-thin py-1.5 flex-1 min-h-0">
              {flat.length === 0 ? (
                <p className="px-4 py-8 text-center text-[13px] text-shodh-text-muted" role="status">
                  Nothing matches “{query.trim()}”.
                </p>
              ) : (
                groups.map(group => (
                  <div key={group.section} role="group" aria-labelledby={`${baseId}-g-${group.section}`}>
                    <div
                      id={`${baseId}-g-${group.section}`}
                      className="px-4 pt-2 pb-1 text-[11px] font-semibold tracking-[0.08em] uppercase text-shodh-text-faint"
                    >
                      {group.section}
                    </div>
                    {group.items.map(item => {
                      optionIndex += 1;
                      const index = optionIndex;
                      const selected = index === activeIndex;
                      const Icon = item.icon;
                      return (
                        <div
                          key={item.id}
                          id={`${baseId}-${item.id}`}
                          role="option"
                          aria-selected={selected}
                          onClick={item.run}
                          onMouseMove={() => { if (!selected) setSelectedIndex(index); }}
                          className={cn(
                            'mx-1.5 flex items-center gap-3 px-2.5 h-9 rounded-lg cursor-pointer transition-colors duration-micro',
                            selected ? 'bg-shodh-raised-2 text-shodh-text' : 'text-shodh-text-secondary',
                          )}
                        >
                          <Icon
                            className={cn('w-4 h-4 shrink-0', selected ? 'text-shodh-accent-text' : 'text-shodh-text-muted')}
                            aria-hidden="true"
                          />
                          <span className="flex-1 min-w-0 flex items-baseline gap-2">
                            <span className="text-[13.5px] font-medium truncate">{item.label}</span>
                            {item.description && (
                              <span className="text-[12px] text-shodh-text-faint truncate">{item.description}</span>
                            )}
                          </span>
                          {item.shortcut && (
                            <kbd className="text-[10.5px] font-mono text-shodh-text-faint shrink-0">{item.shortcut}</kbd>
                          )}
                          {selected && <ArrowRight className="w-3.5 h-3.5 shrink-0 text-shodh-text-muted" aria-hidden="true" />}
                        </div>
                      );
                    })}
                  </div>
                ))
              )}
            </div>
            <div
              className="shrink-0 border-t border-shodh-border px-4 h-8 flex items-center gap-4 text-[11px] text-shodh-text-faint"
              aria-hidden="true"
            >
              <span><kbd className="font-mono">↑ ↓</kbd> move</span>
              <span><kbd className="font-mono">Enter</kbd> open</span>
              <span><kbd className="font-mono">Esc</kbd> close</span>
            </div>
          </motion.div>
        </div>
      )}
    </AnimatePresence>
  );
}

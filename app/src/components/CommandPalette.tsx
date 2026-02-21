import React, { useState, useEffect, useRef, useMemo } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { invoke } from '@tauri-apps/api/core';
import {
  Search,
  MessageSquare,
  FileText,
  Sparkles,
  BarChart3,
  Zap,
  GitBranch,
  Moon,
  Sun,
  Plus,
  Clock,
  Settings,
  FolderOpen,
  ArrowRight,
} from 'lucide-react';
import { useTheme } from '../contexts/ThemeContext';
import type { ViewTab } from './AppSidebar';

interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  onNavigate: (view: ViewTab) => void;
  onNewConversation: () => void;
  onToggleTheme: () => void;
  onOpenLLMSettings: () => void;
  onAddSource: () => void;
  sources: { id: string; name: string; selected: boolean }[];
}

interface CommandItem {
  id: string;
  label: string;
  description?: string;
  icon: React.ElementType;
  section: 'recent' | 'actions' | 'navigate' | 'sources';
  action: () => void;
  keywords?: string;
}

export default function CommandPalette({
  open,
  onClose,
  onNavigate,
  onNewConversation,
  onToggleTheme,
  onOpenLLMSettings,
  onAddSource,
  sources,
}: CommandPaletteProps) {
  const { theme, colors } = useTheme();
  const [query, setQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [recentSearches, setRecentSearches] = useState<string[]>([]);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (open) {
      setQuery('');
      setSelectedIndex(0);
      setTimeout(() => inputRef.current?.focus(), 50);

      invoke<any[]>('get_search_history', { spaceId: null, limit: 5 })
        .then(entries => {
          setRecentSearches(entries.map((e: any) => e.query || e.search_query || '').filter(Boolean));
        })
        .catch(() => {});
    }
  }, [open]);

  const commands = useMemo<CommandItem[]>(() => {
    const items: CommandItem[] = [];

    // Recent searches
    recentSearches.forEach((q, i) => {
      items.push({
        id: `recent-${i}`,
        label: q,
        icon: Clock,
        section: 'recent',
        action: () => {
          onNavigate('chat');
          onClose();
        },
        keywords: q,
      });
    });

    // Actions
    items.push({
      id: 'new-chat',
      label: 'New Chat',
      description: 'Start a new conversation',
      icon: Plus,
      section: 'actions',
      action: () => { onNewConversation(); onClose(); },
      keywords: 'new chat conversation create',
    });
    items.push({
      id: 'toggle-theme',
      label: theme === 'dark' ? 'Switch to Light Mode' : 'Switch to Dark Mode',
      description: 'Toggle between light and dark theme',
      icon: theme === 'dark' ? Sun : Moon,
      section: 'actions',
      action: () => { onToggleTheme(); onClose(); },
      keywords: 'theme dark light mode toggle',
    });
    items.push({
      id: 'llm-settings',
      label: 'AI Model Settings',
      description: 'Configure LLM provider and model',
      icon: Settings,
      section: 'actions',
      action: () => { onOpenLLMSettings(); onClose(); },
      keywords: 'settings model llm ai configure provider',
    });
    items.push({
      id: 'add-source',
      label: 'Add Document Source',
      description: 'Index a new folder of documents',
      icon: FolderOpen,
      section: 'actions',
      action: () => { onAddSource(); onClose(); },
      keywords: 'add source folder documents index workspace',
    });

    // Navigate
    const navItems: { id: ViewTab; label: string; icon: React.ElementType; keywords: string }[] = [
      { id: 'chat', label: 'Chat', icon: MessageSquare, keywords: 'chat messages conversation' },
      { id: 'documents', label: 'Documents', icon: FileText, keywords: 'documents files sources' },
      { id: 'generate', label: 'Generate', icon: Sparkles, keywords: 'generate create document write' },
      { id: 'analytics', label: 'Analytics', icon: BarChart3, keywords: 'analytics dashboard stats metrics' },
      { id: 'graph', label: 'Knowledge Graph', icon: GitBranch, keywords: 'graph knowledge relationships nodes entities' },
      { id: 'integrations', label: 'Integrations', icon: Zap, keywords: 'integrations telegram whatsapp discord' },
    ];
    navItems.forEach(nav => {
      items.push({
        id: `nav-${nav.id}`,
        label: `Go to ${nav.label}`,
        icon: nav.icon,
        section: 'navigate',
        action: () => { onNavigate(nav.id); onClose(); },
        keywords: nav.keywords,
      });
    });

    // Sources
    sources.forEach(source => {
      items.push({
        id: `source-${source.id}`,
        label: source.name,
        description: source.selected ? 'Active' : 'Inactive',
        icon: FileText,
        section: 'sources',
        action: () => { onNavigate('chat'); onClose(); },
        keywords: `source ${source.name}`,
      });
    });

    return items;
  }, [recentSearches, theme, sources, onNavigate, onNewConversation, onToggleTheme, onOpenLLMSettings, onAddSource, onClose]);

  const filtered = useMemo(() => {
    if (!query.trim()) return commands;
    const q = query.toLowerCase();
    return commands.filter(
      cmd => cmd.label.toLowerCase().includes(q) || cmd.keywords?.toLowerCase().includes(q)
    );
  }, [query, commands]);

  const sections = useMemo(() => {
    const groups: { key: string; label: string; items: CommandItem[] }[] = [];
    const sectionOrder = ['recent', 'actions', 'navigate', 'sources'] as const;
    const sectionLabels: Record<string, string> = {
      recent: 'Recent',
      actions: 'Actions',
      navigate: 'Navigate',
      sources: 'Sources',
    };

    for (const key of sectionOrder) {
      const items = filtered.filter(c => c.section === key);
      if (items.length > 0) {
        groups.push({ key, label: sectionLabels[key], items });
      }
    }
    return groups;
  }, [filtered]);

  const flatItems = useMemo(() => sections.flatMap(s => s.items), [sections]);

  useEffect(() => {
    setSelectedIndex(0);
  }, [query]);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (!open) return;

      if (e.key === 'Escape') {
        e.preventDefault();
        onClose();
      } else if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSelectedIndex(prev => Math.min(prev + 1, flatItems.length - 1));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSelectedIndex(prev => Math.max(prev - 1, 0));
      } else if (e.key === 'Enter') {
        e.preventDefault();
        if (flatItems[selectedIndex]) {
          flatItems[selectedIndex].action();
        }
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [open, flatItems, selectedIndex, onClose]);

  // Scroll selected item into view
  useEffect(() => {
    if (listRef.current) {
      const selected = listRef.current.querySelector(`[data-index="${selectedIndex}"]`);
      selected?.scrollIntoView({ block: 'nearest' });
    }
  }, [selectedIndex]);

  if (!open) return null;

  let globalIndex = -1;

  return (
    <div className="fixed inset-0 z-[9999] flex items-start justify-center pt-[15vh]">
      {/* Backdrop */}
      <motion.div
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        exit={{ opacity: 0 }}
        transition={{ duration: 0.15 }}
        className="absolute inset-0 bg-black/50 backdrop-blur-sm"
        onClick={onClose}
      />

      {/* Panel */}
      <motion.div
        initial={{ opacity: 0, scale: 0.96, y: -8 }}
        animate={{ opacity: 1, scale: 1, y: 0 }}
        exit={{ opacity: 0, scale: 0.96, y: -8 }}
        transition={{ duration: 0.15, ease: 'easeOut' }}
        className="relative w-[560px] max-h-[420px] rounded-xl border overflow-hidden"
        style={{
          backgroundColor: colors.bgSecondary,
          borderColor: colors.border,
          boxShadow: '0 24px 48px rgba(0,0,0,0.25)',
        }}
      >
        {/* Search input */}
        <div className="flex items-center gap-3 px-4 py-3 border-b" style={{ borderColor: colors.border }}>
          <Search className="w-4 h-4 shrink-0" style={{ color: colors.textMuted }} />
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={e => setQuery(e.target.value)}
            placeholder="Type a command or search..."
            className="flex-1 bg-transparent text-sm outline-none"
            style={{ color: colors.text }}
          />
          <kbd
            className="text-[10px] px-1.5 py-0.5 rounded border font-mono shrink-0"
            style={{ borderColor: colors.border, color: colors.textMuted }}
          >
            ESC
          </kbd>
        </div>

        {/* Results */}
        <div ref={listRef} className="overflow-y-auto max-h-[340px] py-1">
          {flatItems.length === 0 ? (
            <div className="px-4 py-8 text-center">
              <p className="text-sm" style={{ color: colors.textMuted }}>No results found</p>
            </div>
          ) : (
            sections.map(section => (
              <div key={section.key}>
                <div className="px-4 pt-2 pb-1">
                  <span className="text-[10px] font-bold tracking-widest" style={{ color: colors.textMuted }}>
                    {section.label.toUpperCase()}
                  </span>
                </div>
                {section.items.map(item => {
                  globalIndex++;
                  const idx = globalIndex;
                  const isSelected = idx === selectedIndex;
                  const Icon = item.icon;

                  return (
                    <button
                      key={item.id}
                      data-index={idx}
                      onClick={item.action}
                      onMouseEnter={() => setSelectedIndex(idx)}
                      className="w-full flex items-center gap-3 px-4 py-2 text-left transition-colors"
                      style={{
                        backgroundColor: isSelected ? colors.bgHover : 'transparent',
                      }}
                    >
                      <Icon
                        className="w-4 h-4 shrink-0"
                        style={{ color: isSelected ? colors.primary : colors.textTertiary }}
                      />
                      <div className="flex-1 min-w-0">
                        <span
                          className="text-sm font-medium"
                          style={{ color: isSelected ? colors.text : colors.textSecondary }}
                        >
                          {item.label}
                        </span>
                        {item.description && (
                          <span className="ml-2 text-xs" style={{ color: colors.textMuted }}>
                            {item.description}
                          </span>
                        )}
                      </div>
                      {isSelected && (
                        <ArrowRight className="w-3.5 h-3.5 shrink-0" style={{ color: colors.textMuted }} />
                      )}
                    </button>
                  );
                })}
              </div>
            ))
          )}
        </div>
      </motion.div>
    </div>
  );
}

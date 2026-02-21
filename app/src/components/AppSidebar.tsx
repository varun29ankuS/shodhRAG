import React, { useState } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import {
  MessageSquare,
  FileText,
  Sparkles,
  BarChart3,
  Zap,
  GitBranch,
  Search,
  PanelLeftClose,
  PanelLeft,
  ChevronDown,
  ChevronRight,
  FolderOpen,
  Check,
  Plus,
  Bot,
  ChevronUp,
  Bug,
  Loader2,
  Trash2,
  AlertTriangle,
  CalendarCheck,
} from 'lucide-react';
import { useTheme } from '../contexts/ThemeContext';
import { useSidebar } from '../contexts/SidebarContext';
import { sourceColor } from '../utils/colors';
import { ThemeToggle } from './ThemeToggle';
import ConversationList from './ConversationList';
import type { Conversation } from '../hooks/useConversations';

export type ViewTab = 'chat' | 'documents' | 'generate' | 'analytics' | 'calendar' | 'graph' | 'agents' | 'integrations';

interface SourceItem {
  id: string;
  name: string;
  selected: boolean;
  status: string;
  fileCount?: number;
  chunkCount?: number;
  path?: string;
  indexedAt?: string;
  progress?: number;
  currentFile?: string;
  processedCount?: number;
}

interface LLMStatus {
  connected: boolean;
  model: string;
  provider: string;
}

interface AppSidebarProps {
  activeView: ViewTab;
  onViewChange: (view: ViewTab) => void;
  conversations: Conversation[];
  activeConversationId: string | null;
  onSelectConversation: (id: string) => void;
  onNewConversation: () => void;
  onDeleteConversation: (id: string) => void;
  onRenameConversation: (id: string, title: string) => void;
  onPinConversation: (id: string) => void;
  onReorderConversations?: (reordered: Conversation[]) => void;
  sources: SourceItem[];
  onToggleSource: (id: string) => void;
  onAddSource: () => void;
  onRemoveSource: (id: string, e: React.MouseEvent) => void;
  expandedSources: Set<string>;
  sourceFiles: Record<string, any[]>;
  onToggleSourceExpansion: (sourceId: string, e: React.MouseEvent) => void;
  getFileIconInfo: (fileType: string) => { Icon: any; color: string; badge: string };
  llmStatus: LLMStatus;
  onOpenLLMSettings: () => void;
  onOpenCommandPalette: () => void;
  onShowFeedback: () => void;
  stats: { totalDocs: number; selectedDocs: number; indexSize: string };
}

const navItems: { id: ViewTab; label: string; icon: React.ElementType }[] = [
  { id: 'chat', label: 'Chat', icon: MessageSquare },
  { id: 'documents', label: 'Documents', icon: FileText },
  { id: 'generate', label: 'Generate', icon: Sparkles },
  { id: 'analytics', label: 'Analytics', icon: BarChart3 },
  { id: 'calendar', label: 'Tasks', icon: CalendarCheck },
  { id: 'graph', label: 'Graph', icon: GitBranch },
  { id: 'agents', label: 'Agents', icon: Bot },
  { id: 'integrations', label: 'Integrations', icon: Zap },
];


export default function AppSidebar({
  activeView,
  onViewChange,
  conversations,
  activeConversationId,
  onSelectConversation,
  onNewConversation,
  onDeleteConversation,
  onRenameConversation,
  onPinConversation,
  onReorderConversations,
  sources,
  onToggleSource,
  onAddSource,
  onRemoveSource,
  expandedSources,
  sourceFiles,
  onToggleSourceExpansion,
  getFileIconInfo,
  llmStatus,
  onOpenLLMSettings,
  onOpenCommandPalette,
  onShowFeedback,
  stats,
}: AppSidebarProps) {
  const { theme, colors } = useTheme();
  const { collapsed, toggleSidebar } = useSidebar();
  const [sourcesSectionExpanded, setSourcesSectionExpanded] = useState(true);

  const sidebarWidth = collapsed ? 48 : 280;

  return (
    <motion.div
      className="h-full flex flex-col border-r shrink-0 select-none"
      style={{
        backgroundColor: colors.bgSecondary,
        borderColor: colors.border,
        width: sidebarWidth,
      }}
      animate={{ width: sidebarWidth }}
      transition={{ duration: 0.2, ease: 'easeInOut' }}
    >
      {/* Header */}
      <div className="flex items-center gap-2 px-3 py-3 border-b" style={{ borderColor: colors.border }}>
        {!collapsed && (
          <div className="flex items-center gap-2 flex-1 min-w-0">
            <img src="/shodh_logo_nobackground.svg" alt="Shodh" className="w-9 h-9 shrink-0" />
            <div className="flex flex-col leading-tight">
              <span
                className="font-bold text-lg tracking-tight"
                style={{ color: colors.text }}
              >
                SHODH
              </span>
              <span className="text-[10px]" style={{ color: colors.textMuted }}>
                (शोध)
              </span>
            </div>
          </div>
        )}
        <button
          onClick={toggleSidebar}
          className="w-7 h-7 rounded-md flex items-center justify-center transition-colors shrink-0"
          style={{ color: colors.textTertiary }}
          title={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
        >
          {collapsed ? <PanelLeft className="w-4 h-4" /> : <PanelLeftClose className="w-4 h-4" />}
        </button>
      </div>

      {/* Cmd+K trigger */}
      {!collapsed && (
        <button
          onClick={onOpenCommandPalette}
          className="mx-3 mt-3 flex items-center gap-2 px-2.5 py-1.5 rounded-md border text-xs transition-colors"
          style={{
            borderColor: colors.border,
            color: colors.textMuted,
            backgroundColor: colors.bg,
          }}
        >
          <Search className="w-3 h-3" />
          <span className="flex-1 text-left">Search...</span>
          <kbd
            className="text-[10px] px-1 py-0.5 rounded border font-mono"
            style={{ borderColor: colors.border, color: colors.textMuted }}
          >
            {navigator.platform.includes('Mac') ? '⌘K' : 'Ctrl+K'}
          </kbd>
        </button>
      )}

      {/* Navigation */}
      <nav className="px-2 mt-3">
        {navItems.map(item => {
          const Icon = item.icon;
          const isActive = activeView === item.id;

          return (
            <button
              key={item.id}
              onClick={() => onViewChange(item.id)}
              className="w-full flex items-center gap-2.5 px-2.5 py-1.5 rounded-md mb-0.5 transition-all duration-150"
              style={{
                backgroundColor: isActive ? `${colors.primary}14` : 'transparent',
                borderLeft: isActive ? `2px solid ${colors.primary}` : '2px solid transparent',
                color: isActive ? colors.text : colors.textTertiary,
              }}
              title={collapsed ? item.label : undefined}
            >
              <Icon className="w-4 h-4 shrink-0" style={{ color: isActive ? colors.primary : colors.textTertiary }} />
              {!collapsed && (
                <span className="text-xs font-medium">{item.label}</span>
              )}
            </button>
          );
        })}
      </nav>

      <div className="mx-3 my-2 border-t" style={{ borderColor: colors.border }} />

      {/* Scrollable content area */}
      {!collapsed && (
        <div className="flex-1 overflow-y-auto min-h-0">
          {/* Conversation threads */}
          <ConversationList
            conversations={conversations}
            activeConversationId={activeConversationId}
            collapsed={collapsed}
            onSelect={onSelectConversation}
            onNew={onNewConversation}
            onDelete={onDeleteConversation}
            onRename={onRenameConversation}
            onPin={onPinConversation}
            onReorder={onReorderConversations}
          />

          {/* Sources section */}
          <div className="mt-3 px-3 pb-3">
            <button
              onClick={() => setSourcesSectionExpanded(!sourcesSectionExpanded)}
              className="flex items-center gap-1.5 w-full mb-1.5"
            >
              {sourcesSectionExpanded ? (
                <ChevronDown className="w-3 h-3" style={{ color: colors.textMuted }} />
              ) : (
                <ChevronRight className="w-3 h-3" style={{ color: colors.textMuted }} />
              )}
              <span className="text-[10px] font-bold tracking-widest" style={{ color: colors.textMuted }}>
                SOURCES ({sources.length})
              </span>
              <div className="flex-1" />
              <button
                onClick={e => {
                  e.stopPropagation();
                  onAddSource();
                }}
                className="w-4 h-4 rounded flex items-center justify-center"
                style={{ color: colors.textTertiary }}
                title="Add source"
              >
                <Plus className="w-3 h-3" />
              </button>
            </button>

            <AnimatePresence>
              {sourcesSectionExpanded && (
                <motion.div
                  initial={{ height: 0, opacity: 0 }}
                  animate={{ height: 'auto', opacity: 1 }}
                  exit={{ height: 0, opacity: 0 }}
                  transition={{ duration: 0.15 }}
                  className="overflow-hidden"
                >
                  {sources.length === 0 ? (
                    <div className="py-3 text-center">
                      <FolderOpen className="w-5 h-5 mx-auto mb-1.5" style={{ color: colors.textMuted }} />
                      <p className="text-[10px]" style={{ color: colors.textMuted }}>
                        No sources added
                      </p>
                    </div>
                  ) : (
                    <div className="space-y-1.5">
                      {sources.map(source => (
                        <div key={source.id}>
                          {/* Source item row */}
                          <div
                            className="flex items-start gap-2 px-2 py-2 rounded-lg cursor-pointer transition-colors group"
                            style={{
                              backgroundColor: source.selected ? `${sourceColor(source.name)}08` : 'transparent',
                              border: `1px solid ${source.selected ? sourceColor(source.name) : 'transparent'}`,
                            }}
                            onClick={() => onToggleSource(source.id)}
                          >
                            {/* Checkbox with source-specific color */}
                            <div
                              className="w-4 h-4 rounded flex items-center justify-center shrink-0 mt-0.5"
                              style={{
                                border: source.selected ? 'none' : `1px solid ${colors.border}`,
                                backgroundColor: source.selected ? sourceColor(source.name) : 'transparent',
                              }}
                            >
                              {source.selected && <Check className="w-2.5 h-2.5 text-white" />}
                            </div>

                            {/* Source info */}
                            <div className="flex-1 min-w-0">
                              <div className="flex items-center gap-1.5">
                                <FileText className="w-3.5 h-3.5 shrink-0" style={{ color: sourceColor(source.name) }} />
                                <span className="text-[11px] font-semibold truncate" style={{ color: colors.text }}>
                                  {source.name}
                                </span>
                              </div>
                              <div className="flex items-center gap-2 mt-0.5 text-[10px]" style={{ color: colors.textMuted }}>
                                {/* Expand/collapse file list */}
                                {source.status === 'ready' && (
                                  <button
                                    onClick={(e) => onToggleSourceExpansion(source.id, e)}
                                    className="flex items-center gap-0.5 hover:opacity-70 transition-opacity"
                                    title={expandedSources.has(source.id) ? 'Hide files' : 'Show files'}
                                  >
                                    {expandedSources.has(source.id) ? (
                                      <ChevronUp className="w-3 h-3" style={{ color: colors.textSecondary }} />
                                    ) : (
                                      <ChevronDown className="w-3 h-3" style={{ color: colors.textSecondary }} />
                                    )}
                                  </button>
                                )}
                                <span style={{ color: source.status === 'ready' ? colors.success : colors.textMuted }}>
                                  {source.fileCount || 0} files
                                </span>
                                {source.status === 'indexing' && source.progress != null && (
                                  <span className="text-yellow-500">{source.progress}%</span>
                                )}
                                {source.status === 'ready' && source.indexedAt && (
                                  <span>{new Date(source.indexedAt).toLocaleDateString()}</span>
                                )}
                              </div>
                              {source.path && (
                                <p className="text-[10px] truncate mt-0.5" style={{ color: colors.textMuted }} title={source.path}>
                                  {source.path}
                                </p>
                              )}
                            </div>

                            {/* Right side: progress / delete */}
                            <div className="flex items-center gap-1 shrink-0">
                              {source.status === 'indexing' && (
                                <div className="flex flex-col items-end gap-0.5">
                                  <Loader2 className="w-3 h-3 animate-spin text-yellow-500" />
                                  {source.progress != null && (
                                    <div className="w-10 rounded-full h-1 overflow-hidden" style={{ backgroundColor: colors.bgTertiary }}>
                                      <div
                                        className="h-full bg-yellow-500 transition-all duration-300"
                                        style={{ width: `${source.progress}%` }}
                                      />
                                    </div>
                                  )}
                                </div>
                              )}
                              <button
                                className="opacity-0 group-hover:opacity-100 p-1 rounded transition-all"
                                style={{ color: colors.error }}
                                onClick={(e) => onRemoveSource(source.id, e)}
                                title="Remove source"
                              >
                                <Trash2 className="w-3 h-3" />
                              </button>
                            </div>
                          </div>

                          {/* Expanded file list */}
                          <AnimatePresence>
                            {expandedSources.has(source.id) && (
                              <motion.div
                                initial={{ height: 0, opacity: 0 }}
                                animate={{ height: 'auto', opacity: 1 }}
                                exit={{ height: 0, opacity: 0 }}
                                transition={{ duration: 0.2 }}
                                className="overflow-hidden ml-6 mt-1"
                              >
                                <div
                                  className="space-y-0.5 overflow-y-auto rounded-md p-1"
                                  style={{ maxHeight: '200px', backgroundColor: colors.bgTertiary }}
                                >
                                  {sourceFiles[source.id] ? (
                                    sourceFiles[source.id].length > 0 ? (
                                      sourceFiles[source.id].map((file: any, fileIdx: number) => {
                                        const { Icon, color, badge } = getFileIconInfo(file.file_type);
                                        return (
                                          <div
                                            key={fileIdx}
                                            className="flex items-center gap-1.5 px-2 py-1 text-[10px] rounded transition-colors"
                                            style={{ color: colors.textSecondary }}
                                            onMouseEnter={e => (e.currentTarget.style.backgroundColor = `${colors.primary}10`)}
                                            onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
                                          >
                                            <Icon className="w-3 h-3 shrink-0" style={{ color }} />
                                            <span
                                              className="text-[8px] font-bold px-1 rounded shrink-0"
                                              style={{ backgroundColor: `${color}20`, color, minWidth: '18px', textAlign: 'center' }}
                                            >
                                              {badge}
                                            </span>
                                            <span className="flex-1 truncate" title={file.file_path || file.name}>
                                              {file.name || file.file_path?.split(/[\\/]/).pop() || `File ${fileIdx + 1}`}
                                            </span>
                                            {file.status === 'indexed' && (
                                              <Check className="w-2.5 h-2.5 shrink-0" style={{ color: colors.success }} />
                                            )}
                                            {file.status === 'failed' && (
                                              <AlertTriangle className="w-2.5 h-2.5 shrink-0" style={{ color: colors.error }} />
                                            )}
                                          </div>
                                        );
                                      })
                                    ) : (
                                      <div className="p-2 text-center text-[10px]" style={{ color: colors.textMuted }}>
                                        No files indexed
                                      </div>
                                    )
                                  ) : (
                                    <div className="p-2 flex items-center justify-center gap-1.5">
                                      <Loader2 className="w-3 h-3 animate-spin" style={{ color: colors.primary }} />
                                      <span className="text-[10px]" style={{ color: colors.textMuted }}>Loading files...</span>
                                    </div>
                                  )}
                                </div>
                              </motion.div>
                            )}
                          </AnimatePresence>
                        </div>
                      ))}
                    </div>
                  )}
                </motion.div>
              )}
            </AnimatePresence>
          </div>
        </div>
      )}

      {collapsed && (
        <div className="flex-1 overflow-y-auto">
          <ConversationList
            conversations={conversations}
            activeConversationId={activeConversationId}
            collapsed={collapsed}
            onSelect={onSelectConversation}
            onNew={onNewConversation}
            onDelete={onDeleteConversation}
            onRename={onRenameConversation}
            onPin={onPinConversation}
          />
        </div>
      )}

      {/* Footer */}
      <div className="border-t px-2 py-2" style={{ borderColor: colors.border }}>
        {collapsed ? (
          <div className="flex flex-col items-center gap-1">
            <button
              onClick={onOpenLLMSettings}
              className="w-8 h-8 rounded-md flex items-center justify-center transition-colors"
              style={{ color: llmStatus.connected ? colors.success : colors.warning }}
              title={llmStatus.connected ? llmStatus.model : 'Configure AI'}
            >
              <Bot className="w-4 h-4" />
            </button>
            <button
              onClick={onShowFeedback}
              className="w-8 h-8 rounded-md flex items-center justify-center transition-colors"
              style={{ color: colors.textMuted }}
              title="Report Bug"
            >
              <Bug className="w-3.5 h-3.5" />
            </button>
            <ThemeToggle />
          </div>
        ) : (
          <div className="space-y-2">
            <button
              onClick={onOpenLLMSettings}
              className="w-full flex items-center gap-2 px-2 py-1.5 rounded-md transition-colors text-left"
              style={{ color: colors.textSecondary }}
            >
              <div
                className="w-2 h-2 rounded-full shrink-0"
                style={{ backgroundColor: llmStatus.connected ? colors.success : colors.warning }}
              />
              <span className="text-[11px] font-medium truncate flex-1">
                {llmStatus.connected ? llmStatus.model : 'Configure AI Model'}
              </span>
              <span className="text-[10px]" style={{ color: colors.textMuted }}>
                {stats.totalDocs} docs
              </span>
            </button>

            <div className="flex items-center justify-between px-1">
              <button
                onClick={onShowFeedback}
                className="flex items-center gap-1.5 text-[10px] font-medium transition-colors"
                style={{ color: colors.textMuted }}
              >
                <Bug className="w-3 h-3" />
                Feedback
              </button>
              <ThemeToggle />
            </div>
          </div>
        )}
      </div>
    </motion.div>
  );
}

import React, { useState, useEffect, useRef, useCallback, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { readTextFile, writeTextFile } from "@tauri-apps/plugin-fs";
import { motion, AnimatePresence } from "framer-motion";
import { StructuredOutputRenderer } from "./components/StructuredOutput";
import { CitationFootnotes } from "./components/CitationFootnotes";
import { CitationBadge } from "./components/CitationBadge";
import { ToolCallBubble } from "./components/ToolCallBubble";

// UI Components
import { Button } from "./components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "./components/ui/card";
import { Badge } from "./components/ui/badge";
import { Input } from "./components/ui/input";
import { Progress } from "./components/ui/progress";
// Tabs removed — navigation moved to AppSidebar
import {
  Search, MessageSquare, Sparkles, Settings, Bot,
  FolderOpen, FileText, Code, Terminal, ChevronRight,
  Plus, RefreshCw, Check, X, AlertCircle, Loader2, Pencil,
  Send, Copy, Download, Save, ChevronDown, ChevronUp,
  Database, Cpu, HardDrive, Activity, FileCode, GitBranch,
  BookOpen, TestTube, FileJson, Network, Layers, Package,
  FileSpreadsheet, Presentation, FileDown, FilePlus, BarChart, Trash2, Clock,
  Shield, SearchCheck, Briefcase, Heart, FileCheck, AlertTriangle, TrendingUp,
  Braces, Coffee, Table, Brain, Zap, Globe, Image as ImageIcon
} from 'lucide-react';

// Import LLM Settings and core components
import LLMSettings from './LLMSettings';
import { ImageUpload } from './components/ImageUpload';
import DocumentGenerator from './components/DocumentGenerator';
import { ThemeToggle } from './components/ThemeToggle';
import AppSidebar from './components/AppSidebar';
import type { ViewTab } from './components/AppSidebar';
import { useTheme } from './contexts/ThemeContext';
import { useSidebar } from './contexts/SidebarContext';
import { useConversations } from './hooks/useConversations';
import { useCommandPalette } from './hooks/useCommandPalette';
import CommandPalette from './components/CommandPalette';
import DocumentPreviewPanel from './components/DocumentPreviewPanel';
import AnalyticsDashboard from './components/AnalyticsDashboard';
import KnowledgeGraph from './components/KnowledgeGraph';
import AgentsPanel from './components/AgentsPanel';
import CalendarTodoPanel from './components/CalendarTodoPanel';
import SearchSettings, { useSearchConfig } from './components/SearchSettings';
import { useActivityTracker } from './hooks/useActivityTracker';
import { OnboardingFlow } from './components/OnboardingFlow';
import { FeedbackDialog } from './components/FeedbackDialog';
import { LoadingState } from './components/LoadingState';
import { EmptyState } from './components/EmptyState';
import { UpdateNotification } from './components/UpdateNotification';
import { Bug, Mic, MicOff } from 'lucide-react';
import { toast } from 'sonner';
import { notify, setNotificationHandler } from './lib/notify';
import { useNotifications } from './hooks/useNotifications';
import NotificationCenter from './components/NotificationCenter';
import { IntegrationsPanel } from './components/IntegrationsPanel';
import { EnhancedArtifactPanel } from './components/EnhancedArtifactPanel';
import { ArtifactPreviewCard } from './components/ArtifactPreviewCard';
import { ChartArtifact } from './components/ChartArtifact';
import { TableArtifact } from './components/TableArtifact';
import { intelligentSearch, trackUserMessage, trackAssistantMessage } from './utils/intelligentRetrieval';
import { sourceColor } from './utils/colors';
import { parseResponseWithCitations } from './utils/citationParser';
import { StreamingArtifactExtractor, stripChartContent, extractArtifacts } from './utils/artifactExtractor';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { oneDark } from 'react-syntax-highlighter/dist/esm/styles/prism';
import { oneLight } from 'react-syntax-highlighter/dist/esm/styles/prism';

// Debug logging — set to true during development, false for demo/production
const DEBUG = false;
const debugLog = (...args: any[]) => { if (DEBUG) console.log(...args); };

// Types
interface OutputFormat {
  id: string;
  name: string;
  icon: any;
  extension: string;
  mimeType: string;
}

interface Source {
  id: string;
  name: string;
  path: string;
  type: 'documents';
  fileCount: number;
  indexedAt: string;
  status: 'ready' | 'indexing' | 'error';
  selected: boolean;
  language?: string;
  size?: string;
  progress?: number;
  currentFile?: string;
  processedCount?: number;
}

interface Message {
  id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  timestamp: string;
  sources?: Array<{ file: string; score: number }>;
  searchResults?: any[]; // Full search results for citation parsing
  generationType?: 'chat' | 'code' | 'docs' | 'test';
  image?: string; // Base64 image data for displaying images
  platform?: string; // Platform where the message originated (telegram, discord, etc.)
  artifacts?: any[]; // Artifacts embedded in this message
  toolInvocations?: Array<{
    tool_name: string;
    arguments: Record<string, any>;
    result: string;
    success: boolean;
    duration_ms: number;
    status: 'pending' | 'running' | 'completed' | 'failed';
  }>;
}

interface GenerationTemplate {
  id: string;
  name: string;
  icon: any;
  prompt: string;
  category: 'code' | 'docs' | 'analysis';
}

// Reusable copy button with copied state feedback
function CopyButton({ text, isDark, label = 'Copy', size = 'sm' }: { text: string; isDark: boolean; label?: string; size?: 'sm' | 'md' }) {
  const [copied, setCopied] = useState(false);
  const handleCopy = (e: React.MouseEvent) => {
    e.stopPropagation();
    navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };
  const iconSize = size === 'sm' ? 'w-3 h-3' : 'w-3.5 h-3.5';
  const fontSize = size === 'sm' ? 'text-[10px]' : 'text-[11px]';
  return (
    <button
      onClick={handleCopy}
      className={`flex items-center gap-1 ${fontSize} px-1.5 py-0.5 rounded transition-all`}
      style={{ color: copied ? '#10b981' : (isDark ? '#9ca3af' : '#6b7280') }}
      onMouseEnter={e => (e.currentTarget.style.backgroundColor = isDark ? 'rgba(255,255,255,0.08)' : 'rgba(0,0,0,0.06)')}
      onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
    >
      {copied ? <Check className={iconSize} /> : <Copy className={iconSize} />}
      <span>{copied ? 'Copied' : label}</span>
    </button>
  );
}

// Helper component to render message content with structured outputs
function MessageContentRenderer({ content, searchResults, artifacts, onFollowUpQuery, onOpenUrl, onViewInArtifact, onOpenArtifact, textColor }: {
  content: string;
  searchResults?: any[];
  artifacts?: any[];
  onFollowUpQuery?: (query: string) => void;
  onOpenUrl?: (url: string) => void;
  onViewInArtifact?: (citation: any) => void;
  onOpenArtifact?: (artifactId: string) => void;
  textColor: string;
}) {
  const { theme, colors } = useTheme();
  const [structuredOutputs, setStructuredOutputs] = useState<any[] | null>(null);
  const [loading, setLoading] = useState(false);

  // Structured output parsing disabled — artifacts are extracted by the backend
  // and rendered in the artifact panel. Using StructuredOutputRenderer here
  // was bypassing ReactMarkdown and breaking citation rendering.
  useEffect(() => {}, [content]);

  // Citation placeholder: safe ASCII markers that survive markdown parsing
  const CITE_OPEN = 'XCSHODH';
  const CITE_CLOSE = 'XESHODH';

  // Build citation info for footnotes — must be called before any early returns
  const citationInfo = React.useMemo(() => {
    if (!searchResults || searchResults.length === 0) return [];

    const seen = new Set<string>();
    const citations: any[] = [];
    let citationNumber = 0;

    searchResults.forEach((result: any) => {
      const snippetHash = result.snippet?.substring(0, 50) || '';
      const location = result.pageNumber
        ? `page-${result.pageNumber}`
        : result.lineRange
        ? `line-${result.lineRange[0]}-${result.lineRange[1]}`
        : `text-${snippetHash}`;
      const uniqueKey = `${result.sourceFile}-${location}`;

      if (seen.has(uniqueKey)) return;
      seen.add(uniqueKey);

      citationNumber++;

      citations.push({
        number: citationNumber,
        title: result.citation?.title || result.sourceFile.split(/[/\\]/).pop() || 'Untitled',
        authors: result.citation?.authors || [],
        source: result.citation?.source || result.sourceFile,
        year: result.citation?.year || '',
        url: result.citation?.url,
        sourceFile: result.sourceFile,
        pageNumber: result.pageNumber,
        lineRange: result.lineRange,
        snippet: result.snippet || result.text?.substring(0, 150) || '',
      });
    });

    return citations;
  }, [searchResults]);

  // Pre-process content for markdown rendering:
  // 1. Strip artifact code blocks (```chart, ```table, ```mermaid etc.) — already extracted as artifacts
  // 2. Collapse orphan citation-only lines into previous line
  // 3. Replace [N] citation markers with safe placeholders (but NOT inside code blocks)
  // Must be called before early returns (React hooks rule)
  const preprocessed = React.useMemo(() => {
    let text = content;

    // Step 1: Strip artifact blocks handled by the artifact panel.
    // Fenced blocks: ```chart, ```table, ```mermaid, etc.
    text = text.replace(/```(?:chart|table|mermaid|flowchart|sequence|classDiagram|erDiagram|stateDiagram|gantt|gitGraph|journey|form|action)\s*\n[\s\S]*?```/g, '');

    // Step 1b: Strip all inline chart JSON (with/without "chart" prefix, fenced or not)
    text = stripChartContent(text);

    // Step 2: Protect real code blocks from citation replacement.
    // Extract all ```...``` blocks, replace with placeholders, do citation work, then restore.
    const codeBlocks: string[] = [];
    text = text.replace(/```[\s\S]*?```/g, (match) => {
      codeBlocks.push(match);
      return `\x01CODE${codeBlocks.length - 1}\x01`;
    });

    // Step 3: Collapse lines that contain ONLY citation markers into the previous line.
    const lines = text.split('\n');
    const merged: string[] = [];
    for (let i = 0; i < lines.length; i++) {
      const trimmed = lines[i].trim();
      if (/^(\[(?:Document\s+)?\d+(?:\s*,\s*(?:Document\s+)?\d+)*\]\s*)+$/.test(trimmed) && merged.length > 0) {
        merged[merged.length - 1] = merged[merged.length - 1].trimEnd() + ' ' + trimmed;
      } else {
        merged.push(lines[i]);
      }
    }
    text = merged.join('\n');

    // Step 4: Replace citation markers with safe placeholders (outside code blocks)
    text = text.replace(/【(\d+)†[^】]*】/g, `${CITE_OPEN}$1${CITE_CLOSE}`);
    text = text.replace(/\[(?:Document\s+)?(\d+(?:\s*,\s*(?:Document\s+)?\d+)*)\]/gi, (_, nums) => {
      return nums.split(',').map((n: string) => `${CITE_OPEN}${n.replace(/Document\s+/gi, '').trim()}${CITE_CLOSE}`).join('');
    });

    // Step 5: Restore code blocks
    text = text.replace(/\x01CODE(\d+)\x01/g, (_, idx) => codeBlocks[parseInt(idx)]);

    // Clean up excessive blank lines left from stripped blocks
    text = text.replace(/\n{3,}/g, '\n\n');

    return text;
  }, [content]);

  if (loading) {
    return <div className="text-sm" style={{ color: textColor }}>Rendering...</div>;
  }

  // If we have structured outputs, render them with citations
  if (structuredOutputs && structuredOutputs.length > 0) {
    return <StructuredOutputRenderer outputs={structuredOutputs} searchResults={searchResults} onFollowUpQuery={onFollowUpQuery} onOpenUrl={onOpenUrl} />;
  }

  // Debug: log preprocessed content and searchResults availability
  if (DEBUG && preprocessed.includes('XCSHODH')) {
    console.log('🔍 CITATION DEBUG:', {
      hasSearchResults: !!searchResults,
      searchResultsLength: searchResults?.length || 0,
      firstPlaceholder: preprocessed.match(/XCSHODH\d+XESHODH/)?.[0],
      contentPreview: preprocessed.substring(0, 200),
    });
  }

  // Render a string that may contain XCSHODH{N}XESHODH citation placeholders into React nodes
  const renderWithCitations = (text: string): React.ReactNode => {
    if (!searchResults || searchResults.length === 0) {
      // Strip placeholders if no search results available
      return text.replace(new RegExp(`${CITE_OPEN}\\d+${CITE_CLOSE}`, 'g'), '');
    }

    const parts: React.ReactNode[] = [];
    const pattern = new RegExp(`${CITE_OPEN}(\\d+)${CITE_CLOSE}`, 'g');
    let lastIdx = 0;
    let match;

    while ((match = pattern.exec(text)) !== null) {
      if (match.index > lastIdx) {
        parts.push(text.substring(lastIdx, match.index));
      }
      const num = parseInt(match[1]);
      const result = searchResults[num - 1];
      if (result) {
        parts.push(
          <CitationBadge
            key={`cite-${match.index}-${num}`}
            citation={{
              id: result.id || `sr-${num}`,
              sourceFile: result.sourceFile,
              pageNumber: result.pageNumber,
              snippet: result.snippet || '',
              surroundingContext: result.surroundingContext || '',
              citationTitle: result.citation?.title || result.sourceFile?.split(/[/\\]/).pop() || 'Source',
            }}
            index={num}
            onFollowUpQuery={onFollowUpQuery}
            onOpenUrl={onOpenUrl}
            onViewInArtifact={onViewInArtifact}
          />
        );
      } else {
        parts.push(`[${num}]`);
      }
      lastIdx = match.index + match[0].length;
    }

    if (lastIdx < text.length) {
      parts.push(text.substring(lastIdx));
    }

    return parts.length > 0 ? <>{parts}</> : text;
  };

  // Recursively walk React children, replacing citation placeholders in every text node
  const processChildren = (children: React.ReactNode): React.ReactNode => {
    return React.Children.map(children, (child) => {
      if (typeof child === 'string') return renderWithCitations(child);
      if (typeof child === 'number') return child;
      if (React.isValidElement(child) && child.props?.children) {
        return React.cloneElement(child, {}, processChildren(child.props.children));
      }
      return child;
    });
  };

  const isDark = theme === 'dark';
  const accent = 'rgba(255, 107, 53,';

  // Custom ReactMarkdown components — memoized to avoid new object reference each render
  const markdownComponents: Record<string, React.FC<any>> = React.useMemo(() => ({
    h1: ({ children }) => (
      <h1 className="text-lg font-bold mt-5 mb-2 pb-1.5 border-b"
        style={{ borderBottomColor: `${accent} ${isDark ? '0.2' : '0.15'})`, color: colors.text }}>
        {processChildren(children)}
      </h1>
    ),
    h2: ({ children }) => (
      <h2 className="text-[15px] font-bold mt-4 mb-2 pb-1 border-b"
        style={{ borderBottomColor: `${accent} ${isDark ? '0.15' : '0.1'})`, color: colors.text }}>
        {processChildren(children)}
      </h2>
    ),
    h3: ({ children }) => (
      <h3 className="text-[14px] font-semibold mt-3 mb-1.5" style={{ color: colors.text }}>
        {processChildren(children)}
      </h3>
    ),
    h4: ({ children }) => (
      <h4 className="text-[13px] font-semibold mt-2.5 mb-1" style={{ color: colors.textMuted }}>
        {processChildren(children)}
      </h4>
    ),
    p: ({ children }) => (
      <p className="mb-2 leading-relaxed">{processChildren(children)}</p>
    ),
    ul: ({ children }) => (
      <ul className="my-1.5 ml-4 space-y-0.5 list-disc" style={{ color: colors.text }}>{children}</ul>
    ),
    ol: ({ children }) => (
      <ol className="my-1.5 ml-4 space-y-0.5 list-decimal" style={{ color: colors.text }}>{children}</ol>
    ),
    li: ({ children }) => (
      <li className="pl-1 py-0.5 leading-relaxed">{processChildren(children)}</li>
    ),
    strong: ({ children }) => (
      <strong className="font-semibold" style={{ color: colors.text }}>{processChildren(children)}</strong>
    ),
    em: ({ children }) => <em className="italic">{processChildren(children)}</em>,
    a: ({ href, children }) => (
      <a href={href} target="_blank" rel="noopener noreferrer"
        className="text-blue-500 hover:text-blue-600 underline decoration-blue-300/50 hover:decoration-blue-400 transition-colors">
        {children}
      </a>
    ),
    pre: ({ children }) => (
      <div className="my-3 rounded-lg overflow-hidden" style={{ border: `1px solid ${isDark ? '#2d3748' : '#e2e8f0'}` }}>
        {children}
      </div>
    ),
    code: ({ children, className }) => {
      const match = /language-(\w+)/.exec(className || '');
      if (match) {
        const codeString = String(children).replace(/\n$/, '');
        const lang = match[1];
        return (
          <div className="relative group">
            {/* Language badge + copy button bar */}
            <div className="flex items-center justify-between px-3 py-1.5" style={{ background: isDark ? '#0f0f1e' : '#e9ecef' }}>
              <span className="text-[10px] font-mono font-medium uppercase tracking-wider" style={{ color: isDark ? '#6b7280' : '#9ca3af' }}>
                {lang}
              </span>
              <CopyButton text={codeString} isDark={isDark} />
            </div>
            <SyntaxHighlighter
              style={isDark ? oneDark : oneLight}
              language={lang}
              PreTag="div"
              customStyle={{
                margin: 0,
                padding: '1rem',
                fontSize: '0.75rem',
                lineHeight: '1.6',
                borderRadius: 0,
                background: isDark ? '#141422' : '#f1f3f5',
              }}
            >
              {codeString}
            </SyntaxHighlighter>
          </div>
        );
      }
      return (
        <code className="px-1.5 py-0.5 rounded text-xs font-mono"
          style={{ backgroundColor: isDark ? '#2d3748' : '#edf2f7', color: isDark ? '#fbd38d' : '#c53030' }}>
          {children}
        </code>
      );
    },
    blockquote: ({ children }) => (
      <blockquote className="my-3 pl-4 py-2 border-l-4 italic"
        style={{
          borderLeftColor: isDark ? '#f59e0b' : '#d97706',
          backgroundColor: isDark ? 'rgba(245,158,11,0.05)' : 'rgba(217,119,6,0.05)',
          color: isDark ? '#d1d5db' : '#4b5563',
        }}>
        {children}
      </blockquote>
    ),
    table: ({ children }) => (
      <div className="my-3 overflow-x-auto rounded-lg" style={{ border: `1px solid ${isDark ? '#374151' : '#e5e7eb'}` }}>
        <table className="w-full text-xs border-collapse">{children}</table>
      </div>
    ),
    thead: ({ children }) => (
      <thead style={{ backgroundColor: isDark ? '#1f2937' : '#f9fafb' }}>{children}</thead>
    ),
    th: ({ children }) => (
      <th className="px-3 py-2 text-left font-semibold border-b"
        style={{ borderColor: isDark ? '#374151' : '#e5e7eb', color: isDark ? '#e5e7eb' : '#111827' }}>
        {children}
      </th>
    ),
    td: ({ children }) => (
      <td className="px-3 py-2 border-b"
        style={{ borderColor: isDark ? '#1f2937' : '#f3f4f6', color: isDark ? '#d1d5db' : '#374151' }}>
        {processChildren(children)}
      </td>
    ),
    tr: ({ children }) => <tr className="hover:bg-gray-50 dark:hover:bg-gray-800/50 transition-colors">{children}</tr>,
    hr: () => <hr className="my-4" style={{ borderColor: isDark ? '#374151' : '#e5e7eb' }} />,
  }), [theme, isDark, accent, colors, processChildren]);

  // Classify artifacts for inline rendering
  const chartArtifacts = (artifacts || []).filter((a: any) => {
    const t = typeof a.artifact_type === 'string' ? a.artifact_type.toLowerCase() : '';
    return t === 'chart' || a.artifact_type?.Chart !== undefined;
  });
  const tableArtifacts = (artifacts || []).filter((a: any) => {
    const t = typeof a.artifact_type === 'string' ? a.artifact_type.toLowerCase() : '';
    return t === 'table' || a.artifact_type?.Table !== undefined;
  });
  const otherArtifacts = (artifacts || []).filter((a: any) => {
    const t = typeof a.artifact_type === 'string' ? a.artifact_type.toLowerCase() : '';
    return t !== 'chart' && t !== 'table' && !a.artifact_type?.Chart && !a.artifact_type?.Table;
  });

  return (
    <div>
      {/* Markdown content */}
      <div
        className="text-[13px] leading-relaxed"
        style={{
          color: textColor,
          fontFamily: '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif',
        }}
      >
        <ReactMarkdown remarkPlugins={[remarkGfm]} components={markdownComponents}>
          {preprocessed}
        </ReactMarkdown>
      </div>

      {/* Inline charts — rendered directly, no click required */}
      {chartArtifacts.length > 0 && (
        <div className="mt-3 space-y-3">
          {chartArtifacts.map((artifact: any) => (
            <div key={artifact.id} className="rounded-lg overflow-hidden border"
              style={{ borderColor: isDark ? '#374151' : '#e5e7eb' }}>
              <ChartArtifact artifact={artifact} theme={theme} />
            </div>
          ))}
        </div>
      )}

      {/* Inline tables — rendered directly, no click required */}
      {tableArtifacts.length > 0 && (
        <div className="mt-3 space-y-3">
          {tableArtifacts.map((artifact: any) => (
            <div key={artifact.id} className="rounded-lg overflow-hidden border"
              style={{ borderColor: isDark ? '#374151' : '#e5e7eb' }}>
              <TableArtifact artifact={artifact} theme={theme} />
            </div>
          ))}
        </div>
      )}

      {/* Other artifacts as preview cards */}
      {otherArtifacts.length > 0 && (
        <div className="mt-3 space-y-2">
          {otherArtifacts.map((artifact: any) => (
            <ArtifactPreviewCard key={artifact.id} artifact={artifact} onClick={() => onOpenArtifact?.(artifact.id)} />
          ))}
        </div>
      )}

      {/* Citation Footnotes — always last */}
      {citationInfo.length > 0 && (
        <CitationFootnotes citations={citationInfo} onViewInArtifact={onViewInArtifact} />
      )}
    </div>
  );
}

function AppSplitView() {
  // Theme
  const { theme, colors, toggleTheme } = useTheme();
  const { collapsed } = useSidebar();
  const { config: searchConfig, updateConfig: updateSearchConfig, resetConfig: resetSearchConfig } = useSearchConfig();

  // Conversations
  const {
    conversations,
    activeConversationId,
    activeConversation,
    createConversation,
    switchConversation,
    updateActiveMessages,
    appendMessage: appendConvMessage,
    renameConversation,
    deleteConversation,
    pinConversation,
    updateConversationMeta,
    reorderConversations,
  } = useConversations();

  // Notification center
  const {
    notifications,
    unreadCount,
    add: addNotification,
    markRead: markNotifRead,
    markAllRead: markAllNotifsRead,
    remove: removeNotif,
    clearAll: clearAllNotifs,
  } = useNotifications();

  // Wire notification handler so notify.success() etc. push to bell
  useEffect(() => {
    setNotificationHandler(addNotification);
    return () => setNotificationHandler(null);
  }, [addNotification]);

  // Command palette
  const { open: cmdPaletteOpen, openPalette, closePalette } = useCommandPalette();

  // Activity Tracker
  const { trackActivity } = useActivityTracker();

  // Core state
  const [isLoading, setIsLoading] = useState(true);
  const [isFirstTime, setIsFirstTime] = useState(false);
  const [activeTab, setActiveTab] = useState<ViewTab>('chat');
  const [activeAgentId, setActiveAgentId] = useState<string | null>(null);
  const [sources, setSources] = useState<Source[]>([]);
  const [spaces, setSpaces] = useState<Array<{ id: string; name: string; }>>([]);
  const [isTelegramBotActive, setIsTelegramBotActive] = useState(false);
  const [expandedSources, setExpandedSources] = useState<Set<string>>(new Set());
  const [docsExpandedSources, setDocsExpandedSources] = useState<Set<string>>(new Set());
  const [sourceFiles, setSourceFiles] = useState<Record<string, any[]>>({});
  const [currentlyIndexing, setCurrentlyIndexing] = useState<string | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [artifacts, setArtifacts] = useState<any[]>([]);
  const [showArtifacts, setShowArtifacts] = useState(false);
  const [selectedArtifactId, setSelectedArtifactId] = useState<string | undefined>(undefined);
  const [currentInput, setCurrentInput] = useState("");
  const [isProcessing, setIsProcessing] = useState(false);
  const [isDraggingImage, setIsDraggingImage] = useState(false);
  const lastProcessedImageTimeRef = useRef(0);
  const chatEndRef = useRef<HTMLDivElement>(null);
  const chatScrollContainerRef = useRef<HTMLDivElement>(null);
  const artifactExtractorRef = useRef(new StreamingArtifactExtractor());
  const streamingCleanupRef = useRef<(() => void) | null>(null);
  const currentOperationAbortRef = useRef<(() => void) | null>(null); // For cancelling unified_chat

  // Typewriter buffer for chat streaming — stores the full accumulated text from backend
  // while the UI reveals it gradually, word-by-word
  const streamTargetRef = useRef('');
  const streamDisplayedRef = useRef(0); // char index up to which we've revealed
  const streamTimerRef = useRef<number | null>(null);
  const prevMessageCountRef = useRef(0); // track message count to detect new messages vs content updates
  const lastSyncedConvIdRef = useRef<string | null>(null);

  // Sync messages from conversation store when active conversation changes
  useEffect(() => {
    if (!activeConversationId || activeConversationId === lastSyncedConvIdRef.current) return;
    lastSyncedConvIdRef.current = activeConversationId;

    if (activeConversation && activeConversation.messages.length > 0) {
      // Convert ConversationMessage[] → Message[]
      const loaded: Message[] = activeConversation.messages.map(m => ({
        id: m.id,
        role: m.role as 'user' | 'assistant' | 'system',
        content: m.content,
        timestamp: m.timestamp,
        artifacts: m.artifacts,
        searchResults: m.searchResults,
      }));
      setMessages(loaded);
    } else {
      setMessages([]);
    }
    setArtifacts([]);
    setShowArtifacts(false);
  }, [activeConversationId, activeConversation]);

  // Sync local messages back to conversation store
  // Triggers: new message added (length change) or streaming finalized (id change on last msg)
  const syncConversationMessages = React.useCallback(() => {
    if (messages.length === 0) return;
    updateActiveMessages(() =>
      messages
        .filter(m => !m.id.startsWith('streaming-'))
        .map(m => ({
          id: m.id,
          role: m.role,
          content: m.content,
          timestamp: m.timestamp,
          artifacts: m.artifacts,
          searchResults: m.searchResults,
        }))
    );
  }, [messages, updateActiveMessages]);

  const prevSyncKeyRef = useRef('');
  useEffect(() => {
    // Build a sync key from message count + last message id to detect finalizations
    const lastId = messages.length > 0 ? messages[messages.length - 1].id : '';
    const syncKey = `${messages.length}:${lastId}`;
    if (syncKey === prevSyncKeyRef.current) return;
    // Don't sync while streaming (last msg has streaming- prefix)
    if (lastId.startsWith('streaming-')) return;
    prevSyncKeyRef.current = syncKey;
    syncConversationMessages();
  }, [messages, syncConversationMessages]);

  // Onboarding & Feedback
  const [showOnboarding, setShowOnboarding] = useState(!localStorage.getItem('onboarding_completed'));
  const [showFeedback, setShowFeedback] = useState(false);

  // Search System Status
  // Ref to prevent double initialization (React Strict Mode protection)
  const initializationRef = useRef(false);
  const initializationPromiseRef = useRef<Promise<void> | null>(null);

  const [searchSystemStatus] = useState({
    bm25: {
      enabled: true,
      name: "BM25 Keyword Search",
      description: "Fast keyword matching for exact terms"
    },
    diskann: {
      enabled: true,
      name: "DiskANN Vector Search",
      description: "Semantic understanding using dense vectors"
    },
    vamana: {
      enabled: true,
      name: "Vamana Graph Search",
      description: "Graph-based navigation for related concepts"
    },
    reranking: {
      enabled: true,
      name: "Neural Reranking",
      description: "AI-powered relevance scoring with attention"
    },
    knowledgeGraph: {
      enabled: true,
      name: "Knowledge Graph",
      description: "Entity relationships and concept mapping"
    }
  });

  // LLM State
  const [llmStatus, setLlmStatus] = useState<{
    connected: boolean;
    model: string;
    provider: string;
  }>({
    connected: false,
    model: 'Not configured',
    provider: 'none'
  });
  const [showLLMSettings, setShowLLMSettings] = useState(false);

  // Document preview
  const [previewFile, setPreviewFile] = useState<{ path: string; name: string; page?: number } | null>(null);

  // Follow-up suggestions generated after each AI response
  const [followUpSuggestions, setFollowUpSuggestions] = useState<string[]>([]);
  const [isListening, setIsListening] = useState(false);
  const [showSystemPromptEditor, setShowSystemPromptEditor] = useState(false);
  const [newInstructionText, setNewInstructionText] = useState('');
  const [editingInstructionIdx, setEditingInstructionIdx] = useState<number | null>(null);
  const [editingInstructionText, setEditingInstructionText] = useState('');
  const recognitionRef = useRef<any>(null);
  const pendingSourceDeleteRef = useRef<Map<string, { timeout: ReturnType<typeof setTimeout>; source: Source }>>(new Map());

  // Derive system prompt and active space from the active conversation
  const activeSpaceId = sources.find(s => s.selected)?.id || null;
  const activeSourceName = sources.find(s => s.selected)?.name || null;
  const spaceSystemPrompt = activeConversation?.systemPrompt || '';

  // Parse instructions from newline-separated string into array
  const instructionsList = spaceSystemPrompt
    ? spaceSystemPrompt.split('\n').filter(l => l.trim())
    : [];

  // Instruction list helpers
  const saveInstructions = (lines: string[]) => {
    if (!activeConversationId) return;
    const joined = lines.filter(l => l.trim()).join('\n').trim();
    updateConversationMeta(activeConversationId, { systemPrompt: joined || undefined });
  };

  const addInstruction = () => {
    const text = newInstructionText.trim();
    if (!text) return;
    saveInstructions([...instructionsList, text]);
    setNewInstructionText('');
  };

  const removeInstruction = (idx: number) => {
    saveInstructions(instructionsList.filter((_, i) => i !== idx));
  };

  const commitEditInstruction = () => {
    if (editingInstructionIdx === null) return;
    const text = editingInstructionText.trim();
    if (text) {
      const updated = [...instructionsList];
      updated[editingInstructionIdx] = text;
      saveInstructions(updated);
    } else {
      removeInstruction(editingInstructionIdx);
    }
    setEditingInstructionIdx(null);
    setEditingInstructionText('');
  };

  // Create new conversation with current source association
  const handleNewConversation = () => {
    createConversation({
      spaceId: activeSpaceId || undefined,
      spaceName: activeSourceName || undefined,
    });
  };

  // Generate follow-up suggestions from the AI response content
  const generateFollowUps = useCallback((content: string) => {
    const suggestions: string[] = [];
    const lines = content.split('\n').filter(l => l.trim());

    // Extract key topics from headers and bold text
    const headers = lines.filter(l => /^#{1,3}\s/.test(l)).map(l => l.replace(/^#+\s*/, ''));
    const boldTerms = [...content.matchAll(/\*\*([^*]+)\*\*/g)].map(m => m[1]).filter(t => t.length > 3 && t.length < 60);

    // Strategy 1: Ask to elaborate on a header topic
    if (headers.length > 1) {
      const topic = headers[Math.min(1, headers.length - 1)];
      suggestions.push(`Tell me more about ${topic.toLowerCase()}`);
    }

    // Strategy 2: Ask to compare or contrast
    if (boldTerms.length >= 2) {
      suggestions.push(`How do ${boldTerms[0]} and ${boldTerms[1]} compare?`);
    }

    // Strategy 3: Ask for practical application
    if (content.length > 200) {
      suggestions.push('What are the practical implications of this?');
    }

    // Strategy 4: Ask for a summary if the response was long
    if (content.length > 1500) {
      suggestions.push('Can you summarize the key takeaways?');
    }

    // Strategy 5: Ask for examples
    if (!content.toLowerCase().includes('example') && content.length > 300) {
      suggestions.push('Can you provide specific examples?');
    }

    // Keep max 3 suggestions
    setFollowUpSuggestions(suggestions.slice(0, 3));
  }, []);

  // Search State
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<any[]>([]);
  const [isSearching, setIsSearching] = useState(false);

  // Search Pipeline State (for Apple-style animations)
  const [searchPipeline, setSearchPipeline] = useState<{
    stage: 'idle' | 'bm25' | 'vector' | 'neural' | 'graph' | 'complete';
    progress: number;
  }>({ stage: 'idle', progress: 0 });
  const [pipelineActive, setPipelineActive] = useState(false);

  // Generation State
  const [generationMode, setGenerationMode] = useState<'document' | 'code'>('document');
  const [generationType, setGenerationType] = useState<'report' | 'summary' | 'analysis' | 'code' | 'test' | 'custom'>('report');
  const [outputFormat, setOutputFormat] = useState<'md' | 'docx' | 'xlsx' | 'pptx' | 'pdf' | 'txt' | 'html'>('md');
  const [generationContext, setGenerationContext] = useState("");
  const [generatedContent, setGeneratedContent] = useState<any>(null);
  const [generatedPreview, setGeneratedPreview] = useState("");
  const [isGenerating, setIsGenerating] = useState(false);
  const [generationProgress, setGenerationProgress] = useState(0);
  const [selectedIndustry, setSelectedIndustry] = useState<'legal' | 'healthcare' | 'finance' | 'general'>('general');
  const [streamingSessionId, setStreamingSessionId] = useState<string | null>(null);
  const [generateInput, setGenerateInput] = useState("");

  // Stats
  const [stats, setStats] = useState({
    totalDocs: 0,
    totalChunks: 0,
    selectedDocs: 0,
    indexSize: "0 MB"
  });

  // Add workspace to sources
  const handleAddWorkspace = (path: string, name: string) => {
    const newSource: Source = {
      id: `workspace-${Date.now()}`,
      name: name,
      path: path,
      type: 'documents',
      fileCount: 0,
      indexedAt: new Date().toISOString(),
      status: 'ready',
      selected: true,
    };

    setSources(prev => {
      // Check if this path already exists
      const exists = prev.find(s => s.path === path);
      if (exists) {
        // Just select it
        return prev.map(s => s.path === path ? { ...s, selected: true } : s);
      }
      // Add new source
      return [...prev, newSource];
    });
  };

  // Open URL in system browser
  const handleOpenUrl = (url: string) => {
    window.open(url, '_blank');
  };


  // Load spaces when integrations tab opens
  useEffect(() => {
    if (activeTab === 'integrations') {
      const loadSpaces = async () => {
        try {
          const loadedSpaces = await invoke<any[]>('get_spaces');
          setSpaces(loadedSpaces.map(s => ({ id: s.id, name: s.name })));
        } catch (error) {
          console.error('Failed to load spaces:', error);
          setSpaces([]);
        }
      };
      loadSpaces();
    }
  }, [activeTab]);

  // Enhanced file drop handler - supports images AND documents
  const handleImageDrop = async (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDraggingImage(false);

    const files = Array.from(e.dataTransfer.files);
    if (files.length === 0) return;

    const file = files[0];
    const extension = file.name.split('.').pop()?.toLowerCase();

    // Check if it's a document file (PDF, Word, Excel)
    const documentExtensions = ['pdf', 'docx', 'doc', 'xlsx', 'xls', 'pptx', 'ppt', 'txt', 'md'];
    if (extension && documentExtensions.includes(extension)) {
      await handleDocumentUpload(file);
    }
    // If it's an image, Tauri event listener will handle it
  };

  // Handle document upload
  const handleDocumentUpload = async (file: File) => {
    const messageId = `upload-${Date.now()}`;

    // Show uploading message
    setMessages(prev => [...prev, {
      id: messageId,
      role: 'system',
      content: `📄 Uploading **${file.name}** (${(file.size / 1048576).toFixed(2)} MB)...\n⏳ Parsing → Chunking → Indexing...`,
      timestamp: new Date().toISOString()
    }]);

    try {
      // Get file path from Tauri
      const filePath = await invoke<string>('save_temp_file', {
        fileName: file.name,
        fileData: Array.from(new Uint8Array(await file.arrayBuffer()))
      });

      // Upload to backend
      const result = await invoke<{
        success: boolean;
        fileName: string;
        fileType: string;
        chunksCreated: number;
        fileSizeMb: number;
        processingTimeMs: number;
        error?: string;
      }>('upload_document_file', {
        filePath,
        spaceId: sources.find(s => s.selected)?.id || null
      });

      // Update message with result
      if (result.success) {
        setMessages(prev => prev.map(m => m.id === messageId ? {
          ...m,
          content: `✅ **${result.fileName}** indexed successfully!\n\n` +
                   `📊 **${result.chunksCreated} chunks** created in ${result.processingTimeMs}ms\n` +
                   `💾 Size: ${result.fileSizeMb.toFixed(2)} MB\n\n` +
                   `*Ask me anything about this document!*`,
        } : m));
      } else {
        setMessages(prev => prev.map(m => m.id === messageId ? {
          ...m,
          content: `❌ Failed to index **${result.fileName}**\n\nError: ${result.error}`,
        } : m));
      }
    } catch (error) {
      console.error('Upload error:', error);
      setMessages(prev => prev.map(m => m.id === messageId ? {
        ...m,
        content: `❌ Upload failed: ${error}`,
      } : m));
    }
  };

  const handleDragOver = (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    // Actual handling is done by Tauri event listener
  };

  const handleDragLeave = (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    // Actual handling is done by Tauri event listener
  };

  const handleDragEnter = (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    // Actual handling is done by Tauri event listener
  };

  // File picker for images
  const handlePickImage = async () => {
    debugLog('[FILE PICKER] Button clicked');
    try {
      const selected = await open({
        multiple: false,
        filters: [{
          name: 'Images',
          extensions: ['png', 'jpg', 'jpeg', 'gif', 'bmp', 'webp']
        }]
      });

      if (selected && typeof selected === 'string') {
        debugLog('[FILE PICKER] 🖼️ Image file selected:', selected);

        try {
          debugLog('[FILE PICKER] Invoking process_image_from_file...');
          // Process the image from file path
          const result = await invoke<any>('process_image_from_file', {
            filePath: selected
          });
          debugLog('[FILE PICKER] Got result:', result);

          const extractedText = result.extractedText || result.extracted_text || '';
          const wordCount = result.wordCount || result.word_count || 0;
          const confidence = result.confidence || 0;
          const imageData = result.imageData || result.image_data || '';

          debugLog('[FILE PICKER] Adding message');
          if (extractedText && wordCount > 0) {
            setMessages(prev => [...prev, {
              id: Date.now().toString(),
              role: 'assistant',
              content: `📸 **[FILE PICKER] Image uploaded and processed successfully!**\n\n**Extracted Text (${wordCount} words, ${(confidence * 100).toFixed(0)}% confidence):**\n\n${extractedText}\n\n*The text has been indexed and is now searchable.*`,
              timestamp: new Date().toISOString(),
              image: imageData
            }]);
          } else {
            setMessages(prev => [...prev, {
              id: Date.now().toString(),
              role: 'assistant',
              content: `📸 **[FILE PICKER] Image uploaded**\n\nNo text was detected in this image.`,
              timestamp: new Date().toISOString(),
              image: imageData
            }]);
          }
        } catch (error) {
          console.error('Failed to process selected image:', error);
          notify.error('Image processing failed', `${error}`);
        }
      }
    } catch (error) {
      console.error('Failed to open file picker:', error);
    }
  };

  // Tauri file drop event listener using getCurrentWebview
  useEffect(() => {
    debugLog('🔧 Setting up Tauri drag-drop listeners using webview API...');
    let unlistenDrop: (() => void) | undefined;

    (async () => {
      try {
        // Use getCurrentWebview() not getCurrentWebviewWindow()
        const { getCurrentWebview } = await import('@tauri-apps/api/webview');
        const webview = getCurrentWebview();
        debugLog('✅ Got current webview');

        // Register file drop handler using onDragDropEvent
        unlistenDrop = await webview.onDragDropEvent(async (event: any) => {
          // Event structure from Tauri: Event<DragDropEvent> where payload is DragDropEvent
          const dragEvent = event.payload || event;

          // DragDropEvent is a union type: { type: "drop", paths: string[] } | { type: "over", position: ... } | ...
          // Check if it's a drop event with paths
          if (typeof dragEvent === 'object' && dragEvent !== null && 'type' in dragEvent && 'paths' in dragEvent && dragEvent.type === 'drop') {
            const files = dragEvent.paths as string[];
            setIsDraggingImage(false);

            // Prevent duplicate processing within 1 second
            const now = Date.now();
            if (now - lastProcessedImageTimeRef.current < 1000) {
              debugLog('⚠️ Skipping duplicate drop event (within 1s)');
              return;
            }
            lastProcessedImageTimeRef.current = now;

            try {
              // Separate images from documents/folders
              const imageFiles = files.filter((path: string) =>
                /\.(png|jpg|jpeg|gif|bmp|webp)$/i.test(path)
              );
              const documentFiles = files.filter((path: string) =>
                /\.(pdf|docx?|txt|md|html|json|csv|xlsx?)$/i.test(path)
              );

              // Process image files (OCR)
              if (imageFiles.length > 0) {
                for (const imageFile of imageFiles) {
                  const result = await invoke<any>('process_image_from_file', {
                    filePath: imageFile
                  });

                  const extractedText = result.extractedText || result.extracted_text || '';
                  const wordCount = result.wordCount || result.word_count || 0;
                  const confidence = result.confidence || 0;
                  const imageData = result.imageData || result.image_data || '';

                  if (extractedText && wordCount > 0) {
                    setMessages(prev => [...prev, {
                      id: Date.now().toString(),
                      role: 'assistant',
                      content: `📸 **Image processed successfully!**\n\n**Extracted Text (${wordCount} words, ${(confidence * 100).toFixed(0)}% confidence):**\n\n${extractedText}\n\n*The text has been indexed and is now searchable.*`,
                      timestamp: new Date().toISOString(),
                      image: imageData
                    }]);
                  } else {
                    setMessages(prev => [...prev, {
                      id: Date.now().toString(),
                      role: 'assistant',
                      content: `📸 **Image dropped**\n\nNo text was detected in this image.`,
                      timestamp: new Date().toISOString(),
                      image: imageData
                    }]);
                  }
                }
              }

              // Process document files or folders
              if (documentFiles.length > 0 || (files.length > 0 && imageFiles.length === 0 && documentFiles.length === 0)) {
                // Treat all non-image files as documents or folders to be indexed
                const pathsToIndex = documentFiles.length > 0 ? documentFiles : files;

                for (const path of pathsToIndex) {
                  const fileName = path.split(/[\\\/]/).pop() || 'Document';

                  // Check if path is a file or directory using Tauri filesystem API
                  let isDirectory = false;

                  try {
                    // Use Tauri's stat to check if it's a directory
                    const stats = await invoke<{ isDirectory: boolean }>('check_path_type', { path });
                    isDirectory = stats.isDirectory;
                  } catch (e) {
                    console.warn('Failed to check path type, assuming file:', e);
                    // If check fails, assume it's a file
                    isDirectory = false;
                  }

                  // Create a new source
                  const newSource: Source = {
                    id: Date.now().toString() + Math.random(),
                    name: fileName,
                    path: path,
                    type: 'documents',
                    fileCount: 0,
                    indexedAt: new Date().toISOString(),
                    status: 'indexing',
                    selected: true
                  };

                  setSources(prev => {
                    const updated = [...prev, newSource];
                    localStorage.setItem('indexedSources', JSON.stringify(updated));
                    return updated;
                  });

                  setCurrentlyIndexing(newSource.id);

                  // Index differently based on whether it's a file or folder
                  let result;

                  // Add timeout wrapper (5 minutes max)
                  const timeoutPromise = new Promise((_, reject) =>
                    setTimeout(() => reject(new Error('Indexing timeout - file too large or complex')), 5 * 60 * 1000)
                  );

                  try {
                    if (isDirectory) {
                      // Index entire folder
                      debugLog(`📁 Folder detected: ${fileName}, indexing all files`);
                      result = await Promise.race([
                        invoke("link_folder_enhanced", {
                          folderPath: path,
                          spaceId: newSource.id,
                          options: {
                            skip_indexed: false,
                            watch_changes: false,
                            process_subdirs: true,
                            priority: 'normal',
                            file_types: ['txt', 'md', 'pdf', 'rs', 'js', 'ts', 'py', 'java', 'cpp', 'c', 'html', 'json', 'docx', 'xlsx', 'pptx', 'csv']
                          }
                        }),
                        timeoutPromise
                      ]);
                    } else {
                      // Index single file only
                      debugLog(`📄 Single file detected: ${fileName}, indexing only this file`);
                      result = await Promise.race([
                        invoke("index_single_file", {
                          filePath: path,
                          spaceId: newSource.id
                        }),
                        timeoutPromise
                      ]);
                    }
                  } catch (indexError) {
                    console.error('❌ Indexing error:', indexError);

                    // Update source to error state
                    setSources(prev => prev.map(s =>
                      s.id === newSource.id
                        ? { ...s, status: 'error' as const }
                        : s
                    ));
                    setCurrentlyIndexing(null);

                    throw indexError; // Re-throw to be caught by outer catch
                  }

                  debugLog('✅ Document indexed:', result);

                  // Update source status
                  setSources(prev => prev.map(s =>
                    s.id === newSource.id
                      ? { ...s, status: 'ready' as const, fileCount: (result as any)?.file_count || 1 }
                      : s
                  ));
                  setCurrentlyIndexing(null);

                  // Show success message
                  setMessages(prev => [...prev, {
                    id: Date.now().toString(),
                    role: 'assistant',
                    content: `📄 **${fileName} indexed successfully!**\n\nThe document has been added to your sources and is now searchable.`,
                    timestamp: new Date().toISOString()
                  }]);
                }
              }
            } catch (error) {
              console.error('❌ Failed to process dropped files:', error);
              setMessages(prev => [...prev, {
                id: Date.now().toString(),
                role: 'assistant',
                content: `❌ Failed to process dropped files: ${error}`,
                timestamp: new Date().toISOString()
              }]);
            }
          } else if (typeof dragEvent === 'object' && dragEvent.type === 'over') {
            // Drag is hovering over the window
            setIsDraggingImage(true);
          } else if (typeof dragEvent === 'object' && (dragEvent.type === 'leave' || dragEvent.type === 'cancel')) {
            setIsDraggingImage(false);
          }
        });
        debugLog('✅ Tauri drag-drop listener registered successfully');
      } catch (error) {
        console.error('❌ Failed to setup Tauri drag-drop listener:', error);
      }
    })();

    return () => {
      debugLog('🧹 Cleaning up Tauri drag-drop listener');
      if (unlistenDrop) unlistenDrop();
    };
  }, []);

  // Global Ctrl+V handler for image paste
  useEffect(() => {
    debugLog('🔧 Setting up global Ctrl+V handler...');

    const handleGlobalPaste = async (e: KeyboardEvent) => {
      debugLog('[CTRL+V] Key event:', e.key, e.ctrlKey, e.metaKey);
      // Log all Ctrl/Cmd key combinations for debugging
      if (e.ctrlKey || e.metaKey) {
        debugLog(`🔑 Key pressed: ${e.key}, Ctrl: ${e.ctrlKey}, Meta: ${e.metaKey}`);
      }

      // Only trigger on Ctrl+V or Cmd+V
      if ((e.ctrlKey || e.metaKey) && e.key === 'v') {
        debugLog('🌐 Global Ctrl+V detected - attempting clipboard read');

        // Check if there's an image in clipboard first
        let hasImage = false;

        try {
          // Read image from clipboard using native Windows API
          const base64Data = await invoke<string | null>('read_clipboard_image');

          if (base64Data) {
            hasImage = true;
            debugLog('✅ Image found in clipboard, processing...');

            // Prevent default paste behavior when we have an image
            e.preventDefault();

            // Process the image with OCR
            const result = await invoke<any>('process_image_from_base64', {
              imageData: base64Data
            });

            debugLog('📊 OCR Result received:', result);
            debugLog('📊 Full result object:', JSON.stringify(result, null, 2));

            // Add message to chat showing what was extracted
            // Note: The Rust struct uses camelCase due to #[serde(rename_all = "camelCase")]
            // But we'll check both camelCase and snake_case for compatibility
            const extractedText = result.extractedText || result.extracted_text || '';
            const wordCount = result.wordCount || result.word_count || 0;
            const confidence = result.confidence || 0;

            if (extractedText && wordCount > 0) {
              setMessages(prev => [...prev, {
                id: Date.now().toString(),
                role: 'assistant',
                content: `📸 **Image pasted and processed successfully!**\n\n**Extracted Text (${wordCount} words, ${(confidence * 100).toFixed(0)}% confidence):**\n\n${extractedText}\n\n*The text has been indexed and is now searchable.*`,
                timestamp: new Date().toISOString(),
                image: base64Data
              }]);
            } else {
              setMessages(prev => [...prev, {
                id: Date.now().toString(),
                role: 'assistant',
                content: `📸 **Image pasted**\n\nNo text was detected in this image.`,
                timestamp: new Date().toISOString(),
                image: base64Data
              }]);
            }
          } else {
            debugLog('❌ No image in clipboard');
          }
        } catch (error) {
          console.error('❌ Failed to process clipboard image:', error);
        }
      }
    };

    window.addEventListener('keydown', handleGlobalPaste, true);
    debugLog('🌐 Global Ctrl+V listener attached');

    return () => {
      window.removeEventListener('keydown', handleGlobalPaste, true);
      debugLog('🌐 Global Ctrl+V listener removed');
    };
  }, []);

  // Check Telegram bot status on mount and listen for status changes
  useEffect(() => {
    const checkTelegramStatus = async () => {
      try {
        const status = await invoke<boolean>('check_telegram_bot_status');
        setIsTelegramBotActive(status);
      } catch (error) {
        console.error('Failed to check Telegram status:', error);
        setIsTelegramBotActive(false);
      }
    };

    checkTelegramStatus();

    // Listen for status changes from TelegramBotPanel
    const handleStatusChange = (event: any) => {
      setIsTelegramBotActive(event.detail.connected);
    };

    window.addEventListener('telegram-bot-status', handleStatusChange);
    return () => window.removeEventListener('telegram-bot-status', handleStatusChange);
  }, []);

  // Keyboard shortcut for Command Palette (Cmd+K / Ctrl+K)
  useEffect(() => {
    const handleKeyDown = (_e: KeyboardEvent) => {
      // Keyboard shortcuts can be added here
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, []);

  // Whether the backend has signalled completion (content fully received)
  const streamCompleteContentRef = useRef<string | null>(null);

  // Typewriter tick: reveals 2 words every 80ms (~25 words/sec — comfortable reading pace).
  // When the backend signals completion we let the buffer drain fully, then finalize.
  const startStreamReveal = () => {
    if (streamTimerRef.current !== null) return; // already running

    const tick = () => {
      const target = streamTargetRef.current;
      const displayed = streamDisplayedRef.current;

      if (displayed >= target.length) {
        streamTimerRef.current = null;

        // If backend already finished, finalize the message (remove streaming- prefix)
        if (streamCompleteContentRef.current !== null) {
          const finalContent = streamCompleteContentRef.current;
          streamCompleteContentRef.current = null;
          streamTargetRef.current = '';
          streamDisplayedRef.current = 0;

          // Extract any chart/code/mermaid artifacts from the final content
          const extractedArtifacts = extractArtifacts(finalContent);
          if (extractedArtifacts.length > 0) {
            setArtifacts(prev => [...prev, ...extractedArtifacts]);
          }

          setMessages(prev => {
            const lastMsg = prev[prev.length - 1];
            if (lastMsg && lastMsg.role === 'assistant' && lastMsg.id.startsWith('streaming-')) {
              // Merge frontend-extracted artifacts into the message
              const msgArtifacts = [
                ...(lastMsg.artifacts || []),
                ...extractedArtifacts,
              ];
              return prev.slice(0, -1).concat({
                ...lastMsg,
                id: lastMsg.id.replace('streaming-', ''),
                content: finalContent,
                artifacts: msgArtifacts.length > 0 ? msgArtifacts : lastMsg.artifacts,
              });
            }
            return prev;
          });

          // Generate follow-up suggestions from the completed response
          generateFollowUps(finalContent);
        }
        return;
      }

      // Advance by 2 words per tick
      let end = displayed;
      let wordsToReveal = 2;
      while (wordsToReveal > 0 && end < target.length) {
        end++;
        if (target[end] === ' ' || target[end] === '\n') {
          wordsToReveal--;
        }
      }
      if (end >= target.length) end = target.length;

      streamDisplayedRef.current = end;
      const visibleContent = target.substring(0, end);

      setMessages(prev => {
        const lastMsg = prev[prev.length - 1];
        if (lastMsg && lastMsg.role === 'assistant' && lastMsg.id.startsWith('streaming-')) {
          return prev.slice(0, -1).concat({ ...lastMsg, content: visibleContent });
        }
        return prev;
      });

      // 80ms per tick with 2 words = ~25 words/sec — readable, natural pace
      streamTimerRef.current = window.setTimeout(tick, 80);
    };

    streamTimerRef.current = window.setTimeout(tick, 80);
  };

  // Listen for streaming chat tokens and tool call events
  useEffect(() => {
    let unlistenToken: (() => void) | undefined;
    let unlistenComplete: (() => void) | undefined;
    let unlistenToolStart: (() => void) | undefined;
    let unlistenToolComplete: (() => void) | undefined;

    const setupListeners = async () => {
      // Listen for individual tokens — buffer into ref, let typewriter reveal
      unlistenToken = await listen('chat_token', (event: any) => {
        const { accumulated } = event.payload;
        streamTargetRef.current = accumulated;
        startStreamReveal(); // ensure the reveal loop is running
      });

      // Listen for completion — DON'T flush immediately. Store the final content
      // and let the typewriter drain the buffer naturally. When it catches up,
      // the tick function will detect streamCompleteContentRef and finalize.
      unlistenComplete = await listen('chat_complete', (event: any) => {
        const { content } = event.payload;
        debugLog('Streaming complete, final content length:', content.length);

        // Store final content — make sure the target has the full text
        streamTargetRef.current = content;
        streamCompleteContentRef.current = content;

        // If the typewriter is idle (already caught up), kick it to finalize
        startStreamReveal();
      });

      // Listen for tool call start — add a "running" invocation to the current streaming message
      unlistenToolStart = await listen('tool_call_start', (event: any) => {
        const { tool_name, arguments: args } = event.payload;
        debugLog('Tool call started:', tool_name);
        setMessages(prev => {
          const updated = [...prev];
          const lastMsg = updated[updated.length - 1];
          if (lastMsg && lastMsg.role === 'assistant') {
            const invocations = lastMsg.toolInvocations || [];
            invocations.push({
              tool_name,
              arguments: typeof args === 'string' ? JSON.parse(args || '{}') : (args || {}),
              result: '',
              success: false,
              duration_ms: 0,
              status: 'running',
            });
            updated[updated.length - 1] = { ...lastMsg, toolInvocations: invocations };
          }
          return updated;
        });
      });

      // Listen for tool call complete — update the matching invocation
      unlistenToolComplete = await listen('tool_call_complete', (event: any) => {
        const { tool_name, result, success, duration_ms } = event.payload;
        debugLog('Tool call completed:', tool_name, success ? 'success' : 'failed', duration_ms + 'ms');
        setMessages(prev => {
          const updated = [...prev];
          const lastMsg = updated[updated.length - 1];
          if (lastMsg && lastMsg.role === 'assistant' && lastMsg.toolInvocations) {
            const invocations = [...lastMsg.toolInvocations];
            const runningIdx = invocations.findIndex(
              inv => inv.tool_name === tool_name && inv.status === 'running'
            );
            if (runningIdx !== -1) {
              invocations[runningIdx] = {
                ...invocations[runningIdx],
                result: result || '',
                success: success ?? true,
                duration_ms: duration_ms || 0,
                status: success ? 'completed' : 'failed',
              };
              updated[updated.length - 1] = { ...lastMsg, toolInvocations: invocations };
            }
          }
          return updated;
        });
      });

      debugLog('Chat streaming listeners registered');
    };

    setupListeners().catch(console.error);

    return () => {
      if (unlistenToken) unlistenToken();
      if (unlistenComplete) unlistenComplete();
      if (unlistenToolStart) unlistenToolStart();
      if (unlistenToolComplete) unlistenToolComplete();
      if (streamTimerRef.current !== null) {
        clearTimeout(streamTimerRef.current);
        streamTimerRef.current = null;
      }
      debugLog('Chat streaming listeners cleaned up');
    };
  }, []);

  // Output formats
  const outputFormats: OutputFormat[] = [
    { id: 'md', name: 'Markdown', icon: FileText, extension: '.md', mimeType: 'text/markdown' },
    { id: 'html', name: 'HTML', icon: FileText, extension: '.html', mimeType: 'text/html' },
    { id: 'docx', name: 'Word', icon: FileText, extension: '.docx', mimeType: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document' },
    { id: 'xlsx', name: 'Excel', icon: FileSpreadsheet, extension: '.xlsx', mimeType: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet' },
    { id: 'pdf', name: 'PDF', icon: FileDown, extension: '.pdf', mimeType: 'application/pdf' },
    { id: 'txt', name: 'Text', icon: FileText, extension: '.txt', mimeType: 'text/plain' },
  ];

  // Industry-specific document templates
  const industryTemplates = {
    legal: [
      { id: 'legal-contract-review', name: 'Contract Review Summary', icon: FileCheck, prompt: 'Analyze and summarize key contract terms, obligations, liabilities, and potential risks. Include recommendations for negotiation points.', category: 'legal' },
      { id: 'legal-compliance-report', name: 'Compliance Report', icon: Shield, prompt: 'Generate a comprehensive compliance assessment report covering regulatory requirements, gaps, and remediation steps.', category: 'legal' },
      { id: 'legal-due-diligence', name: 'Due Diligence Report', icon: SearchCheck, prompt: 'Create a thorough due diligence report analyzing legal risks, obligations, and findings from document review.', category: 'legal' },
      { id: 'legal-policy-summary', name: 'Policy Summary', icon: FileText, prompt: 'Summarize key policies, procedures, and governance documents with actionable insights.', category: 'legal' },
      { id: 'legal-case-brief', name: 'Case Brief', icon: Briefcase, prompt: 'Generate a structured case brief with facts, issues, holdings, and analysis.', category: 'legal' },
    ],
    healthcare: [
      { id: 'medical-patient-summary', name: 'Patient Summary', icon: Heart, prompt: 'Generate a comprehensive patient summary including medical history, diagnoses, treatments, and recommendations.', category: 'healthcare' },
      { id: 'medical-clinical-report', name: 'Clinical Report', icon: Activity, prompt: 'Create a detailed clinical report with findings, assessments, and treatment plans.', category: 'healthcare' },
      { id: 'medical-research-summary', name: 'Research Summary', icon: TestTube, prompt: 'Summarize medical research findings, methodologies, and clinical implications.', category: 'healthcare' },
      { id: 'medical-compliance', name: 'HIPAA Compliance Report', icon: Shield, prompt: 'Generate healthcare compliance assessment covering HIPAA, data privacy, and regulatory requirements.', category: 'healthcare' },
      { id: 'medical-discharge', name: 'Discharge Summary', icon: FileText, prompt: 'Create a comprehensive discharge summary with diagnoses, treatments, medications, and follow-up instructions.', category: 'healthcare' },
    ],
    finance: [
      { id: 'financial-audit-report', name: 'Audit Report', icon: FileCheck, prompt: 'Generate a comprehensive audit report with findings, financial analysis, and compliance assessment.', category: 'finance' },
      { id: 'financial-risk-assessment', name: 'Risk Assessment', icon: AlertTriangle, prompt: 'Create a detailed financial risk assessment analyzing potential risks, exposures, and mitigation strategies.', category: 'finance' },
      { id: 'financial-investment-analysis', name: 'Investment Analysis', icon: TrendingUp, prompt: 'Analyze investment opportunities, financial metrics, risks, and recommendations.', category: 'finance' },
      { id: 'financial-quarterly-report', name: 'Quarterly Report', icon: BarChart, prompt: 'Generate a quarterly financial report with performance metrics, trends, and executive summary.', category: 'finance' },
      { id: 'financial-compliance', name: 'Regulatory Compliance', icon: Shield, prompt: 'Create compliance report covering financial regulations, AML, KYC, and regulatory requirements.', category: 'finance' },
    ],
    general: [
      { id: 'executive-report', name: 'Executive Report', icon: BarChart, prompt: 'Generate an executive summary report with key insights, metrics, and strategic recommendations', category: 'docs' },
      { id: 'technical-report', name: 'Technical Report', icon: BookOpen, prompt: 'Generate a detailed technical report with comprehensive analysis and findings', category: 'docs' },
      { id: 'analysis-report', name: 'Analysis Report', icon: BarChart, prompt: 'Generate a comprehensive analysis report with data-driven findings', category: 'analysis' },
      { id: 'summary', name: 'Summary Document', icon: FileText, prompt: 'Generate a concise summary of selected documents', category: 'docs' },
      { id: 'spreadsheet', name: 'Data Export', icon: FileSpreadsheet, prompt: 'Generate spreadsheet with extracted data and tables', category: 'docs' },
    ]
  };

  const documentTemplates = industryTemplates[selectedIndustry];

  // Code templates
  const codeTemplates: GenerationTemplate[] = [
    { id: 'readme', name: 'README', icon: BookOpen, prompt: 'Generate a comprehensive README', category: 'code' },
    { id: 'api-docs', name: 'API Docs', icon: Network, prompt: 'Generate API documentation', category: 'code' },
    { id: 'unit-tests', name: 'Unit Tests', icon: TestTube, prompt: 'Generate unit tests', category: 'code' },
    { id: 'types', name: 'TypeScript Types', icon: FileCode, prompt: 'Generate TypeScript type definitions', category: 'code' },
    { id: 'component', name: 'React Component', icon: Layers, prompt: 'Generate a React component', category: 'code' },
  ];

  // Initialize app once on mount
  useEffect(() => {
    initializeApp();
  }, []); // Empty dependency array - run only once on mount

  // Setup Telegram and Discord event listeners (no dependencies - run once)
  useEffect(() => {
    let unlistenTelegramMsg: (() => void) | undefined;
    let unlistenTelegramResp: (() => void) | undefined;
    let unlistenDiscordMsg: (() => void) | undefined;
    let unlistenDiscordResp: (() => void) | undefined;
    let isMounted = true;

    // Setup listeners with async
    (async () => {
      if (!isMounted) return;

      // Listen for Telegram messages
      unlistenTelegramMsg = await listen('telegram-message', (event: any) => {
        const { username, message, timestamp } = event.payload;
        const newMessage: Message = {
          id: Date.now().toString(),
          role: 'user',
          content: `📱 Telegram (@${username}): ${message}`,
          timestamp: new Date(timestamp).toISOString(),
          platform: 'telegram',
        };
        setMessages(prev => [...prev, newMessage]);
      });

      if (!isMounted) {
        unlistenTelegramMsg?.();
        return;
      }

      unlistenTelegramResp = await listen('telegram-response', (event: any) => {
        const { username, message, timestamp } = event.payload;
        const newMessage: Message = {
          id: Date.now().toString() + '_response',
          role: 'assistant',
          content: message,
          timestamp: new Date(timestamp).toISOString(),
          platform: 'telegram',
        };
        setMessages(prev => [...prev, newMessage]);
      });

      if (!isMounted) {
        unlistenTelegramMsg?.();
        unlistenTelegramResp?.();
        return;
      }

      // Listen for Discord messages
      unlistenDiscordMsg = await listen('discord-message', (event: any) => {
        const { username, message, timestamp } = event.payload;
        const newMessage: Message = {
          id: Date.now().toString(),
          role: 'user',
          content: `💬 Discord (@${username}): ${message}`,
          timestamp: new Date(timestamp).toISOString(),
          platform: 'discord',
        };
        setMessages(prev => [...prev, newMessage]);
      });

      if (!isMounted) {
        unlistenTelegramMsg?.();
        unlistenTelegramResp?.();
        unlistenDiscordMsg?.();
        return;
      }

      unlistenDiscordResp = await listen('discord-response', (event: any) => {
        const { username, message, timestamp } = event.payload;
        const newMessage: Message = {
          id: Date.now().toString() + '_response',
          role: 'assistant',
          content: message,
          timestamp: new Date(timestamp).toISOString(),
          platform: 'discord',
        };
        setMessages(prev => [...prev, newMessage]);
      });

      if (!isMounted) {
        unlistenTelegramMsg?.();
        unlistenTelegramResp?.();
        unlistenDiscordMsg?.();
        unlistenDiscordResp?.();
      }
    })();

    return () => {
      isMounted = false;
      unlistenTelegramMsg?.();
      unlistenTelegramResp?.();
      unlistenDiscordMsg?.();
      unlistenDiscordResp?.();
    };
  }, []); // Empty deps - only run once

  // Setup other event listeners
  useEffect(() => {
    let unlistenProg: (() => void) | null = null;

    // Listen for tab switching events from child components
    const handleSwitchTab = (event: any) => {
      const tab = event.detail;
      if (tab) {
        setActiveTab(tab as any);
      }
    };
    window.addEventListener('switchTab', handleSwitchTab);

    // Listen for indexing progress events
    listen('indexing-progress', (event: any) => {
      debugLog('=== INDEXING PROGRESS EVENT ===');
      debugLog('Full event:', event);
      debugLog('Payload:', event.payload);
      const { current_file, processed_files, total_files, percentage, current_action } = event.payload;

      // Update the currently indexing source with progress
      setSources(prev => prev.map(source => {
        // Update the source that's currently being indexed
        if (source.id === currentlyIndexing || source.status === 'indexing') {
          return {
            ...source,
            status: percentage >= 100 ? 'ready' : 'indexing',
            progress: Math.round(percentage),
            currentFile: current_file === 'Completed' ? undefined : current_file,
            fileCount: total_files || source.fileCount,
            processedCount: processed_files
          };
        }
        return source;
      }));

      // If complete, update stats and clear indexing flag
      if (percentage >= 100 && current_file === 'Completed') {
        setCurrentlyIndexing(null);
        updateStats();
      }
    }).then(fn => { unlistenProg = fn; });

    return () => {
      window.removeEventListener('switchTab', handleSwitchTab);
      unlistenProg?.();
    };
  }, [currentlyIndexing]);

  useEffect(() => {
    const isNewMessage = messages.length !== prevMessageCountRef.current;
    prevMessageCountRef.current = messages.length;
    scrollToBottom(isNewMessage);
  }, [messages]);

  useEffect(() => {
    updateSelectedStats();
  }, [sources]);

  const initializeApp = async () => {
    // Prevent double initialization (React Strict Mode in dev runs effects twice)
    if (initializationRef.current) {
      debugLog("⚠️ Initialization already in progress or completed, skipping...");
      // If there's an ongoing initialization, wait for it
      if (initializationPromiseRef.current) {
        await initializationPromiseRef.current;
      }
      return;
    }

    debugLog("=== INITIALIZING APP ===");
    initializationRef.current = true;

    // Store the initialization promise so concurrent calls can wait for it
    initializationPromiseRef.current = (async () => {
      try {
        // Simulate minimum loading time for smooth UX
        const startTime = Date.now();

        // Initialize RAG
        debugLog("Initializing RAG system...");
        const ragInitResult = await invoke("initialize_rag");
        debugLog("RAG init result:", ragInitResult);

      // Check LLM status
      const checkLLMStatus = async () => {
        try {
          const info: any = await invoke("get_llm_info");
          if (info) {
            setLlmStatus({
              connected: true,
              model: info.model || 'Unknown',
              provider: info.provider || 'Unknown'
            });
            return true; // LLM is connected
          }
        } catch (e) {
          debugLog("LLM not configured:", e);
          setLlmStatus({
            connected: false,
            model: 'Not configured',
            provider: 'none'
          });
        }
        return false; // LLM not connected
      };

      // Initial check
      const isConnected = await checkLLMStatus();

      // If not connected, poll every 2 seconds for up to 30 seconds
      if (!isConnected) {
        let attempts = 0;
        const maxAttempts = 15; // 30 seconds total
        const pollInterval = setInterval(async () => {
          attempts++;
          const connected = await checkLLMStatus();
          if (connected || attempts >= maxAttempts) {
            clearInterval(pollInterval);
            if (connected) {
              debugLog("✅ LLM connected after polling");
            } else {
              debugLog("⏰ LLM polling timeout - LLM may need manual configuration");
            }
          }
        }, 2000);
      }

      // Load saved sources
      const savedSources = localStorage.getItem('indexedSources');
      if (savedSources) {
        const parsed = JSON.parse(savedSources);
        setSources(parsed);
        setIsFirstTime(parsed.length === 0);

        // Fetch file counts for all sources
        if (parsed.length > 0) {
          parsed.forEach(async (source: Source) => {
            if (source.status === 'ready') {
              try {
                const files = await invoke<any[]>('get_source_files', { sourceId: source.id });
                setSources(prevSources =>
                  prevSources.map(s =>
                    s.id === source.id
                      ? { ...s, fileCount: files.length }
                      : s
                  )
                );
              } catch (error) {
                console.error(`Failed to fetch file count for source ${source.id}:`, error);
              }
            }
          });
        }
      } else {
        setIsFirstTime(true);
      }

      await updateStats();

      // Ensure minimum loading time for smooth transition
      const elapsed = Date.now() - startTime;
      if (elapsed < 1500) {
        await new Promise(resolve => setTimeout(resolve, 1500 - elapsed));
      }

      setIsLoading(false);
      } catch (error) {
        console.error("Initialization failed:", error);
        setIsLoading(false);
        // Reset flag on error so user can retry
        initializationRef.current = false;
        throw error;
      }
    })();

    // Await the initialization
    await initializationPromiseRef.current;
  };

  const updateStats = async (sourcesOverride?: Source[]) => {
    try {
      const stats: any = await invoke("get_statistics");
      debugLog("Stats from backend:", stats);

      const currentSources = sourcesOverride || sources;
      const totalFiles = currentSources.reduce((sum, s) => sum + s.fileCount, 0);

      setStats({
        totalDocs: stats.total_documents || totalFiles || 0,
        totalChunks: stats.total_chunks || 0,
        selectedDocs: currentSources.filter(s => s.selected).reduce((sum, s) => sum + s.fileCount, 0),
        indexSize: stats.index_size_mb ? `${parseFloat(stats.index_size_mb).toFixed(2)} MB` : "0 MB"
      });
    } catch (error) {
      console.error("Failed to get stats:", error);
    }
  };

  const updateSelectedStats = () => {
    const selected = sources.filter(s => s.selected);
    const selectedDocs = selected.reduce((sum, s) => sum + s.fileCount, 0);
    setStats(prev => ({ ...prev, selectedDocs }));
  };

  const scrollToBottom = (force = false) => {
    if (!chatEndRef.current) return;

    // When a new message is added (user sends or assistant placeholder created),
    // ALWAYS scroll to show it. For streaming content updates, only scroll if
    // user is near the bottom (hasn't scrolled up to re-read).
    if (!force) {
      const container = chatScrollContainerRef.current;
      if (container) {
        const distFromBottom = container.scrollHeight - container.scrollTop - container.clientHeight;
        if (distFromBottom > 400) return;
      }
    }

    const lastMsg = messages[messages.length - 1];
    const isStreaming = lastMsg?.id?.startsWith('streaming-');
    chatEndRef.current.scrollIntoView({ behavior: isStreaming ? "instant" : "smooth" });
  };

  const handleOptimizeStorage = async () => {
    try {
      debugLog("Optimizing storage...");
      const result = await invoke<string>('optimize_storage');
      debugLog("Storage optimization result:", result);

      await updateStats();

      notify.success('Storage optimized', result);
    } catch (error) {
      console.error("Failed to optimize storage:", error);
      notify.error('Storage optimization failed', `${error}`);
    }
  };

  const handleAddSource = async () => {
    debugLog("=== handleAddSource START ===");

    try {
      debugLog("Opening folder dialog...");
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Select documents folder"
      });

      debugLog("Folder selected:", selected);

      if (selected) {
        debugLog("Processing selected folder...");
        const newSource: Source = {
          id: Date.now().toString(),
          name: (selected as string).split(/[\\\/]/).pop() || 'Folder',
          path: selected as string,
          type: 'documents',
          fileCount: 0,
          indexedAt: new Date().toISOString(),
          status: 'indexing',
          selected: true,
        };

        setSources(prev => {
          const updated = [...prev, newSource];
          localStorage.setItem('indexedSources', JSON.stringify(updated));
          return updated;
        });

        // Set this source as currently indexing
        setCurrentlyIndexing(newSource.id);

        // Index the folder with enhanced progress
        // Note: Top-level params in camelCase, nested struct fields in snake_case
        debugLog("=== Calling link_folder_enhanced ===");
        debugLog("Parameters:", {
          folderPath: selected as string,
          spaceId: newSource.id,
          options: {
            skip_indexed: false,
            watch_changes: false,
            process_subdirs: true,
            priority: 'normal',
            file_types: ['txt', 'md', 'pdf', 'rs', 'js', 'ts', 'py', 'java', 'cpp', 'c', 'html', 'json', 'docx']
          }
        });

        // Try both methods to see which one works
        let result;
        try {
          debugLog("=== TRYING ENHANCED link_folder_enhanced METHOD ===");
          result = await invoke("link_folder_enhanced", {
            folderPath: selected as string,
            spaceId: newSource.id,
            options: {
              skip_indexed: false,
              watch_changes: false,
              process_subdirs: true,
              priority: 'normal',
              file_types: ['txt', 'md', 'pdf', 'rs', 'js', 'ts', 'py', 'java', 'cpp', 'c', 'html', 'json', 'docx']
            }
          });
          debugLog('Enhanced indexing succeeded:', result);
        } catch (enhancedError) {
          console.error("Enhanced method failed:", enhancedError);

          // Fall back to old method
          debugLog("=== FALLING BACK TO OLD link_folder METHOD ===");
          result = await invoke("link_folder", {
            folderPath: selected as string,
            metadata: {
              space_id: newSource.id,
              source_type: 'documents'
            }
          });
          debugLog('Old indexing succeeded:', result);
        }

        debugLog('Final indexing result:', result);
        debugLog('Result type:', typeof result);
        debugLog('Result keys:', Object.keys(result as any));

        // Extract file count from result
        const filesProcessed = (result as any).files_processed ||
                              (result as any).filesProcessed ||
                              (result as any).file_count ||
                              (result as any).fileCount ||
                              0;

        debugLog('Files processed extracted:', filesProcessed);

        // Update status with file count from result
        let updatedSources: Source[] = [];
        setSources(prev => {
          updatedSources = prev.map(s =>
            s.id === newSource.id ? {
              ...s,
              status: 'ready' as const,
              fileCount: filesProcessed,
              progress: undefined,
              currentFile: undefined,
              processedCount: undefined
            } : s
          );
          localStorage.setItem('indexedSources', JSON.stringify(updatedSources));
          return updatedSources;
        });

        setCurrentlyIndexing(null);
        await updateStats(updatedSources);

        // Get actual file count from backend
        let actualFileCount = filesProcessed;
        try {
          const files = await invoke<any[]>('get_source_files', { sourceId: newSource.id });
          actualFileCount = files.length;
        } catch (e) {
          console.error('Failed to get actual file count:', e);
        }

        // Track document indexing activity for timeline
        await trackActivity({
          activityType: 'document_added',
          data: `Indexed ${actualFileCount} files from ${newSource.name}`,
          project: 'shodh'
        });

        notify.success(`Indexed ${actualFileCount} files`, { description: newSource.name });
      }
    } catch (error) {
      console.error("Failed to add source:", error);
      notify.error('Indexing failed', { description: String(error) });

      // Reset indexing status on error
      if (currentlyIndexing) {
        setSources(prev => {
          const updated = prev.map(s =>
            s.id === currentlyIndexing ? { ...s, status: 'error' as const, progress: undefined } : s
          );
          localStorage.setItem('indexedSources', JSON.stringify(updated));
          return updated;
        });
        setCurrentlyIndexing(null);
      }
    }
  };

  const toggleSource = useCallback((id: string) => {
    setSources(prev => {
      const updated = prev.map(s =>
        s.id === id ? { ...s, selected: !s.selected } : s
      );
      localStorage.setItem('indexedSources', JSON.stringify(updated));
      return updated;
    });
  }, []);

  const removeSource = async (id: string, event: React.MouseEvent) => {
    event.stopPropagation();

    const source = sources.find(s => s.id === id);
    if (!source) return;

    // If currently indexing, cancel it first
    if (source.id === currentlyIndexing) {
      setCurrentlyIndexing(null);
    }

    // Cancel any existing pending delete for this source
    const existing = pendingSourceDeleteRef.current.get(id);
    if (existing) {
      clearTimeout(existing.timeout);
      pendingSourceDeleteRef.current.delete(id);
    }

    // Optimistically remove from UI
    setSources(prev => {
      const updated = prev.filter(s => s.id !== id);
      localStorage.setItem('indexedSources', JSON.stringify(updated));
      return updated;
    });

    // Schedule actual backend deletion after 5s (undo window)
    const timeout = setTimeout(() => {
      pendingSourceDeleteRef.current.delete(id);
      invoke<string>("delete_folder_source", { folderPath: source.path })
        .catch(err => console.warn('Backend source deletion failed:', err));
    }, 5000);

    pendingSourceDeleteRef.current.set(id, { timeout, source });

    toast('Source removed', {
      description: source.name,
      action: {
        label: 'Undo',
        onClick: () => {
          const pending = pendingSourceDeleteRef.current.get(id);
          if (pending) {
            clearTimeout(pending.timeout);
            pendingSourceDeleteRef.current.delete(id);
            // Restore source to UI
            setSources(prev => {
              const restored = [...prev, pending.source];
              localStorage.setItem('indexedSources', JSON.stringify(restored));
              return restored;
            });
            notify.success('Source restored');
          }
        },
      },
      duration: 5000,
    });
  };

  // Cancel ongoing streaming
  const cancelStreaming = () => {
    debugLog('🛑 Cancelling operation...');

    // Cancel streaming if active
    if (streamingCleanupRef.current) {
      streamingCleanupRef.current();
      streamingCleanupRef.current = null;
    }

    // Cancel unified_chat if active
    if (currentOperationAbortRef.current) {
      currentOperationAbortRef.current();
      currentOperationAbortRef.current = null;
    }

    // Reset all processing states
    setIsProcessing(false);
    setPipelineActive(false);
    setSearchPipeline({ stage: 'idle', progress: 0 });

    // Add cancellation message to chat
    const cancelMessage: Message = {
      id: Date.now().toString(),
      role: 'system',
      content: '⏹️ Operation cancelled by user (ESC pressed)',
      timestamp: new Date().toISOString(),
    };
    setMessages(prev => [...prev, cancelMessage]);
  };

  // ESC key handler
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && isProcessing) {
        e.preventDefault();
        cancelStreaming();
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [isProcessing]);

  // Voice input via Web Speech API
  const toggleVoiceInput = () => {
    if (isListening) {
      recognitionRef.current?.stop();
      setIsListening(false);
      return;
    }

    const SpeechRecognition = (window as any).webkitSpeechRecognition || (window as any).SpeechRecognition;
    if (!SpeechRecognition) {
      notify.warning('Voice input is not supported in this browser');
      return;
    }

    const recognition = new SpeechRecognition();
    recognition.continuous = false;
    recognition.interimResults = true;
    recognition.lang = 'en-US';

    // Snapshot the current input so voice results replace cleanly (no stutter accumulation)
    const inputBeforeVoice = currentInput;
    let finalTranscript = '';

    recognition.onresult = (event: any) => {
      let interim = '';
      finalTranscript = '';
      for (let i = 0; i < event.results.length; i++) {
        const transcript = event.results[i][0].transcript;
        if (event.results[i].isFinal) {
          finalTranscript += transcript;
        } else {
          interim += transcript;
        }
      }
      // Always set from snapshot + accumulated transcript — never append to previous interim
      const base = inputBeforeVoice + (inputBeforeVoice && !inputBeforeVoice.endsWith(' ') ? ' ' : '');
      const voiceText = finalTranscript || interim;
      setCurrentInput(base + voiceText);
    };

    recognition.onend = () => {
      setIsListening(false);
      recognitionRef.current = null;
    };

    recognition.onerror = (event: any) => {
      setIsListening(false);
      recognitionRef.current = null;
      if (event.error !== 'aborted') {
        notify.error(`Voice input failed: ${event.error}`);
      }
    };

    recognitionRef.current = recognition;
    recognition.start();
    setIsListening(true);
  };

  // Unified chat handler using backend chat_engine
  const handleSendMessageUnified = async () => {
    if (!currentInput.trim() || isProcessing) return;

    const userMessage: Message = {
      id: Date.now().toString(),
      role: 'user',
      content: currentInput,
      timestamp: new Date().toISOString()
    };

    setMessages(prev => [...prev, userMessage]);
    const userQuery = currentInput;
    setCurrentInput("");
    setIsProcessing(true);
    setFollowUpSuggestions([]);
    artifactExtractorRef.current.reset();

    // Auto-associate current source with conversation on first message
    if (activeConversationId && !activeConversation?.spaceId && activeSpaceId && activeSourceName) {
      updateConversationMeta(activeConversationId, { spaceId: activeSpaceId, spaceName: activeSourceName });
    }

    // Track activity
    await trackActivity({
      activityType: 'search',
      data: `Chat: "${userQuery}"`,
      project: 'shodh'
    });

    try {
      // Get current space ID
      const currentSpaceId = sources.find(s => s.selected)?.id || null;

      // Build conversation history from messages
      const conversationHistory = messages.slice(-10).map(msg => ({
        role: msg.role,
        content: msg.content,
      }));

      // Set up cancellation mechanism
      let cancelled = false;
      currentOperationAbortRef.current = () => {
        cancelled = true;
        debugLog('🛑 Marked operation as cancelled');
      };

      // Create a placeholder assistant message for streaming
      const streamingMessageId = `streaming-${Date.now()}`;
      const streamingMessage: Message = {
        id: streamingMessageId,
        role: 'assistant',
        content: '',
        timestamp: new Date().toISOString(),
      };
      // Reset typewriter buffer for new streaming session
      streamTargetRef.current = '';
      streamDisplayedRef.current = 0;
      streamCompleteContentRef.current = null;
      if (streamTimerRef.current !== null) { clearTimeout(streamTimerRef.current); streamTimerRef.current = null; }
      setMessages(prev => [...prev, streamingMessage]);

      // Call unified_chat with full context
      const response = await invoke('unified_chat', {
        message: userQuery,
        context: {
          agent_id: activeAgentId || null,
          conversation_history: conversationHistory,
          space_id: currentSpaceId,
          conversation_id: null,
          user_info: null,
          variables: {},
          metadata: {},
          custom_system_prompt: spaceSystemPrompt || null,
        }
      });

      // Check if cancelled before processing response
      if (cancelled) {
        debugLog('⏹️ Ignoring response - operation was cancelled');
        return;
      }

      // Clear abort ref after successful completion
      currentOperationAbortRef.current = null;

      debugLog('🚀 Unified chat response:', response);

      // Extract artifacts from response (backend-detected)
      const responseObj = response as any;
      if (responseObj.artifacts && responseObj.artifacts.length > 0) {
        debugLog('🎨 Found', responseObj.artifacts.length, 'backend artifacts');
        setArtifacts(prev => [...prev, ...responseObj.artifacts]);
      }

      // Also extract artifacts from the response text (frontend-detected: inline charts, etc.)
      const responseContent = responseObj.response || responseObj.content || '';
      if (responseContent) {
        const frontendArtifacts = extractArtifacts(responseContent);
        if (frontendArtifacts.length > 0) {
          debugLog('🎨 Found', frontendArtifacts.length, 'frontend-extracted artifacts');
          setArtifacts(prev => [...prev, ...frontendArtifacts]);
        }
      }

      // Build metadata footer if available
      let metadataFooter = '';
      if (responseObj.metadata) {
        const meta = responseObj.metadata;
        const parts = [];

        if (meta.model) {
          const displayModel = meta.model === 'llm' ? llmStatus.model : meta.model;
          parts.push(`🤖 ${displayModel}`);
        }
        if (meta.inputTokens) parts.push(`🟢 ${meta.inputTokens}`);
        if (meta.outputTokens) parts.push(`🔴 ${meta.outputTokens}`);
        if (meta.durationMs) parts.push(`⏱ ${(meta.durationMs / 1000).toFixed(1)}s`);
        if (meta.outputTokens && meta.durationMs) {
          const tokensPerSec = (meta.outputTokens / (meta.durationMs / 1000)).toFixed(1);
          parts.push(`⚡ ${tokensPerSec} tok/s`);
        }

        if (parts.length > 0) {
          metadataFooter = `\n\n${parts.join(' │ ')}`;
        }
      }

      // The invoke has returned with the complete response.
      // The streaming typewriter may still be revealing content word-by-word.
      // Strategy: stop the typewriter, use the FULL response content (from the
      // response object, not the partially revealed message), and apply metadata.
      if (streamTimerRef.current !== null) {
        clearTimeout(streamTimerRef.current);
        streamTimerRef.current = null;
      }
      streamTargetRef.current = '';
      streamDisplayedRef.current = 0;
      streamCompleteContentRef.current = null;

      // The response object has the authoritative full content.
      const authoritativeContent = responseObj.content || responseObj.text || '';

      // Merge backend artifacts with frontend-extracted artifacts (charts, etc.)
      const frontendArtifacts = extractArtifacts(authoritativeContent);
      const allMsgArtifacts = [
        ...(responseObj.artifacts || []),
        ...frontendArtifacts,
      ];

      setMessages(prev => {
        const lastMsg = prev[prev.length - 1];
        if (lastMsg && lastMsg.role === 'assistant') {
          const finalContent = authoritativeContent || lastMsg.content || '';
          debugLog('Final message update - content length:', finalContent.length, 'artifacts:', allMsgArtifacts.length);
          return prev.slice(0, -1).concat({
            ...lastMsg,
            id: lastMsg.id.replace('streaming-', ''),
            content: finalContent + metadataFooter,
            artifacts: allMsgArtifacts,
            searchResults: responseObj.search_results || [],
          });
        }
        return [...prev, {
          id: (Date.now() + 1).toString(),
          role: 'assistant',
          content: authoritativeContent + metadataFooter,
          timestamp: new Date().toISOString(),
          artifacts: allMsgArtifacts,
          searchResults: responseObj.search_results || [],
        }];
      });

    } catch (error: any) {
      console.error('Unified chat error:', error);
      notify.error('Chat failed', { description: String(error?.message || error).slice(0, 120) });

      // Update the streaming message with error or create new error message
      setMessages(prev => {
        const lastMsg = prev[prev.length - 1];
        if (lastMsg && lastMsg.role === 'assistant' && lastMsg.id.startsWith('streaming-')) {
          return prev.slice(0, -1).concat({
            ...lastMsg,
            id: lastMsg.id.replace('streaming-', ''),
            content: `Error: ${error.message || error}`,
          });
        }
        // Fallback: create new error message
        return [...prev, {
          id: (Date.now() + 1).toString(),
          role: 'assistant',
          content: `Error: ${error.message || error}`,
          timestamp: new Date().toISOString(),
        }];
      });
    } finally {
      setIsProcessing(false);
      setPipelineActive(false);
      setSearchPipeline({ stage: 'idle', progress: 0 });
    }
  };


  const handleSearch = async () => {
    if (!searchQuery.trim()) return;

    setIsSearching(true);
    try {
      // Track user search message
      await trackUserMessage(searchQuery);

      // Use intelligent search
      const currentSpaceId = sources.find(s => s.selected)?.id || null;
      const { decision, results } = await intelligentSearch(
        searchQuery,
        currentSpaceId,
        20
      );

      debugLog("Search decision:", {
        shouldRetrieve: decision.shouldRetrieve,
        reasoning: decision.reasoning
      });

      setSearchResults(results as any[]);

      // Show decision to user if no retrieval
      if (!decision.shouldRetrieve) {
        debugLog("Search not needed:", decision.reasoning);
      }

      // Track search activity
      await trackActivity({
        activityType: 'search',
        data: `Searched for "${searchQuery}"`,
        project: 'shodh'
      });
    } catch (error) {
      console.error("Search failed:", error);
    } finally {
      setIsSearching(false);
    }
  };

  // Toggle source file list expansion
  const toggleSourceExpansion = async (sourceId: string, e: React.MouseEvent) => {
    e.stopPropagation(); // Prevent source selection toggle

    const newExpanded = new Set(expandedSources);

    if (newExpanded.has(sourceId)) {
      newExpanded.delete(sourceId);
    } else {
      newExpanded.add(sourceId);

      // Fetch files for this source if not already loaded
      if (!sourceFiles[sourceId]) {
        try {
          debugLog('Fetching files for source:', sourceId);
          const files = await invoke<any[]>('get_source_files', { sourceId });
          debugLog('Received files:', files);
          setSourceFiles(prev => ({ ...prev, [sourceId]: files || [] }));

          // Update the source's fileCount
          setSources(prevSources =>
            prevSources.map(s =>
              s.id === sourceId
                ? { ...s, fileCount: files.length }
                : s
            )
          );
        } catch (error) {
          console.error('Failed to fetch source files:', error);
          setSourceFiles(prev => ({ ...prev, [sourceId]: [] }));
        }
      }
    }

    setExpandedSources(newExpanded);
  };

  // Toggle source expansion in Documents tab (independent from sidebar)
  const toggleDocsSourceExpansion = async (sourceId: string, e: React.MouseEvent) => {
    e.stopPropagation();

    const newExpanded = new Set(docsExpandedSources);

    if (newExpanded.has(sourceId)) {
      newExpanded.delete(sourceId);
    } else {
      newExpanded.add(sourceId);

      if (!sourceFiles[sourceId]) {
        try {
          const files = await invoke<any[]>('get_source_files', { sourceId });
          setSourceFiles(prev => ({ ...prev, [sourceId]: files || [] }));
          setSources(prevSources =>
            prevSources.map(s =>
              s.id === sourceId ? { ...s, fileCount: files.length } : s
            )
          );
        } catch (error) {
          console.error('Failed to fetch source files:', error);
          setSourceFiles(prev => ({ ...prev, [sourceId]: [] }));
        }
      }
    }

    setDocsExpandedSources(newExpanded);
  };

  // Get file icon, color, and badge based on file type — stable reference
  const getFileIconInfo = useCallback((fileType: string): { Icon: any; color: string; badge: string } => {
    const type = fileType.toLowerCase();

    // Programming languages
    if (type.includes('rust')) return { Icon: Settings, color: '#f74c00', badge: 'RS' };
    if (type.includes('python')) return { Icon: FileCode, color: '#3776ab', badge: 'PY' };
    if (type.includes('javascript')) return { Icon: Braces, color: '#f7df1e', badge: 'JS' };
    if (type.includes('typescript')) return { Icon: Braces, color: '#3178c6', badge: 'TS' };
    if (type.includes('java')) return { Icon: Coffee, color: '#f89820', badge: 'JAVA' };
    if (type.includes('cpp') || type.includes('c_code') || type === 'c' || type === 'h') return { Icon: Terminal, color: '#00599c', badge: 'C++' };
    if (type.includes('csharp')) return { Icon: Code, color: '#239120', badge: 'C#' };
    if (type.includes('go')) return { Icon: FileCode, color: '#00add8', badge: 'GO' };
    if (type.includes('ruby')) return { Icon: FileCode, color: '#cc342d', badge: 'RB' };
    if (type.includes('php')) return { Icon: Code, color: '#777bb4', badge: 'PHP' };
    if (type.includes('swift')) return { Icon: Code, color: '#f05138', badge: 'SWIFT' };
    if (type.includes('kotlin')) return { Icon: Code, color: '#7f52ff', badge: 'KT' };
    if (type === 'sh' || type === 'bash' || type === 'zsh') return { Icon: Terminal, color: '#4eaa25', badge: 'SH' };

    // Web files
    if (type === 'html') return { Icon: Code, color: '#e34c26', badge: 'HTML' };
    if (type === 'css' || type === 'scss' || type === 'sass') return { Icon: FileCode, color: '#264de4', badge: 'CSS' };
    if (type === 'vue') return { Icon: Code, color: '#42b883', badge: 'VUE' };
    if (type === 'svelte') return { Icon: Code, color: '#ff3e00', badge: 'SVELTE' };

    // Data/Config files
    if (type === 'json') return { Icon: Braces, color: '#000000', badge: 'JSON' };
    if (type === 'yaml' || type === 'yml') return { Icon: FileCode, color: '#cb171e', badge: 'YAML' };
    if (type === 'toml') return { Icon: FileCode, color: '#9c4221', badge: 'TOML' };
    if (type === 'xml') return { Icon: Code, color: '#0060ac', badge: 'XML' };
    if (type === 'sql') return { Icon: Database, color: '#f29111', badge: 'SQL' };

    // Documents
    if (type === 'pdf') return { Icon: FileText, color: '#ef4444', badge: 'PDF' };
    if (type === 'docx' || type === 'doc') return { Icon: FileText, color: '#2b579a', badge: 'DOCX' };
    if (type === 'xlsx' || type === 'xls') return { Icon: FileSpreadsheet, color: '#217346', badge: 'XLSX' };
    if (type === 'pptx' || type === 'ppt') return { Icon: Presentation, color: '#d24726', badge: 'PPTX' };
    if (type === 'md' || type === 'markdown' || type.includes('documentation')) return { Icon: BookOpen, color: '#8b5cf6', badge: 'MD' };
    if (type === 'txt') return { Icon: FileText, color: '#6b7280', badge: 'TXT' };

    // Default for unknown types
    return { Icon: FileText, color: '#9ca3af', badge: type.toUpperCase().slice(0, 4) };
  }, []);

  const handleGenerate = async (template?: GenerationTemplate) => {
    setIsGenerating(true);
    setGenerationProgress(0);

    try {
      const prompt = template ? template.prompt : generationContext;

      if (!prompt.trim()) {
        notify.warning('Please enter a description of what you want to generate');
        return;
      }

      // Simulate progress animation
      const progressInterval = setInterval(() => {
        setGenerationProgress(prev => Math.min(prev + 10, 90));
      }, 300);

      debugLog("🔧 Generating document:", { prompt, format: outputFormat, template: template?.id });

      // Call backend with proper signature
      const response = await invoke("generate_from_rag", {
        prompt: prompt,
        format: outputFormat,
        includeReferences: true,
        maxSourceDocs: 10,
        template: template?.id || null
      }) as any;

      clearInterval(progressInterval);
      setGenerationProgress(100);

      debugLog("✅ Document generated:", response);
      debugLog("📊 Sources used:", response.metadata?.sources?.length || 0);
      debugLog("📄 Preview length:", response.preview?.length || 0);
      debugLog("📦 Content base64 length:", response.content_base64?.length || 0);

      // Store the generated document
      setGeneratedContent(response);
      setGenerationProgress(0);

      // Track document generation activity for timeline
      await trackActivity({
        activityType: 'task_completed',
        data: `Generated ${outputFormat.toUpperCase()} document: ${prompt.substring(0, 50)}...`,
        project: 'shodh'
      });

      // Show preview or download based on format
      if (outputFormat === 'md' || outputFormat === 'html' || outputFormat === 'txt') {
        // Text-based formats - show preview
        try {
          // Use preview field if available (already decoded)
          if (response.preview) {
            setGeneratedPreview(response.preview);
          } else if (response.content_base64) {
            // Decode base64 content
            const decoded = atob(response.content_base64);
            setGeneratedPreview(decoded);
          } else {
            console.error("No content or preview available in response:", response);
            notify.warning('Document generated but no content available for preview');
          }
        } catch (decodeError) {
          console.error("Failed to decode document content:", decodeError);
          debugLog("Response:", response);
          notify.error('Document preview failed', 'Check console for details');
        }
      } else {
        // Binary formats - offer download
        await downloadGeneratedDocument(response);
      }

    } catch (error: any) {
      console.error("❌ Generation failed:", error);
      notify.error('Document generation failed', `${error.message || error}`);
    } finally {
      setIsGenerating(false);
      setGenerationProgress(0);
    }
  };

  // Download generated document (handles both text and binary formats)
  const downloadGeneratedDocument = async (response: any) => {
    try {
      if (!response || !response.content_base64) {
        notify.error('No document content to download');
        return;
      }

      const format = outputFormats.find(f => f.id === response.format);
      if (!format) return;

      // Decode base64 content
      const binaryString = atob(response.content_base64);
      const bytes = new Uint8Array(binaryString.length);
      for (let i = 0; i < binaryString.length; i++) {
        bytes[i] = binaryString.charCodeAt(i);
      }

      // Create blob
      const blob = new Blob([bytes], { type: format.mimeType });
      const url = URL.createObjectURL(blob);

      // Download file
      const a = document.createElement('a');
      a.href = url;
      a.download = response.title || `document-${Date.now()}${format.extension}`;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);

      debugLog("✅ Document downloaded:", response.title);
    } catch (error) {
      console.error('Download failed:', error);
      notify.error('Failed to download document');
    }
  };

  const handleExport = async () => {
    if (!generatedContent) return;

    try {
      await downloadGeneratedDocument(generatedContent);
    } catch (error) {
      console.error('Export failed:', error);
    }
  };

  // Streaming generation handler
  const handleGenerateStream = async () => {
    if (!generateInput.trim()) {
      notify.warning('Please enter a description of what you want to generate');
      return;
    }

    setIsGenerating(true);
    setStreamingSessionId(null);

    try {
      const sessionId = await invoke('generate_document_stream', {
        prompt: generateInput,
        format: outputFormat,
      }) as string;

      debugLog("🔥 Streaming session started:", sessionId);
      setStreamingSessionId(sessionId);

      // Track activity
      try {
        await invoke("track_activity", {
          activityType: "document_generated",
          data: `Streaming ${outputFormat.toUpperCase()} document: ${generateInput.substring(0, 50)}...`,
          project: null
        });
      } catch (e) {
        debugLog("Activity tracking skipped:", e);
      }

    } catch (error: any) {
      console.error("❌ Streaming generation failed:", error);
      notify.error('Document generation failed', `${error.message || error}`);
      setIsGenerating(false);
    }
  };

  // Loading Screen with animations
  if (isLoading) {
    return (
      <div className="h-screen flex items-center justify-center transition-colors duration-200" style={{ backgroundColor: colors.bg }}>
        <motion.div
          initial={{ opacity: 0, scale: 0.9 }}
          animate={{ opacity: 1, scale: 1 }}
          transition={{ duration: 0.5 }}
          className="text-center"
        >
          <motion.img
            src="/shodh_logo_nobackground.svg"
            alt="Shodh"
            className="w-32 h-32 mx-auto mb-4"
            animate={{
              scale: [1, 1.1, 1],
              opacity: [0.7, 1, 0.7]
            }}
            transition={{
              duration: 2,
              repeat: Infinity,
              ease: "easeInOut"
            }}
          />
          <motion.h1
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ delay: 0.2 }}
            className="text-2xl font-bold mb-2"
            style={{ color: colors.text }}
          >
            SHODH <span style={{ color: colors.textMuted }}>(शोध)</span>
          </motion.h1>
          <motion.p
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            transition={{ delay: 0.3 }}
            className="mb-6"
            style={{ color: colors.textSecondary }}
          >
            Initializing your knowledge assistant...
          </motion.p>
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            transition={{ delay: 0.4 }}
            className="flex items-center justify-center gap-2"
          >
            <Loader2 className="w-5 h-5 animate-spin" style={{ color: colors.primary }} />
            <span style={{ color: colors.textMuted }}>Loading...</span>
          </motion.div>
        </motion.div>
      </div>
    );
  }

  // Welcome Screen for First Time Users
  if (isFirstTime && sources.length === 0) {
    return (
      <div className="h-screen flex items-center justify-center p-8 transition-colors duration-200" style={{ backgroundColor: colors.bg }}>
        <Card className="max-w-xl w-full card-elevated transition-colors duration-200" style={{ backgroundColor: colors.cardBg, borderColor: colors.cardBorder }}>
          <CardHeader className="text-center pb-2">
            <img src="/shodh_logo_nobackground.svg" alt="Shodh" className="w-14 h-14 mx-auto mb-3" />
            <CardTitle className="text-2xl" style={{ color: colors.text }}>SHODH</CardTitle>
            <CardDescription className="text-sm mt-1" style={{ color: colors.textSecondary }}>
              AI-powered search and analysis for your documents
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-5 pt-2">
            <div className="grid gap-3">
              <div className="flex items-center gap-3 p-3 rounded-lg" style={{ backgroundColor: colors.bgSecondary }}>
                <FolderOpen className="w-5 h-5 flex-shrink-0" style={{ color: colors.primary }} />
                <div>
                  <h3 className="font-medium text-sm" style={{ color: colors.text }}>Add a folder of documents</h3>
                  <p className="text-xs" style={{ color: colors.textSecondary }}>PDF, DOCX, XLSX, PPTX, TXT, MD, CSV</p>
                </div>
              </div>
              <div className="flex items-center gap-3 p-3 rounded-lg" style={{ backgroundColor: colors.bgSecondary }}>
                <MessageSquare className="w-5 h-5 flex-shrink-0" style={{ color: colors.primary }} />
                <div>
                  <h3 className="font-medium text-sm" style={{ color: colors.text }}>Ask questions in natural language</h3>
                  <p className="text-xs" style={{ color: colors.textSecondary }}>Get answers with source citations</p>
                </div>
              </div>
              <div className="flex items-center gap-3 p-3 rounded-lg" style={{ backgroundColor: colors.bgSecondary }}>
                <Bot className="w-5 h-5 flex-shrink-0" style={{ color: colors.primary }} />
                <div>
                  <h3 className="font-medium text-sm" style={{ color: colors.text }}>AI agents for deep analysis</h3>
                  <p className="text-xs" style={{ color: colors.textSecondary }}>Build teams of agents to research and report</p>
                </div>
              </div>
            </div>

            <motion.button
              className="w-full px-4 py-3 rounded-lg font-semibold flex items-center justify-center"
              style={{ backgroundColor: colors.primary, color: colors.primaryText }}
              onClick={() => {
                setIsFirstTime(false);
                handleAddSource();
              }}
              whileHover={{ scale: 1.02 }}
              whileTap={{ scale: 0.98 }}
            >
              <FolderOpen className="w-5 h-5 mr-2" />
              Add Documents
            </motion.button>
            <motion.button
              className="w-full px-4 py-2 rounded-lg font-medium"
              style={{ color: colors.textSecondary }}
              onClick={() => setIsFirstTime(false)}
              whileHover={{ scale: 1.02 }}
              whileTap={{ scale: 0.98 }}
            >
              Skip — I'll explore first
            </motion.button>

            <p className="text-xs text-center" style={{ color: colors.textMuted }}>
              100% local — your data never leaves your machine
            </p>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div
      className="h-screen flex transition-colors duration-200"
      style={{ backgroundColor: colors.bg, color: colors.text }}
    >
      {/* Left Sidebar */}
      <AppSidebar
        activeView={activeTab}
        onViewChange={setActiveTab}
        conversations={conversations}
        activeConversationId={activeConversationId}
        onSelectConversation={(id: string) => { switchConversation(id); setActiveTab('chat'); }}
        onNewConversation={handleNewConversation}
        onDeleteConversation={deleteConversation}
        onRenameConversation={renameConversation}
        onPinConversation={pinConversation}
        onReorderConversations={reorderConversations}
        sources={sources}
        onToggleSource={toggleSource}
        onAddSource={() => handleAddSource()}
        onRemoveSource={removeSource}
        expandedSources={expandedSources}
        sourceFiles={sourceFiles}
        onToggleSourceExpansion={toggleSourceExpansion}
        getFileIconInfo={getFileIconInfo}
        llmStatus={llmStatus}
        onOpenLLMSettings={() => setShowLLMSettings(true)}
        onOpenCommandPalette={openPalette}
        onShowFeedback={() => setShowFeedback(true)}
        stats={stats}
      />


      {/* Main Content Area */}
      <div className="flex-1 flex flex-col min-w-0">
        {/* Thin Title Bar */}
        <div
          className="flex items-center justify-between px-5 py-2 border-b"
          style={{ borderColor: colors.border, backgroundColor: colors.bg }}
        >
          <div className="flex items-center gap-3">
            <span className="text-xs font-semibold tracking-wide" style={{ color: colors.text }}>
              {activeTab === 'chat' ? 'Chat' : activeTab === 'generate' ? 'Generate' : activeTab === 'integrations' ? 'Integrations' : activeTab === 'analytics' ? 'Analytics' : activeTab === 'calendar' ? 'Tasks' : activeTab === 'graph' ? 'Knowledge Graph' : activeTab === 'agents' ? 'AI Agents' : 'Documents'}
            </span>
            {activeTab === 'chat' && activeConversation?.spaceName && (() => {
              const name = activeConversation.spaceName!;
              // FNV-1a hash — must match sourceColor() in AppSidebar / ConversationList
              let hash = 2166136261;
              for (let i = 0; i < name.length; i++) {
                hash ^= name.charCodeAt(i);
                hash = (hash * 16777619) >>> 0;
              }
              const hue = (hash * 137.508) % 360;
              const s = 0.6, l = 0.5;
              const a = s * Math.min(l, 1 - l);
              const f = (n: number) => {
                const k = (n + hue / 30) % 12;
                const c = l - a * Math.max(Math.min(k - 3, 9 - k, 1), -1);
                return Math.round(255 * c).toString(16).padStart(2, '0');
              };
              const clr = `#${f(0)}${f(8)}${f(4)}`;
              return (
                <span
                  className="text-[10px] px-2 py-0.5 rounded-full font-medium truncate max-w-[120px]"
                  style={{ backgroundColor: `${clr}18`, color: clr, border: `1px solid ${clr}30` }}
                  title={`Source: ${name}`}
                >
                  {name}
                </span>
              );
            })()}
            {llmStatus.connected && (
              <span
                className="text-[10px] px-2 py-0.5 rounded-full font-medium"
                style={{ backgroundColor: `${colors.success}18`, color: colors.success }}
              >
                {llmStatus.model}
              </span>
            )}
          </div>
          <div className="flex items-center gap-2">
            <NotificationCenter
              notifications={notifications}
              unreadCount={unreadCount}
              onMarkRead={markNotifRead}
              onMarkAllRead={markAllNotifsRead}
              onRemove={removeNotif}
              onClearAll={clearAllNotifs}
            />
            {activeTab === 'chat' && messages.length > 0 && (
              <>
                <span
                  className="text-[10px] px-2 py-0.5 rounded-full font-medium"
                  style={{ backgroundColor: `${colors.secondary}14`, color: colors.secondary }}
                >
                  {messages.length} msgs
                </span>
                <span
                  className="text-[10px] px-2 py-0.5 rounded-full font-medium"
                  style={{ backgroundColor: colors.bgTertiary, color: colors.textMuted }}
                  title="Estimated token usage for this conversation"
                >
                  ~{Math.round(messages.reduce((sum, m) => sum + m.content.length, 0) / 4).toLocaleString()} tokens
                </span>
                <button
                  onClick={async () => {
                    try {
                      const md = messages.map(m =>
                        `**${m.role === 'user' ? 'You' : 'Shodh'}** (${new Date(m.timestamp || Date.now()).toLocaleString()}):\n\n${m.content}`
                      ).join('\n\n---\n\n');
                      const filePath = await save({
                        defaultPath: `chat-export-${new Date().toISOString().slice(0, 10)}.md`,
                        filters: [
                          { name: 'Markdown', extensions: ['md'] },
                          { name: 'Text', extensions: ['txt'] },
                        ],
                      });
                      if (filePath) {
                        await writeTextFile(filePath, md);
                        notify.success('Chat exported', { description: filePath });
                      }
                    } catch {
                      notify.error('Failed to export chat');
                    }
                  }}
                  className="text-[10px] px-2 py-0.5 rounded-full font-medium transition-colors"
                  style={{ backgroundColor: colors.bgTertiary, color: colors.textMuted }}
                  title="Export chat as Markdown"
                >
                  <Download className="w-3 h-3 inline mr-0.5" />
                  Export
                </button>
              </>
            )}
            {activeTab === 'chat' && activeConversationId && (
              <div className="relative">
                <button
                  onClick={() => {
                    setShowSystemPromptEditor(!showSystemPromptEditor);
                    setEditingInstructionIdx(null);
                    setNewInstructionText('');
                  }}
                  className="text-[10px] px-2 py-0.5 rounded-full font-medium transition-colors"
                  style={{
                    backgroundColor: instructionsList.length > 0 ? `${colors.primary}14` : colors.bgTertiary,
                    color: instructionsList.length > 0 ? colors.primary : colors.textMuted,
                  }}
                  title={instructionsList.length > 0 ? `${instructionsList.length} instruction(s) active` : 'Set custom instructions for this chat'}
                >
                  <Settings className="w-3 h-3 inline mr-0.5" />
                  {instructionsList.length > 0 ? `Instructions (${instructionsList.length})` : 'Add Instructions'}
                </button>
                {showSystemPromptEditor && (
                  <>
                    <div className="fixed inset-0 z-40" onClick={() => { setShowSystemPromptEditor(false); setEditingInstructionIdx(null); }} />
                    <div
                      className="absolute right-0 top-8 z-50 w-96 rounded-lg border shadow-xl"
                      style={{ backgroundColor: colors.bgSecondary, borderColor: colors.border }}
                    >
                      <div className="px-3 py-2 border-b flex items-center justify-between" style={{ borderColor: colors.border }}>
                        <div>
                          <span className="text-xs font-semibold" style={{ color: colors.text }}>Custom Instructions</span>
                          <p className="text-[10px] mt-0.5" style={{ color: colors.textMuted }}>
                            Applied to every AI response in this chat
                          </p>
                        </div>
                        <button
                          onClick={() => { setShowSystemPromptEditor(false); setEditingInstructionIdx(null); }}
                          className="w-5 h-5 rounded flex items-center justify-center"
                          style={{ color: colors.textMuted }}
                        >
                          <X className="w-3.5 h-3.5" />
                        </button>
                      </div>

                      {/* Saved instructions list */}
                      <div className="max-h-48 overflow-y-auto">
                        {instructionsList.length === 0 ? (
                          <div className="px-3 py-4 text-center">
                            <p className="text-[11px]" style={{ color: colors.textMuted }}>No instructions yet</p>
                            <p className="text-[10px] mt-0.5" style={{ color: colors.textTertiary }}>Add instructions below to guide AI responses</p>
                          </div>
                        ) : (
                          <div className="py-1">
                            {instructionsList.map((instruction, idx) => (
                              <div
                                key={idx}
                                className="flex items-start gap-2 px-3 py-1.5 group transition-colors"
                                style={{ backgroundColor: editingInstructionIdx === idx ? `${colors.primary}08` : 'transparent' }}
                                onMouseEnter={e => { if (editingInstructionIdx !== idx) e.currentTarget.style.backgroundColor = colors.bgTertiary; }}
                                onMouseLeave={e => { if (editingInstructionIdx !== idx) e.currentTarget.style.backgroundColor = 'transparent'; }}
                              >
                                <span className="text-[10px] mt-0.5 shrink-0 font-bold" style={{ color: colors.primary }}>•</span>
                                {editingInstructionIdx === idx ? (
                                  <div className="flex-1 flex items-center gap-1">
                                    <input
                                      type="text"
                                      value={editingInstructionText}
                                      onChange={e => setEditingInstructionText(e.target.value)}
                                      onKeyDown={e => {
                                        if (e.key === 'Enter') commitEditInstruction();
                                        if (e.key === 'Escape') { setEditingInstructionIdx(null); setEditingInstructionText(''); }
                                      }}
                                      autoFocus
                                      className="flex-1 text-[11px] bg-transparent border-b outline-none py-0.5"
                                      style={{ color: colors.text, borderColor: colors.primary }}
                                    />
                                    <button
                                      onClick={commitEditInstruction}
                                      className="shrink-0 p-0.5 rounded"
                                      title="Save"
                                    >
                                      <Check className="w-3 h-3" style={{ color: colors.success }} />
                                    </button>
                                    <button
                                      onClick={() => { setEditingInstructionIdx(null); setEditingInstructionText(''); }}
                                      className="shrink-0 p-0.5 rounded"
                                      title="Cancel"
                                    >
                                      <X className="w-3 h-3" style={{ color: colors.textMuted }} />
                                    </button>
                                  </div>
                                ) : (
                                  <>
                                    <span className="flex-1 text-[11px] leading-snug" style={{ color: colors.textSecondary }}>
                                      {instruction}
                                    </span>
                                    <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity shrink-0">
                                      <button
                                        onClick={() => { setEditingInstructionIdx(idx); setEditingInstructionText(instruction); }}
                                        className="p-0.5 rounded transition-colors"
                                        style={{ color: colors.textMuted }}
                                        title="Edit"
                                      >
                                        <Pencil className="w-3 h-3" />
                                      </button>
                                      <button
                                        onClick={() => { removeInstruction(idx); notify.success('Instruction removed'); }}
                                        className="p-0.5 rounded transition-colors"
                                        style={{ color: colors.error }}
                                        title="Remove"
                                      >
                                        <Trash2 className="w-3 h-3" />
                                      </button>
                                    </div>
                                  </>
                                )}
                              </div>
                            ))}
                          </div>
                        )}
                      </div>

                      {/* Add new instruction */}
                      <div className="px-3 py-2 border-t" style={{ borderColor: colors.border }}>
                        <div className="flex items-center gap-1.5">
                          <input
                            type="text"
                            value={newInstructionText}
                            onChange={e => setNewInstructionText(e.target.value)}
                            onKeyDown={e => { if (e.key === 'Enter') addInstruction(); }}
                            placeholder="Add an instruction..."
                            className="flex-1 text-[11px] rounded-md border px-2 py-1.5 outline-none"
                            style={{
                              backgroundColor: colors.inputBg,
                              borderColor: colors.border,
                              color: colors.text,
                            }}
                          />
                          <button
                            onClick={addInstruction}
                            disabled={!newInstructionText.trim()}
                            className="px-2 py-1.5 rounded-md text-[10px] font-medium text-white transition-colors disabled:opacity-40"
                            style={{ backgroundColor: colors.primary }}
                          >
                            <Plus className="w-3 h-3" />
                          </button>
                        </div>
                        {instructionsList.length > 0 && (
                          <button
                            onClick={() => {
                              saveInstructions([]);
                              notify.success('All instructions cleared');
                            }}
                            className="text-[10px] mt-1.5 transition-colors"
                            style={{ color: colors.error }}
                          >
                            Clear all
                          </button>
                        )}
                      </div>
                    </div>
                  </>
                )}
              </div>
            )}
            <Badge variant="outline" className="text-[10px] px-2 py-0.5 h-auto" style={{ borderColor: colors.border, color: colors.textMuted }}>
              <Database className="w-3 h-3 mr-1" />
              {sources.filter(s => s.selected).length} sources
            </Badge>
          </div>
        </div>

        {/* Content Area */}
        <div className="flex-1 overflow-hidden">
          {/* Chat Tab */}
          {activeTab === 'chat' && (
            <div className="h-full flex relative">
              {/* Messages Section (full width — artifacts overlay as drawer) */}
              <div className="flex-1 flex flex-col">
                {/* Telegram Status Bar */}
                {isTelegramBotActive && (
                  <div className="px-6 py-2 border-b" style={{ backgroundColor: colors.cardBg, borderColor: '#0088cc' }}>
                    <div className="flex items-center gap-2 text-xs">
                      <div className="w-2 h-2 rounded-full bg-green-500 animate-pulse"></div>
                      <span style={{ color: colors.text }}>
                        Telegram Bot Active - Messages will appear here
                      </span>
                    </div>
                  </div>
                )}
                <div
                ref={chatScrollContainerRef}
                className="flex-1 overflow-y-auto relative"
                style={{ backgroundColor: colors.bg }}
                onDrop={handleImageDrop}
                onDragOver={handleDragOver}
                onDragEnter={handleDragEnter}
                onDragLeave={handleDragLeave}
              >
                {/* Global drag overlay */}
                {isDraggingImage && (
                  <div
                    className="absolute inset-0 z-50 flex items-center justify-center rounded-lg border-4 border-dashed pointer-events-none"
                    style={{
                      background: 'rgba(59, 130, 246, 0.1)',
                      borderColor: colors.primary
                    }}
                  >
                    <div className="text-center p-8 rounded-lg" style={{ background: colors.bgSecondary }}>
                      <p className="text-2xl font-bold mb-2" style={{ color: colors.primary }}>📄 Drop File to Index</p>
                      <p className="text-sm" style={{ color: colors.textMuted }}>Supports: PDF, Word, Excel, PowerPoint, Images</p>
                    </div>
                  </div>
                )}

                <div className={messages.length === 0 ? "min-h-full flex items-center justify-center" : "p-6"}>
                {messages.length === 0 ? (() => {
                  const selectedSources = sources.filter(s => s.selected);
                  const firstName = selectedSources[0]?.name || 'your documents';
                  const suggestions = [
                    { icon: FileText, label: 'Summarize', desc: 'Key findings & takeaways', query: `Summarize the key findings from ${firstName}`, color: colors.primary },
                    { icon: Sparkles, label: 'Compare', desc: 'Across your documents', query: 'Compare themes across my documents', color: colors.success },
                    { icon: Search, label: 'Extract', desc: 'Find specific information', query: 'Find specific information about', color: colors.warning },
                    { icon: BarChart, label: 'Analyze', desc: 'Patterns & insights', query: 'What patterns emerge from my sources?', color: colors.secondary },
                  ];

                  return (
                    <div className="w-full max-w-2xl mx-auto px-6" style={{ marginTop: '-5%' }}>
                      {/* Brand */}
                      <div className="text-center mb-8">
                        <div
                          className="w-14 h-14 rounded-2xl flex items-center justify-center mx-auto mb-4"
                          style={{ backgroundColor: `${colors.primary}12` }}
                        >
                          <Search className="w-7 h-7" style={{ color: colors.primary }} />
                        </div>
                        <h1 className="text-2xl font-bold mb-1" style={{ color: colors.text }}>Shodh</h1>
                        <p className="text-sm" style={{ color: colors.textMuted }}>Search your knowledge base</p>
                      </div>

                      {/* Centered Search Bar */}
                      <div
                        className="rounded-xl overflow-hidden mb-4 transition-all"
                        style={{
                          backgroundColor: colors.cardBg,
                          border: `1px solid ${colors.border}`,
                          boxShadow: '0 4px 24px rgba(0,0,0,0.08), 0 1px 4px rgba(0,0,0,0.04)',
                        }}
                      >
                        <div className="flex items-start px-4 py-3 gap-3">
                          <Search className="w-5 h-5 shrink-0 mt-0.5" style={{ color: colors.textMuted }} />
                          <textarea
                            rows={1}
                            placeholder={llmStatus.connected ? "Ask anything about your documents..." : "Search your documents..."}
                            value={currentInput}
                            onChange={(e) => {
                              setCurrentInput(e.target.value);
                              e.target.style.height = 'auto';
                              e.target.style.height = Math.min(e.target.scrollHeight, 120) + 'px';
                            }}
                            onKeyDown={(e) => {
                              if (e.key === 'Enter' && !e.shiftKey && currentInput.trim()) {
                                e.preventDefault();
                                handleSendMessageUnified();
                              }
                            }}
                            disabled={isProcessing}
                            className="flex-1 bg-transparent outline-none text-base disabled:opacity-50 resize-none"
                            style={{ color: colors.text, maxHeight: '120px', overflowY: 'auto' }}
                            autoFocus
                          />
                          <div className="flex items-center gap-1 shrink-0">
                            <button
                              onClick={handlePickImage}
                              disabled={isProcessing}
                              className="p-2 rounded-lg transition-colors disabled:opacity-40"
                              style={{ color: colors.textMuted }}
                              onMouseEnter={e => (e.currentTarget.style.backgroundColor = colors.bgHover)}
                              onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
                              title="Upload image"
                            >
                              <ImageIcon className="w-4 h-4" />
                            </button>
                            {('webkitSpeechRecognition' in window || 'SpeechRecognition' in window) && (
                              <button
                                onClick={toggleVoiceInput}
                                disabled={isProcessing}
                                className="p-2 rounded-lg transition-colors relative disabled:opacity-40"
                                style={{
                                  color: isListening ? colors.error : colors.textMuted,
                                  backgroundColor: isListening ? `${colors.error}12` : 'transparent',
                                }}
                                onMouseEnter={e => { if (!isListening) e.currentTarget.style.backgroundColor = colors.bgHover; }}
                                onMouseLeave={e => { if (!isListening) e.currentTarget.style.backgroundColor = 'transparent'; }}
                                title={isListening ? 'Stop listening' : 'Voice input'}
                              >
                                {isListening ? <MicOff className="w-4 h-4" /> : <Mic className="w-4 h-4" />}
                                {isListening && (
                                  <span className="absolute -top-0.5 -right-0.5 w-2 h-2 rounded-full animate-pulse" style={{ backgroundColor: colors.error }} />
                                )}
                              </button>
                            )}
                            <button
                              onClick={() => handleSendMessageUnified()}
                              disabled={!currentInput.trim() || isProcessing}
                              className="p-2 rounded-lg transition-colors disabled:opacity-30"
                              style={{
                                backgroundColor: currentInput.trim() ? colors.primary : 'transparent',
                                color: currentInput.trim() ? '#fff' : colors.textMuted,
                              }}
                              title="Send"
                            >
                              {isProcessing ? <Loader2 className="w-4 h-4 animate-spin" /> : <Send className="w-4 h-4" />}
                            </button>
                          </div>
                        </div>
                      </div>

                      {/* Search mode chips */}
                      <div className="flex items-center justify-center gap-2 mb-8">
                        <span
                          className="text-[10px] px-2.5 py-1 rounded-full font-medium"
                          style={{ backgroundColor: colors.bgTertiary, color: colors.textMuted }}
                        >
                          {searchConfig.searchMode === 'hybrid' ? 'Hybrid Search' : searchConfig.searchMode === 'semantic' ? 'Semantic Search' : 'Keyword Search'}
                        </span>
                        <span
                          className="text-[10px] px-2.5 py-1 rounded-full font-medium"
                          style={{ backgroundColor: colors.bgTertiary, color: colors.textMuted }}
                        >
                          {searchConfig.maxResults} results
                        </span>
                        {selectedSources.length > 0 && (
                          <span
                            className="text-[10px] px-2.5 py-1 rounded-full font-medium"
                            style={{ backgroundColor: colors.bgTertiary, color: colors.textMuted }}
                          >
                            {selectedSources.length} source{selectedSources.length !== 1 ? 's' : ''}
                          </span>
                        )}
                        {llmStatus.connected && (
                          <span
                            className="text-[10px] px-2.5 py-1 rounded-full font-medium"
                            style={{ backgroundColor: `${colors.success}12`, color: colors.success }}
                          >
                            AI: {llmStatus.model || 'Connected'}
                          </span>
                        )}
                      </div>

                      {/* Suggestion categories — 2x2 grid */}
                      <div className="grid grid-cols-2 gap-3 mb-8">
                        {suggestions.map(s => {
                          const Icon = s.icon;
                          return (
                            <button
                              key={s.label}
                              onClick={() => {
                                setCurrentInput(s.query);
                                // Focus the textarea after a tick so React updates first
                                setTimeout(() => {
                                  const ta = document.querySelector('textarea[autofocus]') as HTMLTextAreaElement;
                                  if (ta) { ta.focus(); ta.setSelectionRange(s.query.length, s.query.length); }
                                }, 50);
                              }}
                              className="text-left p-3.5 rounded-xl transition-all group"
                              style={{
                                backgroundColor: colors.cardBg,
                                border: `1px solid ${colors.border}`,
                                boxShadow: 'none',
                                transform: 'translateY(0)',
                                transition: 'all 0.2s ease',
                              }}
                              onMouseEnter={e => {
                                e.currentTarget.style.borderColor = `${s.color}50`;
                                e.currentTarget.style.backgroundColor = `${s.color}06`;
                                e.currentTarget.style.boxShadow = `0 4px 16px ${s.color}12`;
                                e.currentTarget.style.transform = 'translateY(-2px)';
                              }}
                              onMouseLeave={e => {
                                e.currentTarget.style.borderColor = colors.border;
                                e.currentTarget.style.backgroundColor = colors.cardBg;
                                e.currentTarget.style.boxShadow = 'none';
                                e.currentTarget.style.transform = 'translateY(0)';
                              }}
                            >
                              <div className="flex items-center gap-2.5 mb-1.5">
                                <div
                                  className="w-7 h-7 rounded-lg flex items-center justify-center shrink-0"
                                  style={{ backgroundColor: `${s.color}14`, color: s.color }}
                                >
                                  <Icon className="w-3.5 h-3.5" />
                                </div>
                                <span className="text-xs font-semibold" style={{ color: colors.text }}>{s.label}</span>
                              </div>
                              <p className="text-[10px] leading-relaxed pl-[38px]" style={{ color: colors.textMuted }}>{s.desc}</p>
                            </button>
                          );
                        })}
                      </div>

                      {/* Active sources strip */}
                      <div className="flex flex-wrap items-center justify-center gap-2">
                        {selectedSources.length > 0 ? selectedSources.map(s => (
                          <span
                            key={s.id}
                            className="flex items-center gap-1.5 text-[10px] px-2 py-1 rounded-full"
                            style={{
                              backgroundColor: `${sourceColor(s.name)}10`,
                              color: sourceColor(s.name),
                              border: `1px solid ${sourceColor(s.name)}25`,
                            }}
                          >
                            <span className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: sourceColor(s.name) }} />
                            {s.name}
                          </span>
                        )) : (
                          <span className="text-[11px]" style={{ color: colors.textMuted }}>
                            No sources indexed — add a folder to get started
                          </span>
                        )}
                      </div>
                    </div>
                  );
                })() : (
                  <div className="max-w-4xl mx-auto space-y-4">
                    <AnimatePresence mode="popLayout">
                      {messages.map(msg => (
                        <motion.div
                          key={msg.id}
                          initial={{ opacity: 0, y: 20 }}
                          animate={{ opacity: 1, y: 0 }}
                          exit={{ opacity: 0, scale: 0.95 }}
                          transition={{ duration: 0.3, ease: "easeOut" }}
                          className="w-full py-3 px-4 rounded-xl group/msg relative"
                          style={{
                            background: msg.role === 'user'
                              ? (theme === 'dark' ? 'rgba(59, 130, 246, 0.06)' : 'rgba(59, 130, 246, 0.04)')
                              : msg.role === 'system'
                                ? (theme === 'dark' ? 'rgba(245, 158, 11, 0.06)' : 'rgba(245, 158, 11, 0.04)')
                                : 'transparent',
                            borderLeft: msg.role === 'assistant' ? `3px solid ${theme === 'dark' ? 'rgba(16, 185, 129, 0.4)' : 'rgba(16, 185, 129, 0.3)'}` : 'none',
                            border: msg.role !== 'assistant' ? `1px solid ${msg.role === 'user' ? (theme === 'dark' ? 'rgba(59, 130, 246, 0.10)' : 'rgba(59, 130, 246, 0.12)') : (theme === 'dark' ? 'rgba(245, 158, 11, 0.10)' : 'rgba(245, 158, 11, 0.12)')}` : undefined,
                            borderTop: msg.role === 'assistant' ? `1px solid ${colors.border}` : undefined,
                            borderRight: msg.role === 'assistant' ? `1px solid ${colors.border}` : undefined,
                            borderBottom: msg.role === 'assistant' ? `1px solid ${colors.border}` : undefined,
                          }}
                        >
                          {/* Message Header */}
                          <div className="flex items-center justify-between mb-2">
                            <div className="flex items-center gap-2">
                              <div
                                className="w-6 h-6 rounded-full flex items-center justify-center"
                                style={{
                                  background: msg.role === 'user'
                                    ? '#3b82f6'
                                    : msg.role === 'system'
                                      ? '#f59e0b'
                                      : '#10b981',
                                }}
                              >
                                <span className="text-[10px] font-bold text-white">
                                  {msg.role === 'user' ? 'U' : msg.role === 'system' ? 'S' : 'AI'}
                                </span>
                              </div>
                              <span
                                className="text-[11px] font-bold tracking-wide"
                                style={{
                                  color: msg.role === 'user'
                                    ? '#3b82f6'
                                    : msg.role === 'system'
                                      ? '#f59e0b'
                                      : '#10b981'
                                }}
                              >
                                {msg.role === 'user' ? 'You' : msg.role === 'system' ? 'System' : 'Shodh AI'}
                              </span>
                            </div>
                            <div className="flex items-center gap-2.5">
                              <span className="text-[11px] tabular-nums font-medium" style={{ color: colors.textSecondary }}>
                                {new Date(msg.timestamp).toLocaleDateString([], { month: 'short', day: 'numeric' })}
                                {' '}
                                <span style={{ color: colors.textMuted }}>
                                  {new Date(msg.timestamp).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
                                </span>
                              </span>
                              {msg.role === 'assistant' && (
                                <CopyButton text={msg.content} isDark={theme === 'dark'} label="Copy" size="sm" />
                              )}
                            </div>
                          </div>

                          {/* Message Content */}
                          <div>
                            {/* Display image if present */}
                            {msg.image && (
                              <div className="mb-3">
                                <img
                                  src={msg.image}
                                  alt="Pasted image"
                                  className="max-w-full rounded-lg border"
                                  style={{
                                    borderColor: colors.border,
                                    maxHeight: '400px',
                                    objectFit: 'contain'
                                  }}
                                />
                              </div>
                            )}

                            <MessageContentRenderer
                              content={msg.content}
                              searchResults={msg.searchResults}
                              artifacts={msg.artifacts}
                              onFollowUpQuery={(query) => {
                                setCurrentInput(query);
                                setTimeout(() => handleSendMessageUnified(), 100);
                              }}
                              onOpenUrl={handleOpenUrl}
                              onViewInArtifact={async (citation) => {
                                try {
                                  debugLog('📖 Loading document into artifact panel:', citation);

                                  const fileName = citation.sourceFile.split(/[/\\]/).pop() || 'Document';
                                  const isPdf = fileName.toLowerCase().endsWith('.pdf');
                                  const isDoc = /\.(doc|docx|ppt|pptx|xls|xlsx)$/i.test(fileName);
                                  const isTextFile = /\.(txt|md|json|csv|html?)$/i.test(fileName);

                                  // For PDFs and Office docs, create a PDF artifact that embeds the file
                                  if (isPdf || isDoc) {
                                    const newArtifact = {
                                      id: `citation-doc-${Date.now()}`,
                                      artifact_type: { PDF: null },
                                      title: `📄 ${citation.citationTitle}`,
                                      content: citation.sourceFile, // Store file path in content
                                      editable: false,
                                      version: 1,
                                      created_at: new Date().toISOString(),
                                      metadata: {
                                        filePath: citation.sourceFile,
                                        pageNumber: citation.pageNumber,
                                        snippet: citation.snippet,
                                        lineRange: citation.lineRange
                                      }
                                    };

                                    debugLog('🎨 Creating PDF artifact:', {
                                      id: newArtifact.id,
                                      title: newArtifact.title,
                                      filePath: citation.sourceFile
                                    });

                                    setArtifacts(prev => [...prev, newArtifact]);
                                    setSelectedArtifactId(newArtifact.id);
                                    setShowArtifacts(true);
                                    debugLog('✅ PDF artifact created');
                                    return;
                                  }

                                  // For text files, read and display as markdown
                                  if (isTextFile) {
                                    try {
                                      const documentContent = await readTextFile(citation.sourceFile);
                                      debugLog('✅ Read text file:', documentContent.length, 'chars');

                                      const newArtifact = {
                                        id: `citation-doc-${Date.now()}`,
                                        artifact_type: { Markdown: null },
                                        title: `📄 ${citation.citationTitle}`,
                                        content: `# ${citation.citationTitle}\n\n**Source:** \`${fileName}\`\n\n---\n\n\`\`\`\n${documentContent}\n\`\`\``,
                                        editable: false,
                                        version: 1,
                                        created_at: new Date().toISOString()
                                      };

                                      setArtifacts(prev => [...prev, newArtifact]);
                                      setSelectedArtifactId(newArtifact.id);
                                      setShowArtifacts(true);
                                      debugLog('✅ Text file loaded into artifact panel');
                                    } catch (error) {
                                      console.error('Failed to read text file:', error);
                                      throw new Error(`Failed to read file: ${error}`);
                                    }
                                  }
                                } catch (error) {
                                  console.error('❌ Failed to load document:', error);
                                  notify.error('Failed to load document', `${error}`);
                                }
                              }}
                              onOpenArtifact={(artifactId) => {
                                setSelectedArtifactId(artifactId);
                                setShowArtifacts(true);
                              }}
                              textColor={colors.text}
                            />

                            {/* Tool Call Invocations — show inline when agent used tools */}
                            {msg.toolInvocations && msg.toolInvocations.length > 0 && (
                              <ToolCallBubble
                                invocations={msg.toolInvocations}
                                colors={colors}
                                theme={theme}
                              />
                            )}

                            {/* Claude-style Thinking Animation - Show while searching for CURRENT message */}
                            {msg.role === 'system' && msg.id === messages[messages.length - 1]?.id && isProcessing && searchPipeline.stage !== 'idle' && (
                              <motion.div
                                initial={{ opacity: 0, height: 0 }}
                                animate={{ opacity: 1, height: 'auto' }}
                                className="mt-4 p-4 rounded-xl space-y-3"
                                style={{
                                  background: theme === 'dark' ? 'rgba(16, 185, 129, 0.1)' : '#ecfdf5',
                                  border: theme === 'dark' ? '1px solid rgba(16, 185, 129, 0.2)' : '1px solid #a7f3d0'
                                }}
                              >
                                  {/* Claude Code-style minimal animation - 1-2 lines only */}
                                  <div className="flex items-center justify-between gap-3">
                                    <div className="flex items-center gap-3">
                                      {/* Animated logo - spinning outer ring + pulsing inner dot */}
                                      <div className="relative w-3.5 h-3.5 flex-shrink-0">
                                        {/* Outer spinning ring */}
                                        <motion.div
                                          animate={{ rotate: 360 }}
                                          transition={{ duration: 1.5, repeat: Infinity, ease: "linear" }}
                                          className="absolute inset-0 rounded-full border-2 border-green-500 border-t-transparent"
                                        />
                                        {/* Inner pulsing dot */}
                                        <motion.div
                                          animate={{
                                            scale: [0.6, 1, 0.6],
                                            opacity: [0.5, 1, 0.5]
                                          }}
                                          transition={{ duration: 1.5, repeat: Infinity, ease: "easeInOut" }}
                                          className="absolute inset-[5px] rounded-full bg-green-500"
                                        />
                                      </div>
                                      <span className="text-sm" style={{ color: '#10b981', fontFamily: 'monospace' }}>
                                        {searchPipeline.stage === 'bm25' && 'Searching keywords...'}
                                        {searchPipeline.stage === 'vector' && 'Semantic search...'}
                                        {searchPipeline.stage === 'neural' && 'Reranking results...'}
                                        {searchPipeline.stage === 'graph' && 'Graph analysis...'}
                                        {searchPipeline.stage === 'complete' && 'Complete'}
                                        {searchPipeline.stage === 'idle' && 'Processing...'}
                                      </span>
                                      <span className="text-xs" style={{ color: colors.textMuted, fontFamily: 'monospace' }}>
                                        {searchPipeline.progress}%
                                      </span>
                                    </div>
                                    {/* ESC to cancel hint */}
                                    <span className="text-xs px-2 py-0.5 rounded" style={{
                                      color: colors.textMuted,
                                      fontFamily: 'monospace',
                                      background: theme === 'dark' ? 'rgba(255,255,255,0.05)' : 'rgba(0,0,0,0.05)',
                                      border: `1px solid ${theme === 'dark' ? 'rgba(255,255,255,0.1)' : 'rgba(0,0,0,0.1)'}`
                                    }}>
                                      ESC to cancel
                                    </span>
                                  </div>
                              </motion.div>
                            )}

                            {/* Search metadata removed for cleaner UI */}

                            {msg.sources && msg.sources.length > 0 && (
                              <motion.div
                                initial={{ opacity: 0, height: 0 }}
                                animate={{ opacity: 1, height: 'auto' }}
                                transition={{ delay: 0.2, duration: 0.3 }}
                                className="mt-3 pt-3 border-t border-gray-300"
                              >
                                <p className="text-[10px] text-gray-800 mb-2 font-bold tracking-wider">📚 SOURCES</p>
                                <div className="flex flex-wrap gap-2">
                                  {msg.sources.map((source, idx) => (
                                    <motion.div
                                      key={idx}
                                      initial={{ opacity: 0, scale: 0.8 }}
                                      animate={{ opacity: 1, scale: 1 }}
                                      transition={{ delay: 0.3 + idx * 0.05 }}
                                    >
                                      <Badge variant="outline" className="text-xs bg-blue-100 border-blue-400 text-blue-900 font-medium">
                                        {source.file}
                                      </Badge>
                                    </motion.div>
                                  ))}
                                </div>
                              </motion.div>
                            )}
                          </div>
                        </motion.div>
                      ))}
                    </AnimatePresence>

                    {/* AI Typing Indicator - Shows when generating response (after search completes) */}
                    {isProcessing && (searchPipeline.stage === 'idle' || !searchPipeline.stage) && (
                      <motion.div
                        initial={{ opacity: 0, y: 20 }}
                        animate={{ opacity: 1, y: 0 }}
                        exit={{ opacity: 0, y: -20 }}
                        transition={{ duration: 0.3 }}
                        className="w-full py-4 px-5 rounded-xl"
                        style={{
                          background: 'transparent',
                          border: `1px solid ${colors.border}`
                        }}
                      >
                        <div className="flex items-center gap-3">
                          <div
                            className="w-6 h-6 rounded-full flex items-center justify-center"
                            style={{ background: '#10b981' }}
                          >
                            <span className="text-[10px] font-bold text-white">AI</span>
                          </div>
                          <div className="flex items-center gap-2">
                            <motion.div
                              className="w-2 h-2 rounded-full bg-green-500"
                              animate={{ opacity: [0.3, 1, 0.3], y: [0, -4, 0] }}
                              transition={{ duration: 1.4, repeat: Infinity, delay: 0 }}
                            />
                            <motion.div
                              className="w-2 h-2 rounded-full bg-green-500"
                              animate={{ opacity: [0.3, 1, 0.3], y: [0, -4, 0] }}
                              transition={{ duration: 1.4, repeat: Infinity, delay: 0.2 }}
                            />
                            <motion.div
                              className="w-2 h-2 rounded-full bg-green-500"
                              animate={{ opacity: [0.3, 1, 0.3], y: [0, -4, 0] }}
                              transition={{ duration: 1.4, repeat: Infinity, delay: 0.4 }}
                            />
                            <span className="ml-2 text-sm font-medium" style={{ color: '#10b981' }}>
                              Thinking and writing...
                            </span>
                            {/* ESC to cancel hint */}
                            <span className="ml-3 text-xs px-2 py-0.5 rounded" style={{
                              color: colors.textMuted,
                              fontFamily: 'monospace',
                              background: theme === 'dark' ? 'rgba(255,255,255,0.05)' : 'rgba(0,0,0,0.05)',
                              border: `1px solid ${theme === 'dark' ? 'rgba(255,255,255,0.1)' : 'rgba(0,0,0,0.1)'}`
                            }}>
                              ESC to cancel
                            </span>
                          </div>
                        </div>
                      </motion.div>
                    )}

                    {/* Follow-up suggestion chips */}
                    {followUpSuggestions.length > 0 && !isProcessing && messages.length > 0 && messages[messages.length - 1]?.role === 'assistant' && !messages[messages.length - 1]?.id.startsWith('streaming-') && (
                      <div className="flex flex-wrap gap-2 mt-3 mb-2 px-4">
                        {followUpSuggestions.map((suggestion, i) => (
                          <button
                            key={i}
                            onClick={() => {
                              setCurrentInput(suggestion);
                              setFollowUpSuggestions([]);
                            }}
                            className="text-xs px-3 py-1.5 rounded-full border transition-all hover:scale-[1.02]"
                            style={{
                              borderColor: `${colors.primary}40`,
                              color: colors.primary,
                              backgroundColor: `${colors.primary}08`,
                            }}
                          >
                            {suggestion}
                          </button>
                        ))}
                      </div>
                    )}

                    <div ref={chatEndRef} />
                  </div>
                )}
                </div>
              </div>

              {/* Input Area — hidden when empty state (search landing) is shown */}
              {messages.length > 0 && <div
                className="border-t p-4 transition-colors duration-200"
                style={{
                  borderColor: colors.border,
                  backgroundColor: colors.bgSecondary
                }}
              >
                <div className="max-w-4xl mx-auto">
                  {/* Processing indicator — only shown when active */}
                  <AnimatePresence>
                    {isProcessing && (
                      <motion.div
                        initial={{ opacity: 0, height: 0 }}
                        animate={{ opacity: 1, height: 'auto' }}
                        exit={{ opacity: 0, height: 0 }}
                        transition={{ duration: 0.2 }}
                        className="mb-2"
                      >
                        <div
                          className="inline-flex items-center gap-2 px-3 py-1 rounded-full text-[11px]"
                          style={{
                            backgroundColor: `${colors.primary}10`,
                            color: colors.primary,
                          }}
                        >
                          <Loader2 className="w-3 h-3 animate-spin" />
                          <span className="font-medium">Thinking...</span>
                        </div>
                      </motion.div>
                    )}
                  </AnimatePresence>
                  <div className="flex gap-2 items-end">
                    <textarea
                      rows={1}
                      placeholder={llmStatus.connected ? "Ask anything..." : "Search your documents..."}
                      value={currentInput}
                      onChange={(e) => {
                        setCurrentInput(e.target.value);
                        e.target.style.height = 'auto';
                        e.target.style.height = Math.min(e.target.scrollHeight, 120) + 'px';
                      }}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter' && !e.shiftKey) {
                          e.preventDefault();
                          handleSendMessageUnified();
                        }
                      }}
                      disabled={isProcessing}
                      className="flex-1 input-field transition-all disabled:opacity-50 text-base resize-none rounded-md px-3 py-2 border outline-none"
                      style={{
                        backgroundColor: colors.inputBg,
                        borderColor: colors.border,
                        color: colors.text,
                        maxHeight: '120px',
                        overflowY: 'auto',
                      }}
                    />
                    <motion.button
                      onClick={handlePickImage}
                      disabled={isProcessing}
                      whileHover={{ scale: 1.05 }}
                      whileTap={{ scale: 0.95 }}
                      className="px-4 py-2 rounded-md font-medium disabled:opacity-50 disabled:cursor-not-allowed transition-all"
                      style={{ backgroundColor: colors.bgTertiary, color: colors.textSecondary }}
                      title="Upload image for OCR"
                    >
                      <ImageIcon className="w-5 h-5" />
                    </motion.button>
                    {('webkitSpeechRecognition' in window || 'SpeechRecognition' in window) && (
                      <motion.button
                        onClick={toggleVoiceInput}
                        disabled={isProcessing}
                        whileHover={{ scale: 1.05 }}
                        whileTap={{ scale: 0.95 }}
                        className="px-4 py-2 rounded-md font-medium disabled:opacity-50 disabled:cursor-not-allowed transition-all relative"
                        style={{
                          backgroundColor: isListening ? `${colors.error}20` : colors.bgTertiary,
                          color: isListening ? colors.error : colors.textSecondary,
                        }}
                        title={isListening ? 'Stop listening' : 'Voice input'}
                      >
                        {isListening ? <MicOff className="w-5 h-5" /> : <Mic className="w-5 h-5" />}
                        {isListening && (
                          <span
                            className="absolute -top-0.5 -right-0.5 w-2.5 h-2.5 rounded-full animate-pulse"
                            style={{ backgroundColor: colors.error }}
                          />
                        )}
                      </motion.button>
                    )}
                    <motion.button
                      onClick={() => handleSendMessageUnified()}
                      disabled={!currentInput.trim() || isProcessing}
                      whileHover={{ scale: 1.05 }}
                      whileTap={{ scale: 0.95 }}
                      className="px-4 py-2 text-white rounded-md font-medium disabled:opacity-50 disabled:cursor-not-allowed transition-all"
                      style={{ backgroundColor: colors.primary }}
                    >
                      {isProcessing ? (
                        <Loader2 className="w-5 h-5 animate-spin" />
                      ) : (
                        <Send className="w-5 h-5" />
                      )}
                    </motion.button>
                  </div>
                </div>
              </div>}
              </div>

              {/* Floating Artifacts Toggle Button */}
              {artifacts.length > 0 && !showArtifacts && (
                <motion.button
                  initial={{ scale: 0, opacity: 0 }}
                  animate={{ scale: 1, opacity: 1 }}
                  exit={{ scale: 0, opacity: 0 }}
                  whileHover={{ scale: 1.04, y: -1 }}
                  whileTap={{ scale: 0.96 }}
                  onClick={() => setShowArtifacts(true)}
                  className="absolute bottom-24 right-6 z-20 flex items-center gap-2.5 pl-3 pr-3.5 py-2 rounded-xl transition-all"
                  style={{
                    backgroundColor: colors.cardBg,
                    border: `1px solid ${colors.border}`,
                    boxShadow: '0 4px 20px rgba(0,0,0,0.12), 0 1px 4px rgba(0,0,0,0.08)',
                  }}
                >
                  <div
                    className="w-7 h-7 rounded-lg flex items-center justify-center shrink-0"
                    style={{ backgroundColor: '#ef444418', color: '#ef4444' }}
                  >
                    <Layers className="w-3.5 h-3.5" />
                  </div>
                  <div className="flex flex-col items-start leading-none">
                    <span className="text-[11px] font-semibold" style={{ color: colors.text }}>Artifacts</span>
                    <span className="text-[9px]" style={{ color: colors.textMuted }}>{artifacts.length} item{artifacts.length !== 1 ? 's' : ''}</span>
                  </div>
                  <div
                    className="w-5 h-5 rounded-full flex items-center justify-center text-[10px] font-bold text-white ml-0.5"
                    style={{ backgroundColor: '#ef4444' }}
                  >
                    {artifacts.length}
                  </div>
                </motion.button>
              )}

              {/* Artifacts Drawer Overlay */}
              <AnimatePresence>
                {artifacts.length > 0 && showArtifacts && (
                  <>
                    <motion.div
                      initial={{ opacity: 0 }}
                      animate={{ opacity: 1 }}
                      exit={{ opacity: 0 }}
                      transition={{ duration: 0.2 }}
                      className="absolute inset-0 z-30"
                      style={{ backgroundColor: 'rgba(0,0,0,0.35)' }}
                      onClick={() => setShowArtifacts(false)}
                    />
                    <motion.div
                      initial={{ x: '100%' }}
                      animate={{ x: 0 }}
                      exit={{ x: '100%' }}
                      transition={{ type: 'spring', damping: 25, stiffness: 200 }}
                      className="absolute right-0 top-0 bottom-0 w-[55%] z-40"
                      style={{ boxShadow: '-8px 0 30px rgba(0,0,0,0.18)' }}
                    >
                      <EnhancedArtifactPanel
                        artifacts={artifacts}
                        theme={theme}
                        selectedArtifactId={selectedArtifactId}
                        onClose={() => setShowArtifacts(false)}
                      />
                    </motion.div>
                  </>
                )}
              </AnimatePresence>
            </div>
          )}

          {/* Integrations Tab */}
          {activeTab === 'integrations' && (
            <IntegrationsPanel spaces={spaces} />
          )}

          {/* Generate Tab */}
          {activeTab === 'generate' && (
            <div className="h-full">
              <DocumentGenerator />
            </div>
          )}

          {/* Analytics Tab */}
          {activeTab === 'analytics' && (
            <AnalyticsDashboard />
          )}

          {/* Calendar/Tasks Tab */}
          {activeTab === 'calendar' && (
            <CalendarTodoPanel />
          )}

          {/* Knowledge Graph Tab */}
          {activeTab === 'graph' && (
            <KnowledgeGraph />
          )}

          {/* Agents Tab */}
          {activeTab === 'agents' && (
            <AgentsPanel />
          )}

          {/* Documents Tab — shows indexed sources with file lists */}
          {activeTab === 'documents' && (
            <div className="h-full overflow-y-auto p-6">
              <div className="max-w-4xl mx-auto">
                <div className="flex items-center justify-between mb-6">
                  <div>
                    <h1 className="text-lg font-bold" style={{ color: colors.text }}>Documents</h1>
                    <p className="text-xs mt-0.5" style={{ color: colors.textMuted }}>
                      {stats.totalDocs} documents indexed across {sources.length} sources
                    </p>
                  </div>
                  <button
                    onClick={() => handleAddSource()}
                    className="px-3 py-1.5 text-xs font-medium rounded-md text-white transition-colors"
                    style={{ backgroundColor: colors.primary }}
                  >
                    Add Source
                  </button>
                </div>

                {sources.length === 0 ? (
                  <EmptyState
                    icon={FileText}
                    title="No document sources"
                    description="Add a document folder to start indexing and searching your files."
                    actions={[
                      { label: 'Add Workspace', onClick: () => handleAddSource(), variant: 'default', icon: FileText },
                    ]}
                    size="md"
                    variant="info"
                  />
                ) : (
                  <div className="space-y-4">
                    {sources.map(source => (
                      <div
                        key={source.id}
                        className="rounded-lg border p-4"
                        style={{ borderColor: colors.border, backgroundColor: colors.cardBg }}
                      >
                        <div className="flex items-center justify-between mb-2">
                          <div className="flex items-center gap-2">
                            <FileText className="w-4 h-4" style={{ color: colors.primary }} />
                            <span className="text-sm font-semibold" style={{ color: colors.text }}>
                              {source.name}
                            </span>
                            <span
                              className="text-[10px] px-1.5 py-0.5 rounded-full font-medium"
                              style={{
                                backgroundColor: source.status === 'ready' ? `${colors.success}18` : `${colors.warning}18`,
                                color: source.status === 'ready' ? colors.success : colors.warning,
                              }}
                            >
                              {source.status}
                            </span>
                          </div>
                          <div className="flex items-center gap-2 text-xs" style={{ color: colors.textMuted }}>
                            <span>{source.fileCount || 0} files</span>
                            {source.indexedAt && (
                              <span>Indexed {new Date(source.indexedAt).toLocaleDateString()}</span>
                            )}
                          </div>
                        </div>
                        {source.path && (
                          <p className="text-[11px] truncate mb-3" style={{ color: colors.textMuted }}>
                            {source.path}
                          </p>
                        )}

                        {/* File list toggle (independent from sidebar) */}
                        {source.status === 'ready' && (
                          <div>
                            <button
                              onClick={(e) => toggleDocsSourceExpansion(source.id, e)}
                              className="flex items-center gap-1.5 text-xs font-medium mb-2 transition-colors"
                              style={{ color: colors.textTertiary }}
                            >
                              {docsExpandedSources.has(source.id) ? (
                                <ChevronUp className="w-3.5 h-3.5" />
                              ) : (
                                <ChevronDown className="w-3.5 h-3.5" />
                              )}
                              {docsExpandedSources.has(source.id) ? 'Hide files' : 'Show files'}
                            </button>

                            <AnimatePresence>
                              {docsExpandedSources.has(source.id) && sourceFiles[source.id] && sourceFiles[source.id].length > 0 && (
                                <motion.div
                                  initial={{ height: 0, opacity: 0 }}
                                  animate={{ height: 'auto', opacity: 1 }}
                                  exit={{ height: 0, opacity: 0 }}
                                  transition={{ duration: 0.2 }}
                                  className="overflow-hidden"
                                >
                                  <div
                                    className="rounded-md overflow-hidden"
                                    style={{ backgroundColor: colors.bgTertiary }}
                                  >
                                    {sourceFiles[source.id].slice(0, 20).map((file: any, idx: number) => {
                                      const { Icon, color, badge } = getFileIconInfo(file.file_type);
                                      return (
                                        <div
                                          key={idx}
                                          className="flex items-center gap-2 px-3 py-1.5 text-xs transition-colors cursor-pointer"
                                          style={{ color: colors.textSecondary, borderBottom: `1px solid ${colors.border}` }}
                                          onClick={() => setPreviewFile({ path: file.file_path, name: file.name || file.file_path?.split(/[\\/]/).pop() })}
                                          onMouseEnter={e => (e.currentTarget.style.backgroundColor = `${colors.primary}08`)}
                                          onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
                                        >
                                          <Icon className="w-3.5 h-3.5 shrink-0" style={{ color }} />
                                          <span
                                            className="text-[9px] font-bold px-1 rounded shrink-0"
                                            style={{ backgroundColor: `${color}20`, color }}
                                          >
                                            {badge}
                                          </span>
                                          <span className="flex-1 truncate">
                                            {file.name || file.file_path?.split(/[\\/]/).pop()}
                                          </span>
                                          {file.status === 'indexed' && (
                                            <Check className="w-3 h-3 shrink-0" style={{ color: colors.success }} />
                                          )}
                                        </div>
                                      );
                                    })}
                                    {sourceFiles[source.id].length > 20 && (
                                      <div className="px-3 py-2 text-[10px] text-center" style={{ color: colors.textMuted }}>
                                        +{sourceFiles[source.id].length - 20} more files
                                      </div>
                                    )}
                                  </div>
                                </motion.div>
                              )}
                            </AnimatePresence>

                            {/* Loading state */}
                            {docsExpandedSources.has(source.id) && !sourceFiles[source.id] && (
                              <div className="flex items-center gap-2 py-2">
                                <Loader2 className="w-3 h-3 animate-spin" style={{ color: colors.primary }} />
                                <span className="text-xs" style={{ color: colors.textMuted }}>Loading files...</span>
                              </div>
                            )}
                          </div>
                        )}
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </div>
          )}

        </div>
      </div>

      {/* Document Preview Panel */}
      <AnimatePresence>
        {previewFile && (
          <DocumentPreviewPanel
            file={previewFile}
            onClose={() => setPreviewFile(null)}
          />
        )}
      </AnimatePresence>

      {/* Command Palette */}
      <CommandPalette
        open={cmdPaletteOpen}
        onClose={closePalette}
        onNavigate={setActiveTab}
        onNewConversation={handleNewConversation}
        onToggleTheme={toggleTheme}
        onOpenLLMSettings={() => { setShowLLMSettings(true); closePalette(); }}
        onAddSource={() => { handleAddSource(); closePalette(); }}
        sources={sources.map(s => ({ id: s.id, name: s.name, selected: s.selected }))}
      />

      {/* LLM Settings Modal */}
      {showLLMSettings && (
        <div className="fixed inset-0 bg-black/80 flex items-center justify-center p-4 z-50">
          <div
            className="bg-white dark:bg-gray-800 rounded-lg max-w-4xl w-full max-h-[90vh] overflow-auto"
            style={{
              '--surface': '#1f2937',
              '--border': '#374151',
              '--text': '#f3f4f6',
              '--text-dim': '#9ca3af',
              '--primary': '#f73129',
              '--hover': '#374151',
              '--success': '#10b981',
              '--error': '#ef4444',
            } as React.CSSProperties}
          >
            <LLMSettings
              onClose={async () => {
                setShowLLMSettings(false);
                // Re-check LLM status after settings close
                try {
                  const info: any = await invoke("get_llm_info");
                  if (info) {
                    setLlmStatus({
                      connected: true,
                      model: info.model || 'Unknown',
                      provider: info.provider || 'Unknown'
                    });
                  }
                } catch (e) {
                  debugLog("LLM check after settings:", e);
                }
              }}
              onStatusChange={async () => {
                // Re-check LLM status when it changes
                try {
                  const info: any = await invoke("get_llm_info");
                  if (info) {
                    setLlmStatus({
                      connected: true,
                      model: info.model || 'Unknown',
                      provider: info.provider || 'Unknown'
                    });
                  }
                } catch (e) {
                  debugLog("LLM status change check:", e);
                  setLlmStatus({
                    connected: false,
                    model: 'Not configured',
                    provider: 'none'
                  });
                }
              }}
            />
            {/* Search Settings Section */}
            <div className="p-6 border-t" style={{ borderColor: colors.border }}>
              <SearchSettings
                config={searchConfig}
                onUpdate={updateSearchConfig}
                onReset={resetSearchConfig}
              />
            </div>
          </div>
        </div>
      )}

      {/* Onboarding Flow */}
      <OnboardingFlow
        isOpen={showOnboarding}
        onComplete={() => setShowOnboarding(false)}
        onSkip={() => setShowOnboarding(false)}
      />

      {/* Feedback Dialog */}
      <FeedbackDialog
        isOpen={showFeedback}
        onClose={() => setShowFeedback(false)}
      />

      {/* Update Notification */}
      <UpdateNotification />

    </div>
  );
}

export default AppSplitView;

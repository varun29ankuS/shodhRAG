import React, { Suspense, lazy, useState, useEffect, useRef, useCallback, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { writeTextFile } from "@tauri-apps/plugin-fs";

// UI Components
import { Button } from "./components/ui/button";
import { Badge } from "./components/ui/badge";
import { Input } from "./components/ui/input";
import { Progress } from "./components/ui/progress";
import {
  MessageSquare, Settings, Bot, FolderOpen, FileText, Code, Terminal, Plus, Check, X, Loader2, Pencil, Download, ChevronDown, ChevronUp, Database, FileCode, BookOpen, FileSpreadsheet, Presentation, Trash2, Braces, Coffee,
  FolderPlus, PanelLeftOpen, PanelLeftClose, Sun, Moon, Bug, Search, Sparkles, AlertTriangle, RotateCcw
} from 'lucide-react';

const IS_MAC = typeof navigator !== 'undefined' && /Mac/i.test(navigator.platform);

// Views opened less often load on first use, keeping them out of the startup bundle.
const TasksView = lazy(() => import('./features/tasks/TasksView'));
const ActivityView = lazy(() => import('./features/activity/ActivityView'));
const SettingsView = lazy(() => import('./components/shell/SettingsView'));

function safeStorage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

// Core components
import { LibraryView } from './features/library/LibraryView';
import { parseStoredSources, readIndexingResult, serializeSources, SOURCES_STORAGE_KEY } from './features/library/sources';
import type { LibrarySource } from './features/library/sources';
import type { FileNode } from './features/library/fileTree';
import { errorMessage as searchErrorMessage } from './features/setup/searchModels';
import Sidebar from './components/shell/Sidebar';
import { ConversationDock } from './components/shell/ConversationDock';
import { normalizeViewTab, VIEW_TAB_LABELS } from './lib/viewTabs';
import { isBlankConversation } from './lib/conversationGroups';
import type { ViewTab } from './lib/viewTabs';
import { useTheme } from './contexts/ThemeContext';
import { useSidebar } from './contexts/SidebarContext';
import { ChatSessionProvider, useChatSession } from './features/ask/ChatSessionContext';
import { FocusProvider } from './features/focus/FocusContext';
import { VisualsButton } from './features/visuals/GalleryDialog';
import { VisualNavigator } from './features/visuals/VisualNavigator';
import { SnippetHost } from './features/research/SnippetHost';
import { ReminderAlerts } from './features/tasks/ReminderAlerts';
import { TableModelPrompt } from './features/setup/TableModelPrompt';
import { AskView } from './features/ask/AskView';
import { useCommandPalette } from './hooks/useCommandPalette';
import CommandPalette from './components/CommandPalette';
import type { PaletteAction } from './components/CommandPalette';
import { ViewSkeleton } from './components/shell/ViewSkeleton';
import { useSearchConfig } from './components/SearchSettings';
import { FirstRunFlow } from './features/setup/FirstRunFlow';
import { readFirstRun, resumeStep, writeFirstRun } from './features/setup/firstRun';
import type { FirstRunState, FirstRunStep } from './features/setup/firstRun';
import { useSearchModels } from './features/setup/SearchModelsContext';
import { markStartup } from './lib/startupTiming';
import { FeedbackDialog } from './components/FeedbackDialog';
import { UpdateNotification } from './components/UpdateNotification';
import { toast } from 'sonner';
import { notify, setNotificationHandler } from './lib/notify';
import { migrateLegacyApiKeys } from './lib/apiKeyMigration';
import { useNotifications } from './hooks/useNotifications';
import NotificationCenter from './components/NotificationCenter';
import { useNavigationTarget } from './features/agent/useNavigationTarget';

// Debug logging — set to true during development, false for demo/production
const DEBUG = false;
const debugLog = (...args: any[]) => { if (DEBUG) console.log(...args); };

/** Unique id for OCR / indexing notices appended to the conversation. */
const newNoticeId = () => `notice-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;

/** A Library source (see features/library/sources). */
type Source = LibrarySource;


function AppSplitView() {
  // Theme
  const { theme, colors, toggleTheme } = useTheme();
  const { collapsed, toggleSidebar } = useSidebar();
  const { config: searchConfig, updateConfig: updateSearchConfig, resetConfig: resetSearchConfig } = useSearchConfig();

  // Conversations and the active chat session
  const {
    conversations,
    activeConversationId,
    activeConversation,
    createConversation,
    switchConversation,
    renameConversation,
    deleteConversation,
    pinConversation,
    updateConversationMeta,
    messages,
    appendMessage,
    updateMessage,
  } = useChatSession();

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

  // Move API keys saved by older builds out of localStorage into the OS keychain.
  useEffect(() => {
    void migrateLegacyApiKeys();
  }, []);

  // Command palette
  const { open: cmdPaletteOpen, openPalette, closePalette } = useCommandPalette();

  // Core state
  // True until `initialize_rag` answers; the shell renders immediately regardless.
  const [isLoading, setIsLoading] = useState(true);
  const [initError, setInitError] = useState<string | null>(null);
  const [activeTab, setActiveTab] = useState<ViewTab>('ask');
  // Restored synchronously so the Library and sidebar render with the first frame.
  const [sources, setSources] = useState<Source[]>(() => {
    try {
      return parseStoredSources(localStorage.getItem(SOURCES_STORAGE_KEY));
    } catch {
      return [];
    }
  });
  /** Library source the agent pointed at (show_source); LibraryView scrolls to and highlights it. */
  const [focusedSourceId, setFocusedSourceId] = useState<string | null>(null);
  useNavigationTarget('source', target => setFocusedSourceId(target.sourceId));
  const [currentlyIndexing, setCurrentlyIndexing] = useState<string | null>(null);
  // Latest sources and indexing actions for listeners registered once (drag and drop).
  const sourcesRef = useRef<Source[]>(sources);
  sourcesRef.current = sources;
  const indexSourceRef = useRef<((source: Source, kind: 'folder' | 'file') => Promise<void>) | null>(null);
  const indexingBusyRef = useRef<(() => boolean) | null>(null);
  // Text placed in the Ask composer by another view ("Ask about this file").
  const [askDraft, setAskDraft] = useState<{ text: string; seq: number } | null>(null);
  const clearAskDraft = useCallback(() => setAskDraft(null), []);

  // Persist sources on every change (live progress fields are not stored).
  useEffect(() => {
    try {
      localStorage.setItem(SOURCES_STORAGE_KEY, serializeSources(sources));
    } catch {
      // Storage unavailable: sources last for this session only.
    }
  }, [sources]);
  const [isDraggingImage, setIsDraggingImage] = useState(false);
  const lastProcessedImageTimeRef = useRef(0);

  // Onboarding & Feedback
  // First-run setup: shown over the shell until finished or skipped; resumable.
  const [firstRun, setFirstRun] = useState<FirstRunState>(() => readFirstRun(safeStorage()));
  const [firstRunOpen, setFirstRunOpen] = useState(() => firstRun.status === 'pending');
  const { status: searchModelsStatus } = useSearchModels();
  const [showFeedback, setShowFeedback] = useState(false);

  // Ref to prevent double initialization (React Strict Mode protection)
  const initializationRef = useRef(false);
  const initializationPromiseRef = useRef<Promise<void> | null>(null);

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

  // Re-read the active LLM from the backend (after settings changes).
  const refreshLlmStatus = useCallback(async () => {
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
      debugLog("LLM status check:", e);
      setLlmStatus({
        connected: false,
        model: 'Not configured',
        provider: 'none'
      });
    }
  }, []);

  // Remember the last non-settings view so dismissing model settings returns there.
  const lastContentTabRef = useRef<ViewTab>('ask');
  useEffect(() => {
    if (activeTab !== 'settings') lastContentTabRef.current = activeTab;
  }, [activeTab]);

  const closeModelSettings = useCallback(() => {
    refreshLlmStatus();
    setActiveTab(lastContentTabRef.current);
  }, [refreshLlmStatus]);


  const [showSystemPromptEditor, setShowSystemPromptEditor] = useState(false);
  const [newInstructionText, setNewInstructionText] = useState('');
  const [editingInstructionIdx, setEditingInstructionIdx] = useState<number | null>(null);
  const [editingInstructionText, setEditingInstructionText] = useState('');
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

  // Create new conversation with current source association and show it.
  // An untouched active conversation is reused rather than piling up blanks.
  const handleNewConversation = () => {
    if (activeConversation && isBlankConversation(activeConversation)) {
      setActiveTab('ask');
      return;
    }
    createConversation({
      spaceId: activeSpaceId || undefined,
      spaceName: activeSourceName || undefined,
    });
    setActiveTab('ask');
  };

  const saveFirstRun = useCallback((next: FirstRunState) => {
    setFirstRun(next);
    writeFirstRun(safeStorage(), next);
  }, []);

  // Reopening a finished setup and closing it again must not mark it skipped.
  const statusBeforeOpenRef = useRef(firstRun.status);
  const openFirstRun = useCallback((step?: FirstRunStep) => {
    statusBeforeOpenRef.current = firstRun.status;
    saveFirstRun({ status: 'pending', step: step ?? resumeStep(firstRun) });
    setFirstRunOpen(true);
  }, [firstRun, saveFirstRun]);

  const changeFirstRunStep = useCallback((step: FirstRunStep) => {
    saveFirstRun({ status: 'pending', step });
  }, [saveFirstRun]);

  const finishFirstRun = useCallback(() => {
    saveFirstRun({ status: 'completed', step: 'done' });
    setFirstRunOpen(false);
    setActiveTab('ask');
  }, [saveFirstRun]);

  const skipFirstRun = useCallback(() => {
    setFirstRunOpen(false);
    setFirstRun(prev => {
      const next: FirstRunState = { status: statusBeforeOpenRef.current === 'completed' ? 'completed' : 'skipped', step: prev.step };
      writeFirstRun(safeStorage(), next);
      return next;
    });
  }, []);

  useEffect(() => {
    markStartup('shell-mounted');
  }, []);

  const searchNeedsSetup = !!searchModelsStatus && !searchModelsStatus.ready;

  // Primary actions offered by the command palette.
  const paletteActions: PaletteAction[] = [
    { id: 'new-chat', label: 'New chat', icon: Plus, keywords: 'new chat conversation ask create', shortcut: IS_MAC ? '⌘N' : 'Ctrl+N', run: handleNewConversation },
    { id: 'add-folder', label: 'Add folder to Library', description: 'Index a folder of documents', icon: FolderPlus, keywords: 'add source folder documents index import', run: () => { setActiveTab('library'); void handleAddSource(); } },
    { id: 'choose-model', label: 'Choose model', description: 'Provider and model that answer', icon: Bot, keywords: 'model llm provider ai settings api key', run: () => setActiveTab('settings') },
    { id: 'toggle-sidebar', label: collapsed ? 'Expand sidebar' : 'Collapse sidebar', icon: collapsed ? PanelLeftOpen : PanelLeftClose, keywords: 'sidebar toggle hide show collapse expand', shortcut: IS_MAC ? '⌘B' : 'Ctrl+B', run: toggleSidebar },
    { id: 'toggle-theme', label: theme === 'dark' ? 'Use light theme' : 'Use dark theme', icon: theme === 'dark' ? Sun : Moon, keywords: 'theme dark light mode appearance', run: toggleTheme },
    { id: 'feedback', label: 'Send feedback', icon: Bug, keywords: 'feedback bug report problem', run: () => setShowFeedback(true) },
    ...(searchNeedsSetup
      ? [{ id: 'set-up-search', label: 'Set up search', description: 'Download the search models (one time)', icon: Search, keywords: 'search models download install embedding reranker setup', run: () => openFirstRun('search') }]
      : []),
    firstRun.status === 'completed'
      ? { id: 'first-run', label: 'Run setup again', description: 'Search, model and first folder', icon: Sparkles, keywords: 'setup onboarding welcome first run guide', run: () => openFirstRun('welcome') }
      : { id: 'first-run', label: 'Resume setup', description: 'Pick up where you left off', icon: Sparkles, keywords: 'setup onboarding welcome first run guide resume', run: () => openFirstRun() },
  ];


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
    appendMessage({
      id: messageId,
      role: 'system',
      content: `📄 Uploading **${file.name}** (${(file.size / 1048576).toFixed(2)} MB)...\n⏳ Parsing → Chunking → Indexing...`,
      timestamp: new Date().toISOString()
    });

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
        updateMessage(messageId, { content: `✅ **${result.fileName}** indexed successfully!\n\n` +
                   `📊 **${result.chunksCreated} chunks** created in ${result.processingTimeMs}ms\n` +
                   `💾 Size: ${result.fileSizeMb.toFixed(2)} MB\n\n` +
                   `*Ask me anything about this document!*` });
      } else {
        updateMessage(messageId, { content: `❌ Failed to index **${result.fileName}**\n\nError: ${result.error}` });
      }
    } catch (error) {
      console.error('Upload error:', error);
      updateMessage(messageId, { content: `❌ Upload failed: ${error}` });
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
            appendMessage({
              id: newNoticeId(),
              role: 'assistant',
              content: `📸 **[FILE PICKER] Image uploaded and processed successfully!**\n\n**Extracted Text (${wordCount} words, ${(confidence * 100).toFixed(0)}% confidence):**\n\n${extractedText}`,
              timestamp: new Date().toISOString(),
              image: imageData
            });
          } else {
            appendMessage({
              id: newNoticeId(),
              role: 'assistant',
              content: `📸 **[FILE PICKER] Image uploaded**\n\nNo text was detected in this image.`,
              timestamp: new Date().toISOString(),
              image: imageData
            });
          }
        } catch (error) {
          console.error('Failed to process selected image:', error);
          notify.error('Image processing failed', { description: String(error) });
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
                    appendMessage({
                      id: newNoticeId(),
                      role: 'assistant',
                      content: `📸 **Image processed successfully!**\n\n**Extracted Text (${wordCount} words, ${(confidence * 100).toFixed(0)}% confidence):**\n\n${extractedText}`,
                      timestamp: new Date().toISOString(),
                      image: imageData
                    });
                  } else {
                    appendMessage({
                      id: newNoticeId(),
                      role: 'assistant',
                      content: `📸 **Image dropped**\n\nNo text was detected in this image.`,
                      timestamp: new Date().toISOString(),
                      image: imageData
                    });
                  }
                }
              }

              // Process document files or folders
              if (documentFiles.length > 0 || (files.length > 0 && imageFiles.length === 0 && documentFiles.length === 0)) {
                // Treat all non-image files as documents or folders to be indexed
                const pathsToIndex = documentFiles.length > 0 ? documentFiles : files;

                for (const path of pathsToIndex) {
                  if (indexingBusyRef.current?.()) break;
                  let isDirectory = false;
                  try {
                    const stats = await invoke<{ isDirectory: boolean }>('check_path_type', { path });
                    isDirectory = stats.isDirectory;
                  } catch (e) {
                    console.warn('Failed to check path type, assuming file:', e);
                  }
                  const known = sourcesRef.current.find(s => s.path.toLowerCase() === path.toLowerCase());
                  const source: Source = known ?? {
                    id: `${isDirectory ? 'folder' : 'file'}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
                    name: path.split(/[\\/]/).filter(Boolean).pop() || path,
                    path,
                    type: 'documents',
                    fileCount: 0,
                    indexedAt: new Date().toISOString(),
                    status: 'indexing',
                    selected: true,
                  };
                  if (!known) setSources(prev => [...prev, source]);
                  // Sequential: progress events carry no source id.
                  await indexSourceRef.current?.(source, isDirectory ? 'folder' : 'file');
                }
              }
            } catch (error) {
              console.error('❌ Failed to process dropped files:', error);
              appendMessage({
                id: newNoticeId(),
                role: 'assistant',
                content: `❌ Failed to process dropped files: ${error}`,
                timestamp: new Date().toISOString()
              });
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
              appendMessage({
                id: newNoticeId(),
                role: 'assistant',
                content: `📸 **Image pasted and processed successfully!**\n\n**Extracted Text (${wordCount} words, ${(confidence * 100).toFixed(0)}% confidence):**\n\n${extractedText}`,
                timestamp: new Date().toISOString(),
                image: base64Data
              });
            } else {
              appendMessage({
                id: newNoticeId(),
                role: 'assistant',
                content: `📸 **Image pasted**\n\nNo text was detected in this image.`,
                timestamp: new Date().toISOString(),
                image: base64Data
              });
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

  // Keyboard shortcut for Command Palette (Cmd+K / Ctrl+K)
  useEffect(() => {
    const handleKeyDown = (_e: KeyboardEvent) => {
      // Keyboard shortcuts can be added here
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, []);

  // Initialize app once on mount
  useEffect(() => {
    initializeApp();
  }, []); // Empty dependency array - run only once on mount

  // Setup other event listeners
  useEffect(() => {
    let unlistenProg: (() => void) | null = null;

    // Listen for tab switching events from child components
    const handleSwitchTab = (event: Event) => {
      const tab = normalizeViewTab((event as CustomEvent<unknown>).detail);
      if (tab) {
        setActiveTab(tab);
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
        setInitError(null);
        await invoke("initialize_rag");
        markStartup('index-ready');
        // The shell is already on screen; this only unlocks index-backed views.
        setIsLoading(false);

        const checkLLMStatus = async () => {
          try {
            const info: any = await invoke("get_llm_info");
            if (info) {
              setLlmStatus({
                connected: true,
                model: info.model || 'Unknown',
                provider: info.provider || 'Unknown'
              });
              return true;
            }
          } catch (e) {
            debugLog("LLM not configured:", e);
            setLlmStatus({
              connected: false,
              model: 'Not configured',
              provider: 'none'
            });
          }
          return false;
        };

        // Model status and index statistics are independent: load them together.
        const [isConnected] = await Promise.all([
          checkLLMStatus().finally(() => markStartup('model-checked')),
          updateStats().finally(() => markStartup('stats-loaded')),
        ]);

        // A model that is still starting (e.g. a local server) is picked up
        // without a restart: poll every 2 s for up to 30 s.
        if (!isConnected) {
          let attempts = 0;
          const maxAttempts = 15;
          const pollInterval = setInterval(async () => {
            attempts++;
            const connected = await checkLLMStatus();
            if (connected || attempts >= maxAttempts) clearInterval(pollInterval);
          }, 2000);
        }
      } catch (error) {
        console.error("Initialization failed:", error);
        setInitError(searchErrorMessage(error));
        // Reset flag on error so the user can retry
        initializationRef.current = false;
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

  const handleOptimizeStorage = async () => {
    try {
      debugLog("Optimizing storage...");
      const result = await invoke<string>('optimize_storage');
      debugLog("Storage optimization result:", result);

      await updateStats();

      notify.success('Storage optimized', { description: result });
    } catch (error) {
      console.error("Failed to optimize storage:", error);
      notify.error('Storage optimization failed', { description: String(error) });
    }
  };

  /**
   * Index `source.path` into `source.id` and record the outcome on the
   * source: file count, completion time and per-file failures, or the error.
   * Re-indexing is idempotent (each file's previous chunks are replaced).
   */
  const indexSource = async (source: Source, kind: 'folder' | 'file' = 'folder') => {
    setCurrentlyIndexing(source.id);
    setSources(prev => prev.map(s => s.id === source.id
      ? { ...s, status: 'indexing' as const, progress: 0, processedCount: 0, currentFile: undefined, lastError: undefined }
      : s));
    try {
      const raw = kind === 'file'
        ? await invoke('index_single_file', { filePath: source.path, spaceId: source.id })
        : await invoke('link_folder_enhanced', {
            folderPath: source.path,
            spaceId: source.id,
            // Nested struct fields are snake_case. An empty file_types list
            // means every type the indexer supports.
            options: { skip_indexed: false, watch_changes: false, process_subdirs: true, priority: 'normal', file_types: [] },
          });
      const outcome = readIndexingResult(raw);
      setSources(prev => prev.map(s => s.id === source.id
        ? {
            ...s,
            status: 'ready' as const,
            fileCount: outcome.filesProcessed,
            indexedAt: new Date().toISOString(),
            failures: outcome.failures.length > 0 ? outcome.failures : undefined,
            progress: undefined,
            currentFile: undefined,
            processedCount: undefined,
          }
        : s));
      await updateStats();
      const failed = outcome.failures.length;
      notify.success(`Indexed ${outcome.filesProcessed.toLocaleString()} file${outcome.filesProcessed === 1 ? '' : 's'}`, {
        description: failed > 0 ? `${source.name} · ${failed} could not be indexed` : source.name,
      });
    } catch (error) {
      console.error('Indexing failed:', error);
      const message = searchErrorMessage(error);
      setSources(prev => prev.map(s => s.id === source.id
        ? { ...s, status: 'error' as const, lastError: message, progress: undefined, currentFile: undefined, processedCount: undefined }
        : s));
      notify.error('Indexing failed', { description: message });
    } finally {
      setCurrentlyIndexing(null);
    }
  };

  /** Progress events carry no source id, so one index runs at a time. */
  const indexingBusy = () => {
    if (!sourcesRef.current.some(s => s.status === 'indexing')) return false;
    notify.info('Indexing is already running', { description: 'Add or re-index another folder once it finishes.' });
    return true;
  };
  indexSourceRef.current = indexSource;
  indexingBusyRef.current = indexingBusy;

  /** Whether a source is a single dropped file or a folder, asked of the file system. */
  const sourceKind = async (source: Source): Promise<'folder' | 'file'> => {
    try {
      const info = await invoke<{ isDirectory: boolean; isFile: boolean }>('check_path_type', { path: source.path });
      return info.isFile && !info.isDirectory ? 'file' : 'folder';
    } catch {
      return 'folder';
    }
  };

  const handleAddSource = async () => {
    if (indexingBusy()) return;
    let selected: string | string[] | null;
    try {
      selected = await open({ directory: true, multiple: false, title: 'Add a folder to the Library' });
    } catch (error) {
      notify.error('Could not open the folder picker', { description: String(error) });
      return;
    }
    if (typeof selected !== 'string' || !selected) return;
    const picked = selected;
    const existing = sources.find(s => s.path.toLowerCase() === picked.toLowerCase());
    if (existing) {
      notify.info(`${existing.name} is already in the Library`, { description: 'Indexing it again to pick up changes.' });
      await indexSource(existing, await sourceKind(existing));
      return;
    }
    const newSource: Source = {
      id: `folder-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      name: picked.split(/[\\/]/).filter(Boolean).pop() || picked,
      path: picked,
      type: 'documents',
      fileCount: 0,
      indexedAt: new Date().toISOString(),
      status: 'indexing',
      selected: true,
    };
    setSources(prev => [...prev, newSource]);
    await indexSource(newSource, 'folder');
  };

  /** The file browser counted a source's indexed files: correct a stale card count. */
  const handleFileCount = useCallback((id: string, count: number) => {
    setSources(prev => prev.map(s => (s.id === id && s.status === 'ready' && s.fileCount !== count ? { ...s, fileCount: count } : s)));
  }, []);

  const handleReindex = async (id: string) => {
    const source = sources.find(s => s.id === id);
    if (!source || indexingBusy()) return;
    await indexSource(source, await sourceKind(source));
  };

  /** Start a chat about a Library file: a new conversation with the file named in the composer. */
  const handleAskAboutFile = (file: FileNode, source: LibrarySource) => {
    createConversation({ spaceId: source.id, spaceName: source.name });
    setAskDraft(prev => ({ text: `About ${file.name} (${file.path}): `, seq: (prev?.seq ?? 0) + 1 }));
    setActiveTab('ask');
  };

  const toggleSource = useCallback((id: string) => {
    setSources(prev => {
      return prev.map(s =>
        s.id === id ? { ...s, selected: !s.selected } : s
      );
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
              return restored;
            });
            notify.success('Source restored');
          }
        },
      },
      duration: 5000,
    });
  };


  return (
    <div
      className="h-screen flex transition-colors duration-200"
      style={{ backgroundColor: colors.bg, color: colors.text }}
    >
      <a
        href="#main-content"
        className="sr-only focus:not-sr-only focus:fixed focus:left-3 focus:top-3 focus:z-[10000] focus:px-3 focus:py-2 focus:rounded-lg focus:bg-shodh-raised focus:text-shodh-text focus:ring-2 focus:ring-ring"
      >
        Skip to content
      </a>

      {/* Left Sidebar */}
      <Sidebar
        activeView={activeTab}
        onNavigate={setActiveTab}
        conversations={conversations}
        activeConversationId={activeConversationId}
        onOpenConversation={(id: string) => { switchConversation(id); setActiveTab('ask'); }}
        onNewConversation={handleNewConversation}
        onRenameConversation={renameConversation}
        onPinConversation={pinConversation}
        onDeleteConversation={deleteConversation}
        sources={sources}
        llmStatus={llmStatus}
        onOpenCommandPalette={openPalette}
        onShowFeedback={() => setShowFeedback(true)}
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
              {VIEW_TAB_LABELS[activeTab]}
            </span>
            {activeTab === 'ask' && activeConversation?.spaceName && (() => {
              const name = activeConversation.spaceName!;
              // FNV-1a hash — must match sourceColor() in utils/colors
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
            {activeTab === 'ask' && messages.length > 0 && (
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
            {activeTab === 'ask' && activeConversationId && (
              <VisualsButton conversationId={activeConversationId} conversationTitle={activeConversation?.title ?? ''} />
            )}
            {activeTab === 'ask' && activeConversationId && (
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

        {initError && (
          <div role="alert" className="shrink-0 px-5 py-2 flex items-center gap-3 border-b border-shodh-border bg-shodh-warning-soft text-[12.5px] text-shodh-text">
            <AlertTriangle className="w-4 h-4 shrink-0 text-shodh-warning" aria-hidden="true" />
            <span className="flex-1 min-w-0 break-words">{`The local index could not be opened: ${initError}`}</span>
            <button
              type="button"
              onClick={() => void initializeApp()}
              className="h-7 px-2.5 inline-flex items-center gap-1.5 rounded-md border border-shodh-border bg-shodh-surface hover:bg-shodh-raised focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
              Try again
            </button>
          </div>
        )}

        {/* Content: each view enters with the shared motion tokens (opacity and a small rise, no layout shift). */}
        <main
          id="main-content"
          tabIndex={-1}
          aria-label={VIEW_TAB_LABELS[activeTab]}
          key={activeTab}
          className="shell-view-enter flex-1 min-h-0 overflow-hidden focus:outline-none"
        >
          <Suspense fallback={<ViewSkeleton view={activeTab} />}>
          {/* Ask Tab */}
          {activeTab === 'ask' && (
            <AskView
              sources={sources}
              llmStatus={llmStatus}
              onNavigate={setActiveTab}
              onPickImage={handlePickImage}
              draftRequest={askDraft}
              onDraftApplied={clearAskDraft}
              isDraggingFile={isDraggingImage}
              dropHandlers={{
                onDrop: handleImageDrop,
                onDragOver: handleDragOver,
                onDragEnter: handleDragEnter,
                onDragLeave: handleDragLeave,
              }}
            />
          )}

          {/* Tasks: list first, calendar as a view */}
          {activeTab === 'tasks' && <TasksView />}

          {/* Activity: usage and the audit log */}
          {activeTab === 'activity' && <ActivityView />}

          {/* Settings Tab */}
          {activeTab === 'settings' && (
            <SettingsView
              onModelStatusChange={refreshLlmStatus}
              onCloseModelSettings={closeModelSettings}
              searchConfig={searchConfig}
              onUpdateSearchConfig={updateSearchConfig}
              onResetSearchConfig={resetSearchConfig}
              sources={sources}
              onRemoveSource={removeSource}
              onSourcesCleared={() => {
                setSources([]);
              }}
              conversations={conversations}
              onOpenConversation={(id: string) => { switchConversation(id); setActiveTab('ask'); }}
            />
          )}

          {/* Library: folders, indexing progress and the file browser */}
          {activeTab === 'library' && (
            <LibraryView
              sources={sources}
              totalDocs={stats.totalDocs}
              indexReady={!isLoading}
              onAddFolder={() => void handleAddSource()}
              onReindex={id => void handleReindex(id)}
              onToggleSource={toggleSource}
              onRemoveSource={removeSource}
              onAskAboutFile={handleAskAboutFile}
              onFileCount={handleFileCount}
              focusedSourceId={focusedSourceId}
            />
          )}

          </Suspense>
        </main>
      </div>


      {/* Command Palette */}
      <CommandPalette
        open={cmdPaletteOpen}
        onClose={closePalette}
        onNavigate={setActiveTab}
        actions={paletteActions}
        conversations={conversations}
        onOpenConversation={(id: string) => { switchConversation(id); setActiveTab('ask'); }}
        sources={sources.map(s => ({ id: s.id, name: s.name, path: s.path }))}
      />

      {/* First-run setup */}
      <FirstRunFlow
        open={firstRunOpen}
        step={firstRun.step}
        onStepChange={changeFirstRunStep}
        onFinish={finishFirstRun}
        onSkip={skipFirstRun}
        llmStatus={llmStatus}
        onModelStatusChange={refreshLlmStatus}
        sources={sources}
        onAddFolder={() => void handleAddSource()}
      />

      {/* Feedback Dialog */}
      <FeedbackDialog
        isOpen={showFeedback}
        onClose={() => setShowFeedback(false)}
      />

      {/* Update Notification */}
      <UpdateNotification />
      <ReminderAlerts />
      <TableModelPrompt />

      {/* Active conversation while another view is open */}
      <ConversationDock activeTab={activeTab} onExpand={() => setActiveTab('ask')} />

    </div>
  );
}

/** App shell with the chat session (and the focus pop-out it hosts) mounted above it. */
function AppSplitViewRoot() {
  return (
    <ChatSessionProvider>
      <FocusProvider>
        <AppSplitView />
        <VisualNavigator />
        <SnippetHost />
      </FocusProvider>
    </ChatSessionProvider>
  );
}

export default AppSplitViewRoot;

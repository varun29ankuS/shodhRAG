import React, { useState, useEffect, useRef, useCallback, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import { motion, AnimatePresence, useReducedMotion } from "framer-motion";

// UI Components
import { Button } from "./components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "./components/ui/card";
import { Badge } from "./components/ui/badge";
import { Input } from "./components/ui/input";
import { Progress } from "./components/ui/progress";
import {
  MessageSquare, Settings, Bot, FolderOpen, FileText, Code, Terminal, Plus, Check, X, Loader2, Pencil, Download, ChevronDown, ChevronUp, Database, FileCode, BookOpen, FileSpreadsheet, Presentation, Trash2, Braces, Coffee
} from 'lucide-react';

// Core components
import { ImageUpload } from './components/ImageUpload';
import Sidebar from './components/shell/Sidebar';
import SettingsView from './components/shell/SettingsView';
import { ConversationDock } from './components/shell/ConversationDock';
import type { DockMode } from './components/shell/ConversationDock';
import { useMediaQuery } from './hooks/useMediaQuery';
import { normalizeViewTab, VIEW_TAB_LABELS } from './lib/viewTabs';
import type { ViewTab } from './lib/viewTabs';
import { useTheme } from './contexts/ThemeContext';
import { useSidebar } from './contexts/SidebarContext';
import { ChatSessionProvider, useChatSession } from './features/ask/ChatSessionContext';
import { AskView } from './features/ask/AskView';
import { useCommandPalette } from './hooks/useCommandPalette';
import CommandPalette from './components/CommandPalette';
import DocumentPreviewPanel from './components/DocumentPreviewPanel';
import CalendarTodoPanel from './components/CalendarTodoPanel';
import { useSearchConfig } from './components/SearchSettings';
import { useActivityTracker } from './hooks/useActivityTracker';
import { OnboardingFlow } from './components/OnboardingFlow';
import { FeedbackDialog } from './components/FeedbackDialog';
import { LoadingState } from './components/LoadingState';
import { EmptyState } from './components/EmptyState';
import { UpdateNotification } from './components/UpdateNotification';
import { toast } from 'sonner';
import { notify, setNotificationHandler } from './lib/notify';
import { migrateLegacyApiKeys } from './lib/apiKeyMigration';
import { useNotifications } from './hooks/useNotifications';
import NotificationCenter from './components/NotificationCenter';
import { intelligentSearch, trackUserMessage, trackAssistantMessage } from './utils/intelligentRetrieval';

// Debug logging — set to true during development, false for demo/production
const DEBUG = false;
const debugLog = (...args: any[]) => { if (DEBUG) console.log(...args); };

/** Unique id for OCR / indexing notices appended to the conversation. */
const newNoticeId = () => `notice-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;

// Types
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


function AppSplitView() {
  // Theme
  const { theme, colors, toggleTheme } = useTheme();
  const { collapsed } = useSidebar();
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

  // Activity Tracker
  const { trackActivity } = useActivityTracker();

  // Core state
  const [isLoading, setIsLoading] = useState(true);
  const [isFirstTime, setIsFirstTime] = useState(false);
  const [activeTab, setActiveTab] = useState<ViewTab>('ask');
  const [dockMode, setDockMode] = useState<DockMode>('hidden');
  // On wide windows the open dock gets its own column, so it never covers
  // the view's primary actions; the minimised pill reserves a bottom strip.
  const dockColumn = useMediaQuery('(min-width: 1200px)');
  const dockReserve: React.CSSProperties | undefined =
    dockMode === 'open' && dockColumn ? { paddingRight: 404 }
      : dockMode === 'minimized' ? { paddingBottom: 56 }
        : undefined;
  const prefersReducedMotion = useReducedMotion();
  const [sources, setSources] = useState<Source[]>([]);
  const [docsExpandedSources, setDocsExpandedSources] = useState<Set<string>>(new Set());
  const [sourceFiles, setSourceFiles] = useState<Record<string, any[]>>({});
  const [currentlyIndexing, setCurrentlyIndexing] = useState<string | null>(null);
  const [isDraggingImage, setIsDraggingImage] = useState(false);
  const lastProcessedImageTimeRef = useRef(0);

  // Onboarding & Feedback
  const [showOnboarding, setShowOnboarding] = useState(!localStorage.getItem('onboarding_completed'));
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

  // Document preview
  const [previewFile, setPreviewFile] = useState<{ path: string; name: string; page?: number } | null>(null);

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

  // Create new conversation with current source association and show it
  const handleNewConversation = () => {
    createConversation({
      spaceId: activeSpaceId || undefined,
      spaceName: activeSourceName || undefined,
    });
    setActiveTab('ask');
  };

  // Search State
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<any[]>([]);
  const [isSearching, setIsSearching] = useState(false);

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
              content: `📸 **[FILE PICKER] Image uploaded and processed successfully!**\n\n**Extracted Text (${wordCount} words, ${(confidence * 100).toFixed(0)}% confidence):**\n\n${extractedText}\n\n*The text has been indexed and is now searchable.*`,
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
                      content: `📸 **Image processed successfully!**\n\n**Extracted Text (${wordCount} words, ${(confidence * 100).toFixed(0)}% confidence):**\n\n${extractedText}\n\n*The text has been indexed and is now searchable.*`,
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
                  appendMessage({
                    id: newNoticeId(),
                    role: 'assistant',
                    content: `📄 **${fileName} indexed successfully!**\n\nThe document has been added to your sources and is now searchable.`,
                    timestamp: new Date().toISOString()
                  });
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
                content: `📸 **Image pasted and processed successfully!**\n\n**Extracted Text (${wordCount} words, ${(confidence * 100).toFixed(0)}% confidence):**\n\n${extractedText}\n\n*The text has been indexed and is now searchable.*`,
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
          id: newNoticeId(),
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
              file_types: ['txt', 'md', 'pdf', 'rs', 'js', 'ts', 'py', 'java', 'cpp', 'c', 'html', 'json', 'docx', 'xlsx', 'xls', 'xlsm', 'xlsb', 'ods', 'csv', 'tsv']
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

  // Toggle source file list expansion in the Library view
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

        {/* Content Area — screen enter transition keyed on the active view */}
        <motion.div
          key={activeTab}
          className="flex-1 overflow-hidden"
          style={dockReserve}
          initial={prefersReducedMotion ? false : { opacity: 0, y: 4 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: prefersReducedMotion ? 0 : 0.2, ease: [0.2, 0.7, 0.2, 1] }}
        >
          {/* Ask Tab */}
          {activeTab === 'ask' && (
            <AskView
              sources={sources}
              llmStatus={llmStatus}
              onNavigate={setActiveTab}
              onPickImage={handlePickImage}
              isDraggingFile={isDraggingImage}
              dropHandlers={{
                onDrop: handleImageDrop,
                onDragOver: handleDragOver,
                onDragEnter: handleDragEnter,
                onDragLeave: handleDragLeave,
              }}
            />
          )}

          {/* Calendar Tab */}
          {activeTab === 'calendar' && (
            <CalendarTodoPanel />
          )}

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
                localStorage.setItem('indexedSources', JSON.stringify([]));
              }}
            />
          )}

          {/* Library Tab — shows indexed sources with file lists */}
          {activeTab === 'library' && (
            <div className="h-full overflow-y-auto p-6">
              <div className="max-w-4xl mx-auto">
                <div className="flex items-center justify-between mb-6">
                  <div>
                    <h1 className="text-lg font-bold" style={{ color: colors.text }}>Library</h1>
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
                            <label
                              className="flex items-center gap-1.5 px-2 py-1 rounded-md border cursor-pointer select-none focus-within:ring-2 focus-within:ring-ring"
                              style={{ borderColor: colors.border, color: colors.textSecondary }}
                              title="Include this source when answering in Ask"
                            >
                              <input
                                type="checkbox"
                                checked={source.selected}
                                onChange={() => toggleSource(source.id)}
                                className="w-3.5 h-3.5 focus:outline-none"
                                style={{ accentColor: colors.primary }}
                              />
                              Use in Ask
                            </label>
                            <button
                              type="button"
                              onClick={(e) => removeSource(source.id, e)}
                              aria-label={`Remove source ${source.name}`}
                              title="Remove source"
                              className="w-7 h-7 rounded-md inline-flex items-center justify-center transition-colors hover:bg-shodh-raised focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                              style={{ color: colors.error }}
                            >
                              <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
                            </button>
                          </div>
                        </div>
                        {source.path && (
                          <p className="text-[11px] truncate mb-3" style={{ color: colors.textMuted }}>
                            {source.path}
                          </p>
                        )}

                        {/* File list toggle */}
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

        </motion.div>
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
        onAddSource={() => { handleAddSource(); closePalette(); }}
        sources={sources.map(s => ({ id: s.id, name: s.name, selected: s.selected }))}
      />

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

      {/* Active conversation while another view is open */}
      <ConversationDock activeTab={activeTab} onExpand={() => setActiveTab('ask')} onModeChange={setDockMode} />

    </div>
  );
}

/** App shell with the chat session provider mounted above it. */
function AppSplitViewRoot() {
  return (
    <ChatSessionProvider>
      <AppSplitView />
    </ChatSessionProvider>
  );
}

export default AppSplitViewRoot;

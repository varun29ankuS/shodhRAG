import React, { Suspense, lazy, useState, useEffect, useRef, useCallback, useMemo, useDeferredValue } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ask, open, save } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { homeDir } from "@tauri-apps/api/path";
import { writeTextFile } from "@tauri-apps/plugin-fs";

// UI Components
import { Button } from "./components/ui/button";
import { Badge } from "./components/ui/badge";
import { Input } from "./components/ui/input";
import { Progress } from "./components/ui/progress";
import {
  MessageSquare, Settings, Bot, FolderOpen, FileText, Code, Terminal, Plus, Check, X, Loader2, Pencil, ChevronDown, ChevronUp, Database, FileCode, BookOpen, FileSpreadsheet, Presentation, Trash2, Braces, Coffee,
  FolderPlus, PanelLeftOpen, PanelLeftClose, Sun, Moon, Bug, Search, Sparkles, AlertTriangle, RotateCcw, Layers, MessageCircle, Bell, ListTodo, CalendarDays
} from 'lucide-react';

const IS_MAC = typeof navigator !== 'undefined' && /Mac/i.test(navigator.platform);
const IS_WINDOWS = typeof navigator !== 'undefined' && /Win/i.test(navigator.platform);

// The New task / New event dialog brings the Tasks store with it: loaded when first opened.
const QuickCreateDialog = lazy(() => import('./features/tasks/QuickCreateDialog').then(m => ({ default: m.QuickCreateDialog })));

function safeStorage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

/** The backend's copy of a durable UI state (see lib/durableState). */
async function readDurable(key: DurableKey): Promise<StoredRead> {
  try {
    const value = await invoke<unknown>('ui_state_get', { key });
    return value === null || value === undefined ? { kind: 'none' } : { kind: 'value', value };
  } catch (error) {
    console.error(`The saved ${key} could not be read:`, error);
    return { kind: 'failed' };
  }
}

/** The sources the backend removed from the Library (in this run or before); none when unreadable. */
async function readRemovedSources(): Promise<string[]> {
  try {
    const ids = await invoke<unknown>('removed_library_sources');
    return Array.isArray(ids) ? ids.filter((id): id is string => typeof id === 'string') : [];
  } catch (error) {
    console.error('The removed sources could not be read:', error);
    return [];
  }
}

/** The last save of each durable UI state, so saves reach the disk in the order they were made. */
const durableWrites = new Map<DurableKey, Promise<void>>();

/**
 * Save a durable UI state on disk (resolves once it is written). Saves of one
 * key are chained: the backend writes each on a blocking thread, so two quick
 * saves sent together could otherwise land older-last and leave a stale value.
 */
function writeDurable(key: DurableKey, value: unknown): Promise<void> {
  const previous = durableWrites.get(key) ?? Promise.resolve();
  const next = previous.catch(() => undefined).then(() => invoke<void>('ui_state_set', { key, value }));
  durableWrites.set(key, next);
  return next;
}

// Core components
import { applyFolderSync, applyFolderSyncProgress, applyLibraryChange, indexedFileCount, indexedNotice, removedSourceIds, parseStoredSources, readIndexingResult, scannedPagesStep, serializeSources, SOURCES_STORAGE_KEY, syncedFolders, withPathCase } from './features/library/sources';
import { explainFailure } from './features/library/indexingAdvice';
import type { FolderSyncOutcome, FolderSyncProgress, LibraryChange, LibrarySource } from './features/library/sources';
import { addableFolders, indexedFilePaths, librarySourcesChip, nativePath, NOT_ADDED_REASON, reconcileSources, searchedSources } from './features/library/reconcile';
import type { FileNode } from './features/library/fileTree';
import { errorMessage as searchErrorMessage } from './features/setup/searchModels';
import Sidebar from './components/shell/Sidebar';
import { ConversationDock } from './components/shell/ConversationDock';
import { MODEL_CHANGED_EVENT } from './features/modelPicker/modelApi';
import { normalizeViewTab, VIEW_TAB_LABELS } from './lib/viewTabs';
import { isBlankConversation } from './lib/conversationGroups';
import type { ViewTab } from './lib/viewTabs';
import { useTheme } from './contexts/ThemeContext';
import { useSidebar } from './contexts/SidebarContext';
import { useQueuedQuestionSender } from './features/setup/useQueuedQuestion';
import { useTextSizeShortcuts } from './hooks/useTextSize';
import { ChatSessionProvider, useChatSession } from './features/ask/ChatSessionContext';
import { WorkspaceProvider, useWorkspaces } from './features/workspaces/WorkspaceContext';
import { sourceSummary, workspaceForNewChat } from './features/workspaces/model';
import { WorkspaceIcon } from './features/workspaces/WorkspaceIcon';
import { FocusProvider } from './features/focus/FocusContext';
import { SystemStatusProvider } from './features/status/SystemStatusContext';
import { VisualNavigator } from './features/visuals/VisualNavigator';
import { SnippetHost } from './features/research/SnippetHost';
import { ReminderAlerts } from './features/tasks/ReminderAlerts';
import type { QuickCreateKind } from './features/tasks/QuickCreateDialog';
import { quickCreateWorkspace } from './features/tasks/quickCreate';
import type { NewMenuItemId } from './lib/newMenu';
import { TableModelPrompt } from './features/setup/TableModelPrompt';
import { AskView } from './features/ask/AskView';
import { failureNotice, imageNotice, readImageText } from './features/ask/imageNotice';
import type { ImageOrigin } from './features/ask/imageNotice';
import { insideOtherDialog, instructionsEscape } from './lib/escapeScope';
import { composerAttachments } from './features/ask/imageAttachments';
import { fileQuestionDraft } from './features/ask/fileQuestion';
import type { AskDraftRequest } from './features/ask/fileQuestion';
import { endsRun, withIndexingProgress } from './features/library/indexingProgress';
import type { IndexingProgressPayload } from './features/library/indexingProgress';
import { isIndexableDocument, isIndexableFile, legacyOfficeNotice, legacyOfficeTarget, sortDroppedPaths, unsupportedFilesNotice } from './features/library/dropFiles';
import type { PastedImage } from './features/agent/AgentComposer';
import { ChatActionsMenu } from './features/ask/ChatActionsMenu';
import { useCommandPalette } from './hooks/useCommandPalette';
import CommandPalette from './components/CommandPalette';
import type { PaletteAction } from './components/CommandPalette';
import { ViewSkeleton } from './components/shell/ViewSkeleton';
import { ActivityView, ConnectorsView, LibraryView, SettingsView, TasksView, WorkspacesView, createIdlePreloadGate, isViewReady, preloadAllViews, preloadView } from './components/shell/views';
import { AssistantViewCue, useAssistantViewCue } from './components/shell/AssistantViewCue';
import { clearViewCache } from './lib/viewCache';
import { shownView } from './lib/viewSwitch';
import { useSearchConfig } from './components/SearchSettings';
import { FirstRunFlow } from './features/setup/FirstRunFlow';
import { firstRunStart, hasSavedFirstRun, readFirstRun, resumeStep, writeFirstRun } from './features/setup/firstRun';
import { reconcileFirstRun, reconcileSources as reconcileStoredSources } from './lib/durableState';
import type { DurableKey, StoredRead } from './lib/durableState';
import type { FirstRunState, FirstRunStep } from './features/setup/firstRun';
import { useSearchModels } from './features/setup/SearchModelsContext';
import { markStartup } from './lib/startupTiming';
import { FeedbackDialog } from './components/FeedbackDialog';
import { UpdateNotification } from './components/UpdateNotification';
import { toast } from 'sonner';
import { notify } from './lib/notify';
import { removeWithUndo, restoreAt } from './lib/undoToast';
import { migrateLegacyApiKeys } from './lib/apiKeyMigration';
import { InboxButton, OPEN_INBOX_EVENT } from './features/inbox/InboxButton';
import { useNavigationTarget } from './features/agent/useNavigationTarget';
import { countOf } from './lib/plural';
import { IS_MAC_PLATFORM, shortcutText, withShortcut } from './lib/shortcuts';

// Debug logging — set to true during development, false for demo/production
const DEBUG = false;
const debugLog = (...args: any[]) => { if (DEBUG) console.log(...args); };

/** Unique id for OCR / indexing notices appended to the conversation. */
const newNoticeId = () => `notice-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;

/** A pasted image file as a data URI (`process_image_from_base64` takes one). */
function readFileAsDataUrl(file: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      if (typeof reader.result === 'string') resolve(reader.result);
      else reject(new Error('The pasted image could not be read'));
    };
    reader.onerror = () => reject(reader.error ?? new Error('The pasted image could not be read'));
    reader.readAsDataURL(file);
  });
}

/** A Library source (see features/library/sources). */
type Source = LibrarySource;

/** Library paths resolved at once when their saved case is restored. */
const PATH_CASE_BATCH = 8;


function AppSplitView() {
  // Theme
  const { theme, colors, toggleTheme } = useTheme();
  const { collapsed, toggleSidebar } = useSidebar();
  const { config: searchConfig, updateConfig: updateSearchConfig, resetConfig: resetSearchConfig } = useSearchConfig();

  // Conversations and the active chat session
  const {
    conversations,
    conversationsLoaded,
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
    appendMessageTo,
    updateMessage,
    navigation,
    streamingConversationId,
    sideRun,
  } = useChatSession();
  // A question asked before Shodh was ready goes out once it is, whichever view is open.
  useQueuedQuestionSender(activeConversationId, streamingConversationId !== null || sideRun !== null);

  // Workspaces: the sidebar groups chats by them, new chats start in the current one.
  const {
    workspaces,
    allWorkspaces,
    byId: workspaceById,
    openWorkspace,
    openWorkspaceId,
    requestNewWorkspace,
  } = useWorkspaces();

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
  // The view asked for: the sidebar, the palette and new chats follow it at once.
  const [activeTab, setActiveTab] = useState<ViewTab>('ask');
  // The view shown (lib/viewSwitch): at once when its code has loaded, else
  // the current view stays until it can render.
  const deferredTab = useDeferredValue(activeTab);
  const viewTab = shownView(activeTab, deferredTab, isViewReady);
  const viewCue = useAssistantViewCue(navigation?.seq ?? 0, navigation?.view ?? null, activeTab, viewTab);
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
  const indexSourceRef = useRef<((source: Source, kind: 'folder' | 'file', adopt?: boolean) => Promise<void>) | null>(null);
  const indexingBusyRef = useRef<(() => boolean) | null>(null);
  // Text placed in the Ask composer by another view ("Ask about this file").
  const [askDraft, setAskDraft] = useState<AskDraftRequest | null>(null);
  const clearAskDraft = useCallback(() => setAskDraft(null), []);

  // The sources are kept on disk by the backend, so a force-quit cannot lose
  // them (localStorage, read above for the first frame, is written lazily).
  // Once the backend's copy is read it replaces the local one; a list only
  // older builds saved (localStorage) is moved to the backend once.
  const sourcesRestoredRef = useRef(false);
  const startSourcesRef = useRef<string | null>(null);
  if (startSourcesRef.current === null) startSourcesRef.current = serializeSources(sources);
  const savedSourcesRef = useRef<string | null>(null);
  const saveSourcesDurably = useCallback((list: readonly Source[]) => {
    const text = serializeSources(list);
    if (text === savedSourcesRef.current) return;
    savedSourcesRef.current = text;
    writeDurable('library_sources', JSON.parse(text)).catch(error => {
      savedSourcesRef.current = null;
      console.error('The Library sources could not be saved:', error);
    });
  }, []);
  // Library paths older builds saved lowercased ("c:\research\papers") get
  // the case they have on disk back, once the sources are restored. Never
  // blocks startup; a few paths at a time; identity is unchanged, since
  // every comparison of source paths ignores case.
  const restorePathCase = useCallback(async (list: readonly Source[], cancelled: () => boolean) => {
    const fixes = new Map<string, { from: string; to: string }>();
    const candidates = list.filter(s => s.path.trim() && s.status !== 'indexing');
    for (let i = 0; i < candidates.length; i += PATH_CASE_BATCH) {
      const batch = candidates.slice(i, i + PATH_CASE_BATCH);
      const resolved = await Promise.all(
        batch.map(s => invoke<string>('resolve_path_case', { path: s.path }).catch(() => s.path)),
      );
      if (cancelled()) return;
      batch.forEach((s, j) => {
        const to = resolved[j];
        if (typeof to === 'string' && withPathCase(s, to) !== s) fixes.set(s.id, { from: s.path, to });
      });
    }
    if (fixes.size === 0) return;
    setSources(prev => prev.map(s => {
      const fix = fixes.get(s.id);
      return fix && s.path === fix.from ? withPathCase(s, fix.to) : s;
    }));
  }, []);
  useEffect(() => {
    let cancelled = false;
    void Promise.all([readDurable('library_sources'), readRemovedSources()]).then(([stored, removed]) => {
      if (cancelled) return;
      const local = sourcesRef.current;
      const changed = serializeSources(local) !== startSourcesRef.current;
      const { sources: restored, save } = reconcileStoredSources(stored, local, changed, removed);
      sourcesRestoredRef.current = true;
      if (save) saveSourcesDurably(restored);
      else savedSourcesRef.current = serializeSources(restored);
      if (restored !== local) setSources(restored);
      if (IS_WINDOWS) void restorePathCase(restored, () => cancelled);
    });
    return () => {
      cancelled = true;
    };
  }, [saveSourcesDurably, restorePathCase]);
  // Persist sources on every change (live progress fields are not stored).
  useEffect(() => {
    try {
      localStorage.setItem(SOURCES_STORAGE_KEY, serializeSources(sources));
    } catch {
      // Storage unavailable: the backend's copy still keeps them.
    }
    if (sourcesRestoredRef.current) saveSourcesDurably(sources);
  }, [sources, saveSourcesDurably]);
  // Keep finished folders in sync with the index (watched, and checked once
  // the index is open); each sync updates its source's card.
  const syncedFoldersKey = JSON.stringify(syncedFolders(sources));
  useEffect(() => {
    if (isLoading) return;
    invoke('sync_folder_sources', { sources: JSON.parse(syncedFoldersKey) }).catch(error => {
      console.error('Folder sync could not start:', error);
    });
  }, [isLoading, syncedFoldersKey]);
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    listen<FolderSyncOutcome>('folder-sync', event => {
      setSources(prev => prev.map(s => applyFolderSync(s, event.payload)));
    }).then(fn => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
  // A sync reading and chunking again files indexed under an older chunker
  // shows how far it is on its card; the sync's outcome clears it.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    listen<FolderSyncProgress>('folder-sync-progress', event => {
      setSources(prev => prev.map(s => applyFolderSyncProgress(s, event.payload)));
    }).then(fn => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // The files the index holds (`list_indexed_sources`), compared with the
  // folders above so the Library never contradicts what answers cite. Read
  // once the index is open and again after each indexing run or removal.
  const [indexedFiles, setIndexedFiles] = useState<string[]>([]);
  /** Whether the list has been read (first-run waits for it). */
  const [indexedLoad, setIndexedLoad] = useState<'loading' | 'loaded' | 'failed'>('loading');
  const indexedReadRef = useRef(0);
  const refreshIndexedFiles = useCallback(async () => {
    const read = ++indexedReadRef.current;
    try {
      const docs = await invoke<unknown>('list_indexed_sources');
      if (read !== indexedReadRef.current) return;
      // Shown as the file system spells them; compared case-insensitively.
      setIndexedFiles(indexedFilePaths(docs));
      setIndexedLoad('loaded');
    } catch (error) {
      if (read !== indexedReadRef.current) return;
      console.error('The indexed files could not be listed:', error);
      setIndexedFiles([]);
      setIndexedLoad('failed');
    }
  }, []);
  // The agent indexed or removed files of a source: read the indexed files
  // again; a removed source's card goes (the backend stopped and deleted it).
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    listen<LibraryChange>('library-changed', event => {
      const removed = removedSourceIds(event.payload);
      if (removed.length > 0) {
        setSources(prev => applyLibraryChange(prev, event.payload));
        setCurrentlyIndexing(current => (current && removed.includes(current) ? null : current));
      }
      void refreshIndexedFiles();
    }).then(fn => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refreshIndexedFiles]);
  // Folders removed from the list whose files leave the index when the undo
  // window ends: until then they still cover those files.
  const [removingPaths, setRemovingPaths] = useState<string[]>([]);
  // Keyed on the folder paths, not the sources: indexing progress changes
  // the sources many times a second and leaves the paths alone.
  const coveringKey = [...sources.map(s => s.path), ...removingPaths].join('\u0000');
  const reconciliation = useMemo(
    () => reconcileSources(coveringKey ? coveringKey.split('\u0000').map(path => ({ path })) : [], indexedFiles),
    [coveringKey, indexedFiles],
  );
  const unlistedCount = reconciliation.unlisted.length;
  // What answers search, in indexed files (one value for the top bar and Ask).
  // Keyed like `coveringKey`: each folder's inclusion and path.
  const scopeKey = sources.map(s => `${s.selected ? '1' : '0'}${s.path}`).join('\u0000');
  const searched = useMemo(
    () => searchedSources(
      scopeKey ? scopeKey.split('\u0000').map(entry => ({ selected: entry[0] === '1', path: entry.slice(1) })) : [],
      indexedLoad === 'loaded' ? indexedFiles : null,
    ),
    [scopeKey, indexedFiles, indexedLoad],
  );
  // The home folder: it and the folders holding it are never added for the user.
  const [homePath, setHomePath] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    homeDir()
      .then(path => { if (!cancelled) setHomePath(path); })
      .catch(error => console.error('The home folder could not be read:', error));
    return () => {
      cancelled = true;
    };
  }, []);
  const listedKey = sources.map(s => s.path).join('\u0000');
  const addableUnlisted = useMemo(
    () => addableFolders(reconciliation.groups, listedKey ? listedKey.split('\u0000').map(path => ({ path })) : [], homePath ? [homePath] : []),
    [reconciliation.groups, listedKey, homePath],
  );
  const [unlistedBusy, setUnlistedBusy] = useState(false);
  const unlistedBusyRef = useRef(false);

  const [isDraggingImage, setIsDraggingImage] = useState(false);
  const lastProcessedImageTimeRef = useRef(0);

  // Onboarding & Feedback
  // First-run setup: shown over the shell until finished or skipped; resumable.
  // Kept on disk by the backend too (see lib/durableState): without a local
  // copy (a fresh install, or one lost to a force-quit) setup opens only once
  // the backend's copy is read, so a finished setup never starts again.
  const firstRunSavedLocallyRef = useRef<boolean | null>(null);
  if (firstRunSavedLocallyRef.current === null) firstRunSavedLocallyRef.current = hasSavedFirstRun(safeStorage());
  const [firstRun, setFirstRun] = useState<FirstRunState>(() => readFirstRun(safeStorage()));
  const [firstRunOpen, setFirstRunOpen] = useState(() => firstRun.status === 'pending' && firstRunSavedLocallyRef.current === true);
  const firstRunRef = useRef(firstRun);
  firstRunRef.current = firstRun;
  const firstRunChangedRef = useRef(false);
  /** The setup state found at start, waiting for the open-or-not decision. */
  const [firstRunStartFrom, setFirstRunStartFrom] = useState<{ saved: FirstRunState | null; diskReadFailed: boolean } | null>(null);
  useEffect(() => {
    let cancelled = false;
    void readDurable('first_run').then(stored => {
      if (cancelled || firstRunChangedRef.current) return;
      const { state, save, saved } = reconcileFirstRun(stored, firstRunRef.current, firstRunSavedLocallyRef.current === true);
      setFirstRun(state);
      // Whether setup opens is decided once the index's documents are known
      // (see firstRunStart); until then it stays closed, so it never flashes.
      setFirstRunStartFrom({ saved: saved ? state : null, diskReadFailed: stored.kind === 'failed' });
      // A fresh install's default is not written: written, it would count as
      // a saved state next start, before this start's decision was made.
      if (saved) writeFirstRun(safeStorage(), state);
      if (save) {
        writeDurable('first_run', state).catch(error => console.error('The setup progress could not be saved:', error));
      }
    });
    return () => {
      cancelled = true;
    };
  }, []);
  const { status: searchModelsStatus } = useSearchModels();
  // Ctrl + / Ctrl − / Ctrl 0: the text size (Settings → Appearance), from anywhere.
  useTextSizeShortcuts();
  const [showFeedback, setShowFeedback] = useState(false);
  // "New task…" / "New event…" over the current view (New menu, palette).
  const [quickCreate, setQuickCreate] = useState<QuickCreateKind | null>(null);

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

  // A model chosen in the picker (Ask chip, Settings → Model) changes the status line too.
  useEffect(() => {
    const onModelChanged = () => void refreshLlmStatus();
    window.addEventListener(MODEL_CHANGED_EVENT, onModelChanged);
    return () => window.removeEventListener(MODEL_CHANGED_EVENT, onModelChanged);
  }, [refreshLlmStatus]);




  const [showSystemPromptEditor, setShowSystemPromptEditor] = useState(false);
  const [newInstructionText, setNewInstructionText] = useState('');
  const [editingInstructionIdx, setEditingInstructionIdx] = useState<number | null>(null);
  const [editingInstructionText, setEditingInstructionText] = useState('');
  // Focus returns to the Instructions button when its popover closes, and to the
  // new-instruction field when a row's control it was on goes away (Save, Cancel,
  // Remove, Clear all), so it never falls to the page body.
  const instructionsButtonRef = useRef<HTMLButtonElement>(null);
  const newInstructionInputRef = useRef<HTMLInputElement>(null);
  const instructionsPopoverRef = useRef<HTMLDivElement>(null);
  const focusNewInstruction = () => {
    requestAnimationFrame(() => newInstructionInputRef.current?.focus());
  };
  // Esc while the popover is open, wherever focus is (even on the page body after a
  // row was removed): cancels an edit, else closes the popover, and is marked handled
  // so a running answer is never interrupted (features/ask/AskView's Esc). Capture
  // phase, before any other handler sees the key.
  useEffect(() => {
    if (!showSystemPromptEditor) return;
    const onKeyDown = (e: KeyboardEvent) => {
      const action = instructionsEscape({
        key: e.key,
        handled: e.defaultPrevented,
        editing: editingInstructionIdx !== null,
        inOtherDialog: insideOtherDialog<Element>(e.target instanceof Element ? e.target : null, instructionsPopoverRef.current),
      });
      if (action === 'ignore') return;
      e.preventDefault();
      e.stopPropagation();
      if (action === 'cancel-edit') {
        setEditingInstructionIdx(null);
        setEditingInstructionText('');
        focusNewInstruction();
        return;
      }
      setShowSystemPromptEditor(false);
      setEditingInstructionIdx(null);
      instructionsButtonRef.current?.focus();
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, [showSystemPromptEditor, editingInstructionIdx]);

  // Derive the system prompt from the active conversation
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

  // Start a chat in `workspaceId` (null: no workspace) and show it. An untouched active
  // conversation is reused (moved to that workspace) rather than piling up blanks.
  const startChat = (workspaceId: string | null) => {
    if (activeConversation && isBlankConversation(activeConversation)) {
      if ((activeConversation.workspaceId ?? null) !== workspaceId) {
        updateConversationMeta(activeConversation.id, { workspaceId: workspaceId ?? undefined });
      }
      setActiveTab('ask');
      return;
    }
    createConversation({ workspaceId });
    setActiveTab('ask');
  };

  // "New chat" starts in the current workspace: the workspace page being shown, else the
  // workspace of the chat in use.
  const handleNewConversation = () => {
    startChat(workspaceForNewChat(activeTab === 'workspaces' ? openWorkspaceId : null, activeConversation?.workspaceId));
  };

  const persistFirstRun = useCallback((next: FirstRunState) => {
    firstRunChangedRef.current = true;
    writeFirstRun(safeStorage(), next);
    writeDurable('first_run', next).catch(error => console.error('The setup progress could not be saved:', error));
  }, []);

  const saveFirstRun = useCallback((next: FirstRunState) => {
    setFirstRun(next);
    persistFirstRun(next);
  }, [persistFirstRun]);

  // Open setup on start, or not: a saved state decides at once; without one,
  // once the index's documents are listed (an index with documents was set
  // up before, so setup is recorded as skipped and stays closed).
  useEffect(() => {
    if (!firstRunStartFrom) return;
    if (firstRunChangedRef.current) {
      // Setup was opened or changed meanwhile: that wins.
      setFirstRunStartFrom(null);
      return;
    }
    if (!firstRunStartFrom.saved && indexedLoad === 'loading') return;
    const { open, record, persist } = firstRunStart(
      firstRunStartFrom.saved,
      indexedLoad === 'loaded' ? indexedFiles.length : null,
      firstRunStartFrom.diskReadFailed,
    );
    setFirstRunStartFrom(null);
    // Not saved over a disk copy that could not be read: this session only.
    if (record) {
      if (persist) saveFirstRun(record);
      else setFirstRun(record);
    }
    setFirstRunOpen(open);
  }, [firstRunStartFrom, indexedLoad, indexedFiles.length, saveFirstRun]);

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
    const next: FirstRunState = {
      status: statusBeforeOpenRef.current === 'completed' ? 'completed' : 'skipped',
      step: firstRunRef.current.step,
    };
    setFirstRun(next);
    persistFirstRun(next);
  }, [persistFirstRun]);

  useEffect(() => {
    markStartup('shell-mounted');
  }, []);

  const searchNeedsSetup = !!searchModelsStatus && !searchModelsStatus.ready;

  // What the New menu's items and their palette entries run: one handler each.
  const newItemActions: Record<NewMenuItemId, () => void> = {
    'new-chat': handleNewConversation,
    'new-workspace': requestNewWorkspace,
    'new-task': () => setQuickCreate('task'),
    'new-event': () => setQuickCreate('event'),
    'add-folder': () => { setActiveTab('library'); void handleAddSource(); },
  };

  // Primary actions offered by the command palette.
  const paletteActions: PaletteAction[] = [
    { id: 'new-chat', label: 'New chat', icon: Plus, keywords: 'new chat conversation ask create', shortcut: shortcutText('newChat', IS_MAC), run: newItemActions['new-chat'] },
    { id: 'new-chat-plain', label: 'New chat outside workspaces', description: 'Searches your whole library', icon: MessageCircle, keywords: 'new chat no workspace library all', run: () => startChat(null) },
    { id: 'new-workspace', label: 'New workspace', description: 'Sources, instructions and chats for one piece of work', icon: Layers, keywords: 'new create workspace project template', run: newItemActions['new-workspace'] },
    { id: 'add-folder', label: 'Add folder to Library', description: 'Index a folder of documents', icon: FolderPlus, keywords: 'add source folder documents index import', run: newItemActions['add-folder'] },
    { id: 'new-task', label: 'New task', description: 'Add a task without leaving this view', icon: ListTodo, keywords: 'new create task todo reminder due', run: newItemActions['new-task'] },
    { id: 'new-event', label: 'New event', description: 'Add an event without leaving this view', icon: CalendarDays, keywords: 'new create event calendar meeting schedule', run: newItemActions['new-event'] },
    { id: 'choose-model', label: 'Choose model', description: 'Provider and model that answer', icon: Bot, keywords: 'model llm provider ai settings api key', run: () => setActiveTab('settings') },
    { id: 'toggle-sidebar', label: collapsed ? 'Expand sidebar' : 'Collapse sidebar', icon: collapsed ? PanelLeftOpen : PanelLeftClose, keywords: 'sidebar toggle hide show collapse expand', shortcut: shortcutText('toggleSidebar', IS_MAC), run: toggleSidebar },
    { id: 'toggle-theme', label: theme === 'dark' ? 'Use light theme' : 'Use dark theme', icon: theme === 'dark' ? Sun : Moon, keywords: 'theme dark light mode appearance', run: toggleTheme },
    { id: 'inbox', label: 'Inbox', description: 'Approvals, suggestions, reminders and finished work', icon: Bell, keywords: 'inbox notifications approvals approve pending reminders suggestions done finished', run: () => window.dispatchEvent(new Event(OPEN_INBOX_EVENT)) },
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


  /** The chat notice for a document that could not be indexed: the plain reason, what to do, and what the indexer said. */
  const indexFailureNotice = (fileName: string, error: string): string => {
    const failure = explainFailure(error, fileName);
    const detail = failure.detail ? `\n\nDetails: \`${failure.detail.replace(/`/g, "'")}\`` : '';
    return `❌ Could not index **${fileName}**: ${failure.reason}.\n\n${failure.advice.hint}${detail}`;
  };

  // Enhanced file drop handler - supports images AND documents
  const handleImageDrop = async (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDraggingImage(false);

    const files = Array.from(e.dataTransfer.files);
    if (files.length === 0) return;

    const file = files[0];

    // The documents the indexer reads (features/library/dropFiles); old
    // .doc/.ppt files are named with what to do.
    if (isIndexableDocument(file.name)) {
      await handleDocumentUpload(file);
    } else if (legacyOfficeTarget(file.name)) {
      notify.error('Old Office files are not indexed', { description: legacyOfficeNotice([file.name]) });
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

    // The saved copy of the dropped file; removed again when it was not indexed.
    let savedPath: string | null = null;
    const discardSaved = () => {
      if (!savedPath) return;
      invoke<boolean>('discard_upload', { filePath: savedPath })
        .catch(err => console.error('The failed upload could not be removed:', err));
    };
    try {
      // Get file path from Tauri
      const filePath = await invoke<string>('save_temp_file', {
        fileName: file.name,
        fileData: Array.from(new Uint8Array(await file.arrayBuffer()))
      });
      savedPath = filePath;

      // Upload to backend
      const result = await invoke<{
        success: boolean;
        fileName: string;
        fileType: string;
        chunksCreated: number;
        fileSizeMb: number;
        processingTimeMs: number;
        error?: string;
        // Pages of a scan OCR could not read ("pages 4, 9 could not be read").
        notice?: string | null;
      }>('upload_document_file', {
        // A file added in a chat belongs to where it was added (the chat's
        // workspace, or the main Ask), not to a Library folder.
        filePath,
        workspaceId: activeConversation?.workspaceId ?? null,
      });

      // Update message with result
      if (result.success) {
        updateMessage(messageId, { content: `✅ **${result.fileName}** indexed successfully!\n\n` +
                   `📊 **${countOf(result.chunksCreated, 'chunk')}** created in ${result.processingTimeMs}ms\n` +
                   `💾 Size: ${result.fileSizeMb.toFixed(2)} MB\n\n` +
                   (result.notice ? `⚠️ ${result.notice[0].toUpperCase()}${result.notice.slice(1)}; answers cannot use them.\n\n` : '') +
                   `*Ask me anything about this document!*` });
      } else {
        discardSaved();
        updateMessage(messageId, { content: indexFailureNotice(result.fileName, result.error ?? '') });
      }
    } catch (error) {
      console.error('Upload error:', error);
      discardSaved();
      updateMessage(messageId, { content: indexFailureNotice(file.name, String(error)) });
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

  // Text read from an image: a notice in the chat, and an attachment of the
  // next question (features/ask/imageAttachments), sent as the user's own
  // context. Both go to the conversation that was open when the paste, drop or
  // pick started, even if another one is open by the time OCR finishes. The
  // drop listener outlives its render, so the conversation is read through a ref.
  const activeConversationIdRef = useRef(activeConversationId);
  activeConversationIdRef.current = activeConversationId;
  const showImageText = (conversationId: string | null, origin: ImageOrigin, raw: unknown) => {
    if (!conversationId) return;
    const result = readImageText(raw);
    appendMessageTo(conversationId, imageNotice(newNoticeId(), new Date().toISOString(), origin, result));
    // Only text the notice reports as found (OCR words) is attached.
    if (result.wordCount > 0) composerAttachments.add(conversationId, { id: newNoticeId(), origin, text: result.text });
  };

  // File picker for images
  const handlePickImage = async () => {
    const conversationId = activeConversationIdRef.current;
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
          showImageText(conversationId, 'added', result);
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
            const conversationId = activeConversationIdRef.current;

            try {
              // Images are read (OCR); old .doc/.ppt files are named with what
              // to do; documents, folders and other files go to the Library
              // (features/library/dropFiles).
              const dropped = sortDroppedPaths(files);

              for (const imageFile of dropped.images) {
                const result = await invoke<unknown>('process_image_from_file', {
                  filePath: imageFile
                });
                showImageText(conversationId, 'dropped', result);
              }

              if (dropped.legacy.length > 0) {
                notify.error('Old Office files are not indexed', { description: legacyOfficeNotice(dropped.legacy) });
              }

              if (dropped.library.length > 0) {
                // Files of a type the indexer does not read are named once, not added.
                const unsupported: string[] = [];
                for (const path of dropped.library) {
                  if (indexingBusyRef.current?.()) break;
                  let isDirectory = false;
                  try {
                    const stats = await invoke<{ isDirectory: boolean }>('check_path_type', { path });
                    isDirectory = stats.isDirectory;
                  } catch (e) {
                    console.warn('Failed to check path type, assuming file:', e);
                  }
                  if (!isDirectory && !isIndexableFile(path)) {
                    unsupported.push(path);
                    continue;
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
                  // Sequential: progress events carry no source id. A folder
                  // added by dropping it adopts its indexed files, as when picked.
                  await indexSourceRef.current?.(source, isDirectory ? 'folder' : 'file', !known);
                }
                if (unsupported.length > 0) {
                  notify.error('Some files were not added', { description: unsupportedFilesNotice(unsupported) });
                }
              }
            } catch (error) {
              console.error('❌ Failed to process dropped files:', error);
              appendMessage(failureNotice(newNoticeId(), new Date().toISOString(), 'The dropped files could not be added', String(error)));
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

  // An image pasted into the Ask composer (AgentComposer decides during the
  // paste event, so a text paste is never touched): its text is read and shown
  // as a notice. One at a time: a held Ctrl+V repeats the paste.
  const pasteInFlightRef = useRef(false);
  const handlePasteImage = async (image: PastedImage) => {
    if (pasteInFlightRef.current) return;
    const conversationId = activeConversationIdRef.current;
    pasteInFlightRef.current = true;
    try {
      const imageData = image.kind === 'file'
        ? await readFileAsDataUrl(image.file)
        : await invoke<string | null>('read_clipboard_image');
      // No image on the system clipboard either: nothing was pasted.
      if (!imageData) return;
      const result = await invoke<unknown>('process_image_from_base64', { imageData });
      showImageText(conversationId, 'pasted', result);
    } catch (error) {
      console.error('Failed to process pasted image:', error);
      notify.error('The pasted image could not be read', { description: String(error) });
    } finally {
      pasteInFlightRef.current = false;
    }
  };

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

  // Fetch the likely views' code while idle, once the first answer has
  // finished or a while after launch, and never while an answer streams.
  const preloadGate = useRef<ReturnType<typeof createIdlePreloadGate> | null>(null);
  useEffect(() => {
    const gate = createIdlePreloadGate();
    preloadGate.current = gate;
    return () => {
      gate.dispose();
      preloadGate.current = null;
    };
  }, []);
  const answerStreaming = streamingConversationId !== null;
  useEffect(() => {
    preloadGate.current?.setStreaming(answerStreaming);
  }, [answerStreaming]);

  // The command palette reaches every view in a keystroke: fetch their code as it opens.
  useEffect(() => {
    if (cmdPaletteOpen) preloadAllViews();
  }, [cmdPaletteOpen]);

  // Tab switches asked for by child components.
  useEffect(() => {
    const handleSwitchTab = (event: Event) => {
      const tab = normalizeViewTab((event as CustomEvent<unknown>).detail);
      if (tab) {
        // The assistant's navigations come this way: start the view's code now.
        preloadView(tab);
        setActiveTab(tab);
      }
    };
    window.addEventListener('switchTab', handleSwitchTab);
    return () => window.removeEventListener('switchTab', handleSwitchTab);
  }, []);

  // Indexing progress on the card of the source it names, while that source is
  // indexing (features/library/indexingProgress). Registered once; the source
  // being indexed is read through a ref.
  const currentlyIndexingRef = useRef(currentlyIndexing);
  currentlyIndexingRef.current = currentlyIndexing;
  useEffect(() => {
    let unlistenProg: (() => void) | null = null;
    let disposed = false;

    listen<IndexingProgressPayload>('indexing-progress', event => {
      const payload = event.payload;
      const currentStep = scannedPagesStep(payload);
      setSources(prev => prev.map(source => withIndexingProgress(source, payload, currentStep)));

      if (endsRun(payload, currentlyIndexingRef.current)) {
        setCurrentlyIndexing(null);
        // The sources as they are now: this listener outlives the render it was made in.
        void updateStats(sourcesRef.current);
      }
    }).then(fn => {
      if (disposed) fn();
      else unlistenProg = fn;
    });

    return () => {
      disposed = true;
      unlistenProg?.();
    };
  }, []);

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
        void refreshIndexedFiles();

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
        // The index did not open: first-run decides without its documents.
        setIndexedLoad(prev => (prev === 'loading' ? 'failed' : prev));
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
   * `adopt` (a folder being added): files of the folder already indexed
   * under a source the Library no longer lists, unchanged, are taken over
   * instead of indexed again. Indexing a source again leaves it off, so
   * every file is indexed.
   */
  const indexSource = async (source: Source, kind: 'folder' | 'file' = 'folder', adopt = false) => {
    setCurrentlyIndexing(source.id);
    setSources(prev => prev.map(s => s.id === source.id
      ? { ...s, status: 'indexing' as const, progress: 0, processedCount: 0, currentFile: undefined, currentStep: undefined, lastError: undefined, syncProgress: undefined }
      : s));
    try {
      const raw = kind === 'file'
        ? await invoke('index_single_file', { filePath: source.path, spaceId: source.id })
        : await invoke('link_folder_enhanced', {
            folderPath: source.path,
            spaceId: source.id,
            // Nested struct fields are snake_case. An empty file_types list
            // means every type the indexer supports.
            options: { skip_indexed: adopt, watch_changes: false, process_subdirs: true, priority: 'normal', file_types: [] },
          });
      const outcome = readIndexingResult(raw);
      setSources(prev => prev.map(s => s.id === source.id
        ? {
            ...s,
            status: 'ready' as const,
            fileCount: indexedFileCount(outcome),
            indexedAt: new Date().toISOString(),
            failures: outcome.failures.length > 0 ? outcome.failures : undefined,
            progress: undefined,
            currentFile: undefined,
            currentStep: undefined,
            processedCount: undefined,
          }
        : s));
      await updateStats();
      const failed = outcome.failures.length;
      notify.success(indexedNotice(outcome), {
        description: failed > 0 ? `${source.name} · ${failed} could not be indexed` : source.name,
      });
    } catch (error) {
      console.error('Indexing failed:', error);
      const message = searchErrorMessage(error);
      setSources(prev => prev.map(s => s.id === source.id
        ? { ...s, status: 'error' as const, lastError: message, progress: undefined, currentFile: undefined, currentStep: undefined, processedCount: undefined }
        : s));
      notify.error('Indexing failed', { description: `${source.name}: ${explainFailure(message, source.path).reason}` });
    } finally {
      setCurrentlyIndexing(null);
      void refreshIndexedFiles();
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
    await addFolderPath(selected);
  };

  /**
   * Add `picked` as a folder source and index it (index it again when it is
   * already listed). The caller checks that no index is running.
   */
  const addFolderPath = async (picked: string) => {
    const pathKey = (path: string) => path.replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();
    const existing = sourcesRef.current.find(s => pathKey(s.path) === pathKey(picked));
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
    await indexSource(newSource, 'folder', true);
  };

  /**
   * "Add these folders": each folder of the unlisted files becomes a folder
   * source, indexed in turn with all its subfolders, once the user agrees.
   */
  const handleAddUnlistedFolders = async () => {
    // A ref, not only the state: a second click can arrive before the re-render.
    if (unlistedBusyRef.current || addableUnlisted.length === 0 || indexingBusy()) return;
    unlistedBusyRef.current = true;
    setUnlistedBusy(true);
    try {
      // The home folder is normally read at startup; if not yet, read it now.
      // Unknown, it could be added unasked, so nothing is added.
      const home = homePath ?? (await homeDir().catch(() => null));
      if (!home) {
        notify.error('Could not check your home folder, so no folders were added.');
        return;
      }
      const folders = addableFolders(reconciliation.groups, sources, [home])
        .map(folder => nativePath(folder, IS_WINDOWS));
      if (folders.length === 0) {
        notify.info('No folders were added', { description: NOT_ADDED_REASON });
        return;
      }
      let confirmed: boolean;
      try {
        confirmed = await ask(
          `${folders.map(f => `• ${f}`).join('\n')}\n\nEach folder is indexed with all its subfolders and kept in sync.`,
          { title: `Add ${countOf(folders.length, 'folder')} to the Library?`, kind: 'info', okLabel: 'Add and index', cancelLabel: 'Cancel' },
        );
      } catch (error) {
        notify.error('Could not ask to add the folders', { description: String(error) });
        return;
      }
      if (!confirmed || indexingBusy()) return;
      for (const folder of folders) await addFolderPath(folder);
    } finally {
      unlistedBusyRef.current = false;
      setUnlistedBusy(false);
    }
  };

  /** Save the conversation as Markdown where the user picks. */
  const exportChat = async () => {
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

  /**
   * Start a chat about a Library file: a new conversation with the file named in the
   * composer by its name only; its path goes with the question as its search limit.
   */
  const handleAskAboutFile = (file: FileNode, _source: LibrarySource) => {
    // Outside workspaces: a workspace may not contain this file.
    createConversation({ workspaceId: null });
    setAskDraft(prev => ({
      text: fileQuestionDraft(file.name),
      file: { path: file.path, name: file.name },
      seq: (prev?.seq ?? 0) + 1,
    }));
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

    // Removed from the list now and from the index when the undo window ends.
    const index = sources.findIndex(s => s.id === id);
    const dropPending = () => setRemovingPaths(prev => {
      const at = prev.indexOf(source.path);
      return at < 0 ? prev : [...prev.slice(0, at), ...prev.slice(at + 1)];
    });
    removeWithUndo({
      message: 'Source removed',
      description: source.name,
      hide: () => {
        setSources(prev => prev.filter(s => s.id !== id));
        setRemovingPaths(prev => [...prev, source.path]);
      },
      restore: () => {
        setSources(prev => restoreAt(prev, source, index));
        dropPending();
      },
      commit: async () => {
        try {
          return await invoke<string>('delete_folder_source', { folderPath: source.path, sourceId: source.id });
        } finally {
          await refreshIndexedFiles();
          dropPending();
        }
      },
      onError: err => notify.error('The source was not removed', { description: String(err) }),
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
        conversationsLoaded={conversationsLoaded}
        activeConversationId={activeConversationId}
        onOpenConversation={(id: string) => { switchConversation(id); setActiveTab('ask'); }}
        onNewConversation={handleNewConversation}
        onNewItem={id => newItemActions[id]()}
        onRenameConversation={renameConversation}
        onPinConversation={pinConversation}
        onDeleteConversation={deleteConversation}
        sources={sources}
        onOpenCommandPalette={openPalette}
        onShowFeedback={() => setShowFeedback(true)}
        workspaces={allWorkspaces}
        onOpenWorkspace={id => openWorkspace(id)}
        onNewChatInWorkspace={id => startChat(id)}
        onMoveToWorkspace={(conversationId, workspaceId) => {
          updateConversationMeta(conversationId, { workspaceId: workspaceId ?? undefined });
          const name = workspaceId ? workspaceById(workspaceId)?.name : null;
          notify.success(name ? `Moved to “${name}”` : 'Moved out of its workspace', {
            description: name ? 'Its next answers search only this workspace’s sources.' : 'Its next answers search your whole library.',
          });
        }}
      />


      {/* Main Content Area */}
      <div className="flex-1 flex flex-col min-w-0">
        {/* Thin Title Bar */}
        <div
          className="flex items-center justify-between px-5 py-2 border-b"
          style={{ borderColor: colors.border, backgroundColor: colors.bg }}
        >
          <div className="flex items-center gap-3">
            <span className="inline-flex items-center">
              <span className="text-xs font-semibold tracking-wide" style={{ color: colors.text }}>
                {VIEW_TAB_LABELS[viewTab]}
              </span>
              <AssistantViewCue cue={viewCue} />
            </span>
            {viewTab === 'ask' && (() => {
              const workspace = workspaceById(activeConversation?.workspaceId);
              if (!workspace) return null;
              return (
                <button
                  type="button"
                  onClick={() => openWorkspace(workspace.id)}
                  className="text-caption px-2 py-0.5 rounded-full font-medium inline-flex items-center gap-1 max-w-[180px] bg-shodh-raised text-shodh-text-secondary hover:text-shodh-text transition-colors duration-micro focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  title={`Workspace: ${workspace.name} (${sourceSummary(workspace.sourceCounts)}). Open it.`}
                >
                  <WorkspaceIcon icon={workspace.icon} color={workspace.color} className="w-3 h-3" />
                  <span className="truncate">{workspace.name}</span>
                </button>
              );
            })()}
          </div>
          <div className="flex items-center gap-2">
            <InboxButton
              onNavigate={setActiveTab}
              onOpenConversation={id => { switchConversation(id); setActiveTab('ask'); }}
            />
            {viewTab === 'ask' && activeConversationId && (
              <ChatActionsMenu
                conversationId={activeConversationId}
                conversationTitle={activeConversation?.title ?? ''}
                messages={messages}
                onExport={messages.length > 0 ? () => void exportChat() : null}
              />
            )}
            {viewTab === 'ask' && activeConversationId && (
              <div className="relative">
                <button
                  ref={instructionsButtonRef}
                  type="button"
                  aria-expanded={showSystemPromptEditor}
                  aria-haspopup="dialog"
                  onClick={() => {
                    setShowSystemPromptEditor(!showSystemPromptEditor);
                    setEditingInstructionIdx(null);
                    setNewInstructionText('');
                  }}
                  className="text-caption px-2 py-0.5 rounded-full font-medium transition-colors"
                  style={{
                    backgroundColor: instructionsList.length > 0 ? `${colors.primary}14` : colors.bgTertiary,
                    color: instructionsList.length > 0 ? colors.primary : colors.textMuted,
                  }}
                  title={instructionsList.length > 0 ? `${instructionsList.length} instruction(s) active` : 'Set custom instructions for this chat'}
                >
                  <Settings className="w-3 h-3 inline mr-0.5" />
                  {instructionsList.length > 0 ? `Instructions (${instructionsList.length})` : 'Instructions'}
                </button>
                {showSystemPromptEditor && (
                  <>
                    <div className="fixed inset-0 z-40" onClick={() => { setShowSystemPromptEditor(false); setEditingInstructionIdx(null); }} />
                    <div
                      ref={instructionsPopoverRef}
                      role="dialog"
                      aria-label="Custom instructions"
                      className="absolute right-0 top-8 z-50 w-96 rounded-lg border shadow-xl"
                      style={{ backgroundColor: colors.bgSecondary, borderColor: colors.border }}
                    >
                      <div className="px-3 py-2 border-b flex items-center justify-between" style={{ borderColor: colors.border }}>
                        <div>
                          <span className="text-xs font-semibold" style={{ color: colors.text }}>Custom Instructions</span>
                          <p className="text-caption mt-0.5" style={{ color: colors.textMuted }}>
                            Applied to every AI response in this chat
                          </p>
                        </div>
                        <button
                          type="button"
                          onClick={() => { setShowSystemPromptEditor(false); setEditingInstructionIdx(null); instructionsButtonRef.current?.focus(); }}
                          className="w-5 h-5 rounded flex items-center justify-center"
                          style={{ color: colors.textMuted }}
                          aria-label="Close instructions"
                          title={withShortcut('Close', 'close', IS_MAC_PLATFORM)}
                        >
                          <X className="w-3.5 h-3.5" aria-hidden="true" />
                        </button>
                      </div>

                      {/* Saved instructions list */}
                      <div className="max-h-48 overflow-y-auto">
                        {instructionsList.length === 0 ? (
                          <div className="px-3 py-4 text-center">
                            <p className="text-caption" style={{ color: colors.textMuted }}>No instructions yet</p>
                            <p className="text-caption mt-0.5" style={{ color: colors.textTertiary }}>Add instructions below to guide AI responses</p>
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
                                <span className="text-caption mt-0.5 shrink-0 font-bold" style={{ color: colors.primary }}>•</span>
                                {editingInstructionIdx === idx ? (
                                  <div className="flex-1 flex items-center gap-1">
                                    <input
                                      type="text"
                                      value={editingInstructionText}
                                      onChange={e => setEditingInstructionText(e.target.value)}
                                      // Esc (cancel the edit) is handled by the popover's window listener.
                                      onKeyDown={e => {
                                        if (e.key === 'Enter') {
                                          commitEditInstruction();
                                          focusNewInstruction();
                                        }
                                      }}
                                      aria-label="Edit instruction"
                                      autoFocus
                                      className="flex-1 text-caption bg-transparent border-b outline-none py-0.5"
                                      style={{ color: colors.text, borderColor: colors.primary }}
                                    />
                                    <button
                                      type="button"
                                      onClick={() => { commitEditInstruction(); focusNewInstruction(); }}
                                      className="shrink-0 p-0.5 rounded"
                                      title="Save"
                                      aria-label="Save instruction"
                                    >
                                      <Check className="w-3 h-3" style={{ color: colors.success }} aria-hidden="true" />
                                    </button>
                                    <button
                                      type="button"
                                      onClick={() => { setEditingInstructionIdx(null); setEditingInstructionText(''); focusNewInstruction(); }}
                                      className="shrink-0 p-0.5 rounded"
                                      title={withShortcut('Cancel', 'close', IS_MAC_PLATFORM)}
                                      aria-label="Cancel editing"
                                    >
                                      <X className="w-3 h-3" style={{ color: colors.textMuted }} aria-hidden="true" />
                                    </button>
                                  </div>
                                ) : (
                                  <>
                                    <span className="flex-1 text-caption leading-snug" style={{ color: colors.textSecondary }}>
                                      {instruction}
                                    </span>
                                    <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 transition-opacity shrink-0">
                                      <button
                                        type="button"
                                        onClick={() => { setEditingInstructionIdx(idx); setEditingInstructionText(instruction); }}
                                        className="p-0.5 rounded transition-colors"
                                        style={{ color: colors.textMuted }}
                                        title="Edit"
                                        aria-label={`Edit instruction: ${instruction}`}
                                      >
                                        <Pencil className="w-3 h-3" aria-hidden="true" />
                                      </button>
                                      <button
                                        type="button"
                                        onClick={() => { removeInstruction(idx); notify.success('Instruction removed'); focusNewInstruction(); }}
                                        className="p-0.5 rounded transition-colors"
                                        style={{ color: colors.error }}
                                        title="Remove"
                                        aria-label={`Remove instruction: ${instruction}`}
                                      >
                                        <Trash2 className="w-3 h-3" aria-hidden="true" />
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
                            ref={newInstructionInputRef}
                            type="text"
                            value={newInstructionText}
                            onChange={e => setNewInstructionText(e.target.value)}
                            onKeyDown={e => { if (e.key === 'Enter') addInstruction(); }}
                            placeholder="Add an instruction..."
                            aria-label="New instruction"
                            // Focus starts here as the popover opens, and comes back here
                            // after Save, Cancel, Remove or Clear all (focusNewInstruction).
                            autoFocus
                            className="flex-1 text-caption rounded-md border px-2 py-1.5 outline-none"
                            style={{
                              backgroundColor: colors.inputBg,
                              borderColor: colors.border,
                              color: colors.text,
                            }}
                          />
                          <button
                            type="button"
                            onClick={addInstruction}
                            disabled={!newInstructionText.trim()}
                            className="px-2 py-1.5 rounded-md text-caption font-medium text-white transition-colors disabled:opacity-40"
                            style={{ backgroundColor: colors.primary }}
                            aria-label="Add instruction"
                            title="Add instruction"
                          >
                            <Plus className="w-3 h-3" aria-hidden="true" />
                          </button>
                        </div>
                        {instructionsList.length > 0 && (
                          <button
                            type="button"
                            onClick={() => {
                              saveInstructions([]);
                              notify.success('All instructions cleared');
                              focusNewInstruction();
                            }}
                            className="text-caption mt-1.5 transition-colors"
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
            {(() => {
              const chip = librarySourcesChip(searched);
              return (
                <Badge
                  variant="outline"
                  className="text-caption px-2 py-0.5 h-auto"
                  style={{ borderColor: colors.border, color: colors.textMuted }}
                  title={chip.title}
                >
                  <Database className="w-3 h-3 mr-1" />
                  {chip.label}
                </Badge>
              );
            })()}
          </div>
        </div>

        {initError && (
          <div role="alert" className="shrink-0 px-5 py-2 flex items-center gap-3 border-b border-shodh-border bg-shodh-warning-soft text-small text-shodh-text">
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

        {/*
          Content. <main> and its Suspense boundary stay mounted across switches:
          the deferred `viewTab` keeps the current view until the next one has
          rendered (a mounted boundary never falls back during a deferred
          update), and only the inner keyed frame changes, entering from most of
          its opacity with the shared motion tokens (no layout shift).
        */}
        <main
          id="main-content"
          tabIndex={-1}
          aria-label={VIEW_TAB_LABELS[viewTab]}
          className="flex-1 min-h-0 overflow-hidden focus:outline-none"
        >
          <Suspense fallback={<ViewSkeleton view={viewTab} />}>
          <div key={viewTab} data-view={viewTab} className="shell-view-enter h-full">
          {/* Ask Tab */}
          {viewTab === 'ask' && (
            <AskView
              sources={sources}
              searched={searched}
              llmStatus={llmStatus}
              onNavigate={setActiveTab}
              onPickImage={handlePickImage}
              onPasteImage={image => void handlePasteImage(image)}
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

          {/* Workspaces: sources, instructions, chats and memory of one piece of work */}
          {viewTab === 'workspaces' && (
            <WorkspacesView
              library={sources.map(s => ({ id: s.id, name: s.name, path: s.path }))}
              onNewChat={id => startChat(id)}
              onOpenChat={id => { switchConversation(id); setActiveTab('ask'); }}
            />
          )}

          {/* Tasks: list first, calendar as a view */}
          {viewTab === 'tasks' && <TasksView />}

          {/* Activity: usage and the audit log */}
          {viewTab === 'activity' && <ActivityView />}

          {/* Settings Tab */}
          {viewTab === 'settings' && (
            <SettingsView
              searchConfig={searchConfig}
              onUpdateSearchConfig={updateSearchConfig}
              onResetSearchConfig={resetSearchConfig}
              sources={sources}
              onRemoveSource={removeSource}
              onSourcesCleared={() => {
                clearViewCache();
                setSources([]);
                void refreshIndexedFiles();
              }}
              conversations={conversations}
              onOpenConversation={(id: string) => { switchConversation(id); setActiveTab('ask'); }}
            />
          )}

          {/* Library: folders, indexing progress and the file browser */}
          {viewTab === 'library' && (
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
              unlisted={reconciliation}
              addableFolders={addableUnlisted.length}
              onAddUnlisted={() => void handleAddUnlistedFolders()}
              unlistedBusy={unlistedBusy}
            />
          )}

          {/* Connectors: Slack and other tools Shodh answers in */}
          {viewTab === 'connectors' && <ConnectorsView />}
          </div>
          </Suspense>
        </main>
      </div>


      {/* Command Palette */}
      <CommandPalette
        open={cmdPaletteOpen}
        onClose={closePalette}
        onNavigate={setActiveTab}
        actions={paletteActions}
        conversations={conversations.map(c => ({
          id: c.id,
          title: c.title,
          updatedAt: c.updatedAt,
          workspaceName: workspaceById(c.workspaceId)?.name,
        }))}
        onOpenConversation={(id: string) => { switchConversation(id); setActiveTab('ask'); }}
        sources={sources.map(s => ({ id: s.id, name: s.name, path: s.path }))}
        workspaces={workspaces.map(w => ({ id: w.id, name: w.name, summary: sourceSummary(w.sourceCounts) }))}
        onOpenWorkspace={id => openWorkspace(id)}
        onNewChatInWorkspace={id => startChat(id)}
      />

      {/* First-run setup */}
      <FirstRunFlow
        open={firstRunOpen}
        step={firstRun.step}
        onStepChange={changeFirstRunStep}
        onFinish={finishFirstRun}
        onSkip={skipFirstRun}
        llmStatus={llmStatus}
        sources={sources}
        unlistedFiles={unlistedCount}
        onAddFolder={() => void handleAddSource()}
      />

      {/* New task / New event */}
      {quickCreate && (
        <Suspense fallback={null}>
          <QuickCreateDialog
            kind={quickCreate}
            onClose={() => setQuickCreate(null)}
            workspaces={workspaces.map(w => ({ id: w.id, name: w.name }))}
            defaultWorkspaceId={quickCreateWorkspace(activeConversation?.workspaceId, workspaces)}
          />
        </Suspense>
      )}

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
      <ConversationDock activeTab={viewTab} sources={sources} onExpand={() => setActiveTab('ask')} />

    </div>
  );
}

/** App shell with the chat session (and the focus pop-out it hosts) mounted above it. */
function AppSplitViewRoot() {
  return (
    <WorkspaceProvider>
      <ChatSessionProvider>
        <SystemStatusProvider>
          <FocusProvider>
            <AppSplitView />
            <VisualNavigator />
            <SnippetHost />
          </FocusProvider>
        </SystemStatusProvider>
      </ChatSessionProvider>
    </WorkspaceProvider>
  );
}

export default AppSplitViewRoot;

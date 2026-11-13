import { useState, useCallback, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { notify } from '../lib/notify';
import { removeWithUndo, restoreAt } from '../lib/undoToast';
import { markStartup } from '../lib/startupTiming';
import { diagramLedger } from '../features/ask/visual/diagramLedger';
import { diagramKeysToForget } from '../features/ask/visual/diagramKeys';
import { APP_QUITTING_EVENT, ConversationSaves, FailureLatch, loadFailureNotice, quitRequestId, updateRecord } from './conversationSaves';
import type { LoadFailureNotice } from './conversationSaves';
import {
  CONVERSATION_CHANGED_EVENT,
  NEW_CHAT_TITLE,
  autoTitle,
  changedConversationId,
  mergePatches,
  reloadedConversation,
  withPatch,
} from './conversationSync';
import type { MessagePatch } from './conversationSync';
import type { AgentMode } from '../features/agent/agentModes';
import { firstAskConversation } from '../features/connectors/conversations';
import type { ConversationSeed, ConversationSource } from '../features/connectors/conversations';

export interface ConversationMessage {
  id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  timestamp: string;
  artifacts?: any[];
  searchResults?: any[];
  /** Chat engine response metadata (camelCase `ResponseMetadata`). */
  metadata?: Record<string, unknown>;
  /** Run record observed while the answer streamed (see features/ask/types). */
  run?: Record<string, unknown>;
  /** Agent transcript of the answer (see features/agent/reducer). */
  transcript?: Record<string, unknown>;
}

export type ConversationMode = 'research' | 'code';

export interface Conversation {
  id: string;
  title: string;
  messages: ConversationMessage[];
  createdAt: string;
  updatedAt: string;
  pinned: boolean;
  /** Legacy: the Library source a chat was started from (kept as saved; see workspaceId). */
  spaceId?: string;
  spaceName?: string;
  /** The workspace the chat belongs to; absent is "No workspace". */
  workspaceId?: string;
  systemPrompt?: string;
  /** How answers work: Research (absent) or Code (the workspace's code folder). */
  mode?: ConversationMode;
  /**
   * What runs without asking (`features/agent/agentModes`), picked by the user; absent
   * follows the default in Settings. The agent never sets it.
   */
  agentMode?: AgentMode;
  /**
   * Side discussions not tied to a message (e.g. about a task). Opaque here;
   * read and written through features/focus/threadStore. Saved with the
   * conversation (`ConversationRecord.focus_threads`).
   */
  focusThreads?: unknown[];
  /**
   * Set on a conversation that happened in a connector (Slack, Telegram): written by the
   * backend only, shown read-only (`features/connectors/conversations`).
   */
  source?: ConversationSource;
  /** What a chat started with "Continue in Ask" carries into its first question. */
  seed?: ConversationSeed;
  /**
   * The file an "About <file>" chat is limited to, for the rest of the chat until the
   * user removes its chip (`features/ask/fileQuestion` reads it). Opaque here.
   */
  aboutFile?: unknown;
}

/** A conversation whose messages changed in the store and were reloaded (`seq` tells repeats apart). */
export interface ConversationReload {
  id: string;
  seq: number;
}

/** Emitted by the backend when the agent renames or pins a conversation. */
const CONVERSATION_UPDATED_EVENT = 'conversation-updated';

/** Emitted by the backend when a connector conversation is added to, created or removed. */
const CONNECTOR_CONVERSATION_EVENT = 'connector-conversation-changed';

interface ConnectorConversationChange {
  conversation: Conversation | null;
  removed: string[];
}

function parseConnectorChange(value: unknown): ConnectorConversationChange | null {
  if (typeof value !== 'object' || value === null) return null;
  const v = value as Record<string, unknown>;
  const removed = Array.isArray(v.removed) ? v.removed.filter((id): id is string => typeof id === 'string') : [];
  const c = v.conversation;
  if (c === undefined || c === null) return removed.length > 0 ? { conversation: null, removed } : null;
  if (typeof c !== 'object') return null;
  const record = c as Record<string, unknown>;
  if (typeof record.id !== 'string' || !Array.isArray(record.messages) || typeof record.source !== 'object' || record.source === null) return null;
  return { conversation: c as Conversation, removed };
}

interface ConversationChange {
  conversationId: string;
  title: string;
  pinned: boolean;
  updatedAt: string;
}

function parseConversationChange(value: unknown): ConversationChange | null {
  if (typeof value !== 'object' || value === null) return null;
  const v = value as Record<string, unknown>;
  if (typeof v.conversationId !== 'string' || typeof v.title !== 'string') return null;
  if (typeof v.pinned !== 'boolean' || typeof v.updatedAt !== 'string') return null;
  return { conversationId: v.conversationId, title: v.title, pinned: v.pinned, updatedAt: v.updatedAt };
}

function generateId(): string {
  return `conv-${Date.now()}-${Math.random().toString(36).substring(2, 8)}`;
}

/** Told when a change is not saved because saving is paused. */
function notifyNotSaved(what: string): void {
  notify.warning(`${what} was not saved`, {
    description: 'Saving is paused because your saved conversations could not be loaded.',
  });
}

export function useConversations() {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeConversationId, setActiveConversationId] = useState<string | null>(null);
  // The open conversation for listeners registered once (a removed one is left).
  const activeIdRef = useRef<string | null>(null);
  activeIdRef.current = activeConversationId;
  // The saved conversations could not be loaded: saving is paused and the
  // window shows why until the user acknowledges it.
  const [loadFailure, setLoadFailure] = useState<LoadFailureNotice | null>(null);
  const [loadFailureSeen, setLoadFailureSeen] = useState(false);
  // The saved conversations were read (or could not be): the list is final.
  const [loaded, setLoaded] = useState(false);
  // The latest conversation reloaded from the store (`conversation_changed`).
  const [reloaded, setReloaded] = useState<ConversationReload | null>(null);
  // A store that keeps refusing saves is reported once, not on every save.
  const [saveFailures] = useState(() => new FailureLatch());
  const reportSaveFailure = useCallback((id: string, err: unknown) => {
    console.error(`Failed to save conversation ${id}:`, err);
    if (saveFailures.fail()) {
      notify.error('Your conversation was not saved', {
        description: `${String(err)} It is saved again with your next change.`,
      });
    }
  }, [saveFailures]);
  // A conversation's own settings (workspace, mode, side threads, …), never its messages:
  // one debounce timer per conversation (a save for conversation A must not be cancelled
  // by a save for conversation B scheduled within the window), and writes of one
  // conversation in the order they were made.
  const [saves] = useState(() => new ConversationSaves<Conversation>({
    write: conversation => invoke('save_conversation', { conversation }),
    schedule: (run, ms) => {
      const timer = setTimeout(run, ms);
      return () => clearTimeout(timer);
    },
    onError: (conversation, err) => reportSaveFailure(conversation.id, err),
    onWritten: () => saveFailures.succeed(),
    delayMs: 500,
  }));
  // The messages this window changed, per conversation: only those are written (the agent
  // session saves its turns itself), and changes made within the delay go as one.
  const [messageSaves] = useState(() => new ConversationSaves<MessagePatch>({
    write: patch => invoke('save_conversation_messages', {
      conversationId: patch.id,
      messages: patch.messages,
      removed: patch.removed.length > 0 ? patch.removed : null,
    }),
    schedule: (run, ms) => {
      const timer = setTimeout(run, ms);
      return () => clearTimeout(timer);
    },
    onError: (patch, err) => reportSaveFailure(patch.id, err),
    onWritten: () => saveFailures.succeed(),
    delayMs: 500,
    merge: mergePatches,
  }));
  const loadedRef = useRef(false);
  // The list with every change made so far, rendered or not. Changes are
  // computed from it and saved outside React's state updaters (an updater
  // may run twice, or later), so a save writes exactly the record put in
  // the list, once, at the moment it is made.
  const listRef = useRef<Conversation[]>([]);
  const setList = useCallback((next: Conversation[]) => {
    listRef.current = next;
    setConversations(next);
  }, []);

  // The agent renamed or pinned a conversation (organize_conversation). The
  // backend saved it already; patch the in-memory copy so it shows.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<unknown>(CONVERSATION_UPDATED_EVENT, event => {
      const change = parseConversationChange(event.payload);
      if (!change) return;
      setList(listRef.current.map(c => (c.id === change.conversationId
        ? { ...c, title: change.title, pinned: change.pinned, updatedAt: change.updatedAt }
        : c)));
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(err => console.error(`Failed to listen for ${CONVERSATION_UPDATED_EVENT}:`, err));
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [setList]);

  // The messages of a conversation were written (an answer the agent session saved, a
  // question asked from another device, an edit in a window): reload it from the store,
  // keeping this window's own changes still waiting to be written. Reloads of one
  // conversation run one at a time; a change that arrives during one reloads again after.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    let seq = 0;
    const running = new Set<string>();
    const again = new Set<string>();
    const reload = (id: string) => {
      if (running.has(id)) {
        again.add(id);
        return;
      }
      running.add(id);
      invoke<Conversation | null>('load_conversation', { conversationId: id })
        .then(stored => {
          if (disposed || !stored) return;
          const existing = listRef.current.find(c => c.id === id);
          const merged = reloadedConversation(existing, stored, messageSaves.waiting(id));
          setList(existing
            ? listRef.current.map(c => (c.id === id ? merged : c))
            : [merged, ...listRef.current]);
          seq += 1;
          setReloaded({ id, seq });
        })
        .catch(err => console.error(`Failed to reload conversation ${id}:`, err))
        .finally(() => {
          running.delete(id);
          if (!disposed && again.delete(id)) reload(id);
        });
    };
    listen<unknown>(CONVERSATION_CHANGED_EVENT, event => {
      const id = changedConversationId(event.payload);
      if (id) reload(id);
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(err => console.error(`Failed to listen for ${CONVERSATION_CHANGED_EVENT}:`, err));
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [messageSaves, setList]);

  // A connector (Slack, Telegram) answered a question or forgot its conversations: the
  // backend saved it already; the list shows it now. The backend keeps the connector's
  // messages whatever the UI saves later.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<unknown>(CONNECTOR_CONVERSATION_EVENT, event => {
      const change = parseConnectorChange(event.payload);
      if (!change) return;
      let next = listRef.current;
      if (change.removed.length > 0) {
        const gone = new Set(change.removed);
        const removed = next.filter(c => gone.has(c.id));
        next = next.filter(c => !gone.has(c.id));
        // As for a deleted chat: what the diagram ledger kept for them goes too.
        for (const conversation of removed) {
          diagramLedger().forget(diagramKeysToForget(conversation, next));
        }
        if (activeIdRef.current && gone.has(activeIdRef.current)) {
          const fallback = firstAskConversation(next);
          if (fallback) {
            setActiveConversationId(fallback.id);
          } else {
            const now = new Date().toISOString();
            const fresh: Conversation = { id: generateId(), title: NEW_CHAT_TITLE, messages: [], createdAt: now, updatedAt: now, pinned: false };
            next = [fresh, ...next];
            setActiveConversationId(fresh.id);
            void saves.write(fresh);
          }
        }
      }
      const incoming = change.conversation;
      if (incoming) {
        const existing = next.find(c => c.id === incoming.id);
        const merged: Conversation = existing
          ? { ...incoming, title: existing.title, pinned: existing.pinned, focusThreads: existing.focusThreads }
          : incoming;
        next = [merged, ...next.filter(c => c.id !== incoming.id)];
      }
      setList(next);
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(err => console.error(`Failed to listen for ${CONNECTOR_CONVERSATION_EVENT}:`, err));
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [saves, setList]);

  // Load conversations on mount
  useEffect(() => {
    if (loadedRef.current) return;
    loadedRef.current = true;

    invoke<Conversation[]>('load_conversations')
      .then((saved) => {
        markStartup('conversations-loaded');
        setLoaded(true);
        // A connector's conversation never opens by itself: the newest Ask chat does.
        const first = firstAskConversation(saved);
        if (first) {
          setList(saved);
          setActiveConversationId(first.id);
        } else {
          const id = generateId();
          const now = new Date().toISOString();
          const fresh: Conversation = {
            id,
            title: NEW_CHAT_TITLE,
            messages: [],
            createdAt: now,
            updatedAt: now,
            pinned: false,
          };
          setList([fresh, ...saved]);
          setActiveConversationId(id);
          void saves.write(fresh);
        }
      })
      .catch((err) => {
        markStartup('conversations-loaded');
        setLoaded(true);
        console.error('Failed to load conversations:', err);
        // Nothing may overwrite the file the history is still in: saving
        // stays paused for this session, and the window says so.
        saves.pause();
        messageSaves.pause();
        setLoadFailure(loadFailureNotice(err));
        const id = generateId();
        const now = new Date().toISOString();
        const fresh: Conversation = {
          id,
          title: NEW_CHAT_TITLE,
          messages: [],
          createdAt: now,
          updatedAt: now,
          pinned: false,
        };
        setList([fresh]);
        setActiveConversationId(id);
      });
  }, [messageSaves, saves, setList]);

  const activeConversation = conversations.find(c => c.id === activeConversationId) || null;

  // Debounced save, per conversation; `immediate` writes now, replacing a save still
  // waiting for that conversation. `onSaved` runs only if this save is the one that
  // reaches the backend and succeeds. Waiting saves are written when the provider
  // unmounts (reload, hot update) or the page is hidden (also: closed to the tray) or
  // unloaded, and before the app exits (`app-quitting`), so a change made in the last
  // 500 ms is not dropped by a close or quit. Answers are saved by the backend.
  const scheduleSave = useCallback((conv: Conversation, onSaved?: () => void, immediate = false) => {
    saves.save(conv, onSaved, immediate);
  }, [saves]);

  useEffect(() => {
    const flush = () => {
      saves.flush();
      messageSaves.flush();
    };
    const onVisibility = () => {
      if (document.visibilityState === 'hidden') flush();
    };
    window.addEventListener('pagehide', flush);
    window.addEventListener('beforeunload', flush);
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      window.removeEventListener('pagehide', flush);
      window.removeEventListener('beforeunload', flush);
      document.removeEventListener('visibilitychange', onVisibility);
      flush();
    };
  }, [messageSaves, saves]);

  // The app is about to exit (tray Quit, closing the window, installing an
  // update): write every waiting save, wait for the writes, then let it go.
  // The backend exits anyway after a bounded wait.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<unknown>(APP_QUITTING_EVENT, event => {
      const requestId = quitRequestId(event.payload);
      if (requestId === null) return;
      void Promise.all([saves.drain(), messageSaves.drain()])
        .then(() => invoke('quit_ready', { requestId }))
        .catch(err => console.error('Failed to confirm the saves before quitting:', err));
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(err => console.error(`Failed to listen for ${APP_QUITTING_EVENT}:`, err));
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [messageSaves, saves]);

  const createConversation = useCallback((opts?: { workspaceId?: string | null }): string => {
    const id = generateId();
    const now = new Date().toISOString();
    const fresh: Conversation = {
      id,
      title: NEW_CHAT_TITLE,
      messages: [],
      createdAt: now,
      updatedAt: now,
      pinned: false,
      ...(opts?.workspaceId ? { workspaceId: opts.workspaceId } : {}),
    };
    setList([fresh, ...listRef.current]);
    setActiveConversationId(id);
    void saves.write(fresh);
    return id;
  }, [saves, setList]);

  const switchConversation = useCallback((id: string) => {
    setActiveConversationId(id);
  }, []);

  /** A conversation with every change made so far, rendered or not. */
  const getConversation = useCallback((id: string): Conversation | null => (
    listRef.current.find(c => c.id === id) ?? null
  ), []);

  /**
   * Save messages this window changed in a conversation (visible or not): each replaces
   * the stored message with its id, or is added at the end, and `removed` go. The list is
   * never written whole: turns the agent session saved, or another device added, stay.
   * Takes an explicit id so a change that completes after the user switched
   * conversations is written to the conversation it belongs to.
   */
  const saveMessages = useCallback(
    (
      conversationId: string,
      messages: ConversationMessage[],
      removed: string[] = [],
      options: { immediate?: boolean } = {},
    ) => {
      if (messages.length === 0 && removed.length === 0) return;
      const patch: MessagePatch = { id: conversationId, messages, removed };
      const { list, updated } = updateRecord(listRef.current, conversationId, conv => withPatch(conv, patch, new Date().toISOString()));
      if (!updated) return;
      setList(list);
      messageSaves.save(patch, undefined, options.immediate === true);
    },
    [messageSaves, setList]
  );

  /**
   * Show the conversation's name from the question just asked, before the agent session
   * saves the question (and names it the same way). Display only.
   */
  const previewTitle = useCallback((conversationId: string, question: string) => {
    const conv = listRef.current.find(c => c.id === conversationId);
    if (!conv || conv.title !== NEW_CHAT_TITLE) return;
    setList(listRef.current.map(c => (c.id === conversationId ? { ...c, title: autoTitle(question) } : c)));
  }, [setList]);

  const renameConversation = useCallback((id: string, title: string) => {
    setList(listRef.current.map(c => (c.id === id ? { ...c, title, updatedAt: new Date().toISOString() } : c)));
    if (saves.paused) {
      notifyNotSaved('The new name');
      return;
    }
    invoke('rename_conversation', { conversationId: id, newTitle: title })
      .then(() => notify.success('Conversation renamed'))
      .catch(err => notify.error('The conversation was not renamed', { description: String(err) }));
  }, [saves, setList]);

  const deleteConversation = useCallback(
    (id: string) => {
      if (saves.paused) {
        notifyNotSaved('Deleting the conversation');
        return;
      }
      const index = listRef.current.findIndex(c => c.id === id);
      if (index < 0) return;
      const conversation = listRef.current[index];
      const wasActive = id === activeConversationId;
      removeWithUndo({
        message: 'Conversation deleted',
        description: conversation.title !== NEW_CHAT_TITLE ? conversation.title : undefined,
        hide: () => {
          const remaining = listRef.current.filter(c => c.id !== id);
          if (remaining.length === 0) {
            const freshId = generateId();
            const now = new Date().toISOString();
            const fresh: Conversation = {
              id: freshId,
              title: NEW_CHAT_TITLE,
              messages: [],
              createdAt: now,
              updatedAt: now,
              pinned: false,
            };
            setList([fresh]);
            setActiveConversationId(freshId);
            void saves.write(fresh);
            return;
          }
          setList(remaining);
          if (wasActive) setActiveConversationId(remaining[0].id);
        },
        // The same record at the same place in the list; it was never deleted on disk.
        restore: () => {
          setList(restoreAt(listRef.current, conversation, index));
          if (wasActive) setActiveConversationId(id);
        },
        commit: async () => {
          await invoke('delete_conversation', { conversationId: id });
          // Its diagrams' versions, notes and lists go with it (kept where another conversation draws the same diagram).
          diagramLedger().forget(diagramKeysToForget(conversation, listRef.current.filter(c => c.id !== id)));
        },
        onError: err => notify.error('The conversation was not deleted', { description: String(err) }),
      });
    },
    [activeConversationId, saves, setList]
  );

  /**
   * "Continue in Ask": a new Ask chat carrying `seed` (a connector conversation as quoted
   * context for its first question), opened at once.
   */
  const createSeededConversation = useCallback((seed: ConversationSeed): string => {
    const id = generateId();
    const now = new Date().toISOString();
    const fresh: Conversation = {
      id,
      title: `Continued: ${seed.label}`,
      messages: [],
      createdAt: now,
      updatedAt: now,
      pinned: false,
      seed,
    };
    setList([fresh, ...listRef.current]);
    setActiveConversationId(id);
    void saves.write(fresh);
    return id;
  }, [saves, setList]);

  const updateConversationMeta = useCallback((id: string, meta: Partial<Pick<Conversation, 'workspaceId' | 'systemPrompt' | 'mode' | 'agentMode' | 'seed' | 'aboutFile'>>) => {
    const { list, updated } = updateRecord(listRef.current, id, c => ({ ...c, ...meta, updatedAt: new Date().toISOString() }));
    if (!updated) return;
    setList(list);
    scheduleSave(updated);
  }, [scheduleSave, setList]);

  /**
   * Change a conversation's side threads that have no parent message. With
   * `touch: false` the conversation keeps its place in the list (used when
   * moving threads kept by an older version into the conversation).
   */
  const updateFocusThreads = useCallback((
    id: string,
    update: (prev: unknown[] | undefined) => unknown[],
    options: { touch?: boolean; onSaved?: () => void } = {},
  ) => {
    const { list, updated } = updateRecord(listRef.current, id, c => {
      const next = update(c.focusThreads);
      return {
        ...c,
        focusThreads: next.length > 0 ? next : undefined,
        ...(options.touch === false ? {} : { updatedAt: new Date().toISOString() }),
      };
    });
    if (!updated) return;
    setList(list);
    scheduleSave(updated, options.onSaved);
  }, [scheduleSave, setList]);

  /**
   * A workspace was deleted: its chats move to "No workspace". The backend already saved
   * that; the in-memory copies change so their next save keeps it.
   */
  const detachWorkspace = useCallback((workspaceId: string) => {
    setList(listRef.current.map(c => {
      if (c.workspaceId !== workspaceId) return c;
      const { workspaceId: _removed, ...rest } = c;
      return rest;
    }));
  }, [setList]);

  const pinConversation = useCallback((id: string) => {
    const conv = listRef.current.find(c => c.id === id);
    if (!conv) return;
    const newPinned = !conv.pinned;
    setList(listRef.current.map(c => (c.id === id ? { ...c, pinned: newPinned, updatedAt: new Date().toISOString() } : c)));
    if (saves.paused) {
      notifyNotSaved(newPinned ? 'Pinning' : 'Unpinning');
      return;
    }
    invoke('pin_conversation', { conversationId: id, pinned: newPinned })
      .then(() => notify.success(newPinned ? 'Conversation pinned' : 'Conversation unpinned'))
      .catch(err => notify.error('The pin was not saved', { description: String(err) }));
  }, [saves, setList]);

  const reorderConversations = useCallback((reordered: Conversation[]) => {
    setList(reordered);
    // Persist the new order — save each conversation so the backend knows
    for (const conv of reordered) {
      void saves.write(conv);
    }
  }, [saves, setList]);

  const acknowledgeLoadFailure = useCallback(() => setLoadFailureSeen(true), []);

  return {
    conversations,
    /** The saved conversations were read (or failed to be): an empty view is not loading. */
    loaded,
    /** Why the saved conversations could not be loaded (saving is paused); null when they loaded. */
    loadFailure,
    /** The user saw the load failure notice; saving stays paused. */
    loadFailureSeen,
    acknowledgeLoadFailure,
    /** The latest conversation reloaded from the store after its messages changed there. */
    reloaded,
    activeConversationId,
    activeConversation,
    createConversation,
    createSeededConversation,
    switchConversation,
    getConversation,
    saveMessages,
    previewTitle,
    renameConversation,
    deleteConversation,
    pinConversation,
    updateConversationMeta,
    updateFocusThreads,
    reorderConversations,
    detachWorkspace,
  };
}

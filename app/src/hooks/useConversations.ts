import { useState, useCallback, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { notify } from '../lib/notify';
import { removeWithUndo, restoreAt } from '../lib/undoToast';
import { markStartup } from '../lib/startupTiming';
import { ConversationSaves } from './conversationSaves';

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
   * Side discussions not tied to a message (e.g. about a task). Opaque here;
   * read and written through features/focus/threadStore. Saved with the
   * conversation (`ConversationRecord.focus_threads`).
   */
  focusThreads?: unknown[];
}

/** Emitted by the backend when the agent renames or pins a conversation. */
const CONVERSATION_UPDATED_EVENT = 'conversation-updated';

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

function autoTitle(firstMessage: string): string {
  const words = firstMessage.trim().split(/\s+/).slice(0, 6);
  let title = words.join(' ');
  if (firstMessage.trim().split(/\s+/).length > 6) title += '...';
  return title || 'New Chat';
}

export function useConversations() {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeConversationId, setActiveConversationId] = useState<string | null>(null);
  // One debounce timer per conversation (a save for conversation A must not
  // be cancelled by a save for conversation B scheduled within the window),
  // and writes of one conversation in the order they were made.
  const [saves] = useState(() => new ConversationSaves<Conversation>({
    write: conversation => invoke('save_conversation', { conversation }),
    schedule: (run, ms) => {
      const timer = setTimeout(run, ms);
      return () => clearTimeout(timer);
    },
    onError: (conversation, err) => console.error(`Failed to save conversation ${conversation.id}:`, err),
    delayMs: 500,
  }));
  const loadedRef = useRef(false);
  // The list as last rendered, for removals that need a record's place in it.
  const latestConversations = useRef<Conversation[]>([]);
  latestConversations.current = conversations;

  // The agent renamed or pinned a conversation (organize_conversation). The
  // backend saved it already; patch the in-memory copy so the next save of
  // the whole record keeps the change.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<unknown>(CONVERSATION_UPDATED_EVENT, event => {
      const change = parseConversationChange(event.payload);
      if (!change) return;
      setConversations(prev => prev.map(c => (c.id === change.conversationId
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
  }, []);

  // Load conversations on mount
  useEffect(() => {
    if (loadedRef.current) return;
    loadedRef.current = true;

    invoke<Conversation[]>('load_conversations')
      .then((loaded) => {
        markStartup('conversations-loaded');
        if (loaded.length > 0) {
          setConversations(loaded);
          setActiveConversationId(loaded[0].id);
        } else {
          const id = generateId();
          const now = new Date().toISOString();
          const fresh: Conversation = {
            id,
            title: 'New Chat',
            messages: [],
            createdAt: now,
            updatedAt: now,
            pinned: false,
          };
          setConversations([fresh]);
          setActiveConversationId(id);
          invoke('save_conversation', { conversation: fresh }).catch(console.error);
        }
      })
      .catch((err) => {
        markStartup('conversations-loaded');
        console.error('Failed to load conversations:', err);
        const id = generateId();
        const now = new Date().toISOString();
        const fresh: Conversation = {
          id,
          title: 'New Chat',
          messages: [],
          createdAt: now,
          updatedAt: now,
          pinned: false,
        };
        setConversations([fresh]);
        setActiveConversationId(id);
      });
  }, []);

  const activeConversation = conversations.find(c => c.id === activeConversationId) || null;

  // Debounced save, per conversation; `immediate` writes now (a finished
  // answer), replacing a save still waiting for that conversation. `onSaved`
  // runs only if this save is the one that reaches the backend and succeeds.
  // Waiting saves are written when the provider unmounts (reload, hot
  // update) or the page is hidden (also: closed to the tray) or unloaded, so
  // a normal close does not drop a change made in the last 500 ms. A hard
  // kill or the tray's Quit (`app.exit`) skips these hooks; a finished
  // answer is saved immediately for that reason.
  const scheduleSave = useCallback((conv: Conversation, onSaved?: () => void, immediate = false) => {
    saves.save(conv, onSaved, immediate);
  }, [saves]);

  useEffect(() => {
    const flush = () => saves.flush();
    const onVisibility = () => {
      if (document.visibilityState === 'hidden') saves.flush();
    };
    window.addEventListener('pagehide', flush);
    window.addEventListener('beforeunload', flush);
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      window.removeEventListener('pagehide', flush);
      window.removeEventListener('beforeunload', flush);
      document.removeEventListener('visibilitychange', onVisibility);
      saves.flush();
    };
  }, [saves]);

  const createConversation = useCallback((opts?: { workspaceId?: string | null }): string => {
    const id = generateId();
    const now = new Date().toISOString();
    const fresh: Conversation = {
      id,
      title: 'New Chat',
      messages: [],
      createdAt: now,
      updatedAt: now,
      pinned: false,
      ...(opts?.workspaceId ? { workspaceId: opts.workspaceId } : {}),
    };
    setConversations(prev => [fresh, ...prev]);
    setActiveConversationId(id);
    void saves.write(fresh);
    return id;
  }, [saves]);

  const switchConversation = useCallback((id: string) => {
    setActiveConversationId(id);
  }, []);

  /**
   * Update the messages of a specific conversation. Takes an explicit id so a
   * response that completes after the user switched conversations is written
   * to the conversation it belongs to.
   */
  const updateConversationMessages = useCallback(
    (
      conversationId: string,
      updater: (prev: ConversationMessage[]) => ConversationMessage[],
      options: { immediate?: boolean } = {},
    ) => {
      setConversations(prev => {
        return prev.map(conv => {
          if (conv.id !== conversationId) return conv;
          const newMessages = updater(conv.messages);
          const updated = {
            ...conv,
            messages: newMessages,
            updatedAt: new Date().toISOString(),
          };
          // Auto-title from first user message
          if (conv.title === 'New Chat' && newMessages.length > 0) {
            const firstUser = newMessages.find(m => m.role === 'user');
            if (firstUser) {
              updated.title = autoTitle(firstUser.content);
            }
          }
          scheduleSave(updated, undefined, options.immediate === true);
          return updated;
        });
      });
    },
    [scheduleSave]
  );

  const updateActiveMessages = useCallback(
    (updater: (prev: ConversationMessage[]) => ConversationMessage[]) => {
      if (!activeConversationId) return;
      updateConversationMessages(activeConversationId, updater);
    },
    [activeConversationId, updateConversationMessages]
  );

  const appendMessage = useCallback(
    (msg: ConversationMessage) => {
      updateActiveMessages(prev => [...prev, msg]);
    },
    [updateActiveMessages]
  );

  const renameConversation = useCallback((id: string, title: string) => {
    setConversations(prev =>
      prev.map(c => (c.id === id ? { ...c, title, updatedAt: new Date().toISOString() } : c))
    );
    invoke('rename_conversation', { conversationId: id, newTitle: title }).catch(console.error);
    notify.success('Conversation renamed');
  }, []);

  const deleteConversation = useCallback(
    (id: string) => {
      const index = latestConversations.current.findIndex(c => c.id === id);
      if (index < 0) return;
      const conversation = latestConversations.current[index];
      const wasActive = id === activeConversationId;
      removeWithUndo({
        message: 'Conversation deleted',
        description: conversation.title !== 'New Chat' ? conversation.title : undefined,
        hide: () => {
          setConversations(prev => {
            const remaining = prev.filter(c => c.id !== id);
            if (remaining.length === 0) {
              const freshId = generateId();
              const now = new Date().toISOString();
              const fresh: Conversation = {
                id: freshId,
                title: 'New Chat',
                messages: [],
                createdAt: now,
                updatedAt: now,
                pinned: false,
              };
              setActiveConversationId(freshId);
              invoke('save_conversation', { conversation: fresh }).catch(console.error);
              return [fresh];
            }
            if (wasActive) setActiveConversationId(remaining[0].id);
            return remaining;
          });
        },
        // The same record at the same place in the list; it was never deleted on disk.
        restore: () => {
          setConversations(prev => restoreAt(prev, conversation, index));
          if (wasActive) setActiveConversationId(id);
        },
        commit: () => invoke('delete_conversation', { conversationId: id }),
        onError: err => notify.error('The conversation was not deleted', { description: String(err) }),
      });
    },
    [activeConversationId]
  );

  const updateConversationMeta = useCallback((id: string, meta: Partial<Pick<Conversation, 'workspaceId' | 'systemPrompt' | 'mode'>>) => {
    setConversations(prev =>
      prev.map(c => {
        if (c.id !== id) return c;
        const updated = { ...c, ...meta, updatedAt: new Date().toISOString() };
        scheduleSave(updated);
        return updated;
      })
    );
  }, [scheduleSave]);

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
    setConversations(prev =>
      prev.map(c => {
        if (c.id !== id) return c;
        const next = update(c.focusThreads);
        const updated: Conversation = {
          ...c,
          focusThreads: next.length > 0 ? next : undefined,
          ...(options.touch === false ? {} : { updatedAt: new Date().toISOString() }),
        };
        scheduleSave(updated, options.onSaved);
        return updated;
      })
    );
  }, [scheduleSave]);

  /**
   * A workspace was deleted: its chats move to "No workspace". The backend already saved
   * that; the in-memory copies change so their next save keeps it.
   */
  const detachWorkspace = useCallback((workspaceId: string) => {
    setConversations(prev =>
      prev.map(c => {
        if (c.workspaceId !== workspaceId) return c;
        const { workspaceId: _removed, ...rest } = c;
        return rest;
      })
    );
  }, []);

  const pinConversation = useCallback((id: string) => {
    setConversations(prev =>
      prev.map(c =>
        c.id === id ? { ...c, pinned: !c.pinned, updatedAt: new Date().toISOString() } : c
      )
    );
    const conv = conversations.find(c => c.id === id);
    const newPinned = !(conv?.pinned ?? false);
    invoke('pin_conversation', { conversationId: id, pinned: newPinned }).catch(console.error);
    notify.success(newPinned ? 'Conversation pinned' : 'Conversation unpinned');
  }, [conversations]);

  const reorderConversations = useCallback((reordered: Conversation[]) => {
    setConversations(reordered);
    // Persist the new order — save each conversation so the backend knows
    for (const conv of reordered) {
      invoke('save_conversation', { conversation: conv }).catch(console.error);
    }
  }, []);

  return {
    conversations,
    activeConversationId,
    activeConversation,
    createConversation,
    switchConversation,
    updateConversationMessages,
    updateActiveMessages,
    appendMessage,
    renameConversation,
    deleteConversation,
    pinConversation,
    updateConversationMeta,
    updateFocusThreads,
    reorderConversations,
    detachWorkspace,
  };
}

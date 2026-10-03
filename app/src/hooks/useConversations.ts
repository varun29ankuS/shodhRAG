import { useState, useCallback, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { notify } from '../lib/notify';
import { markStartup } from '../lib/startupTiming';

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

export interface Conversation {
  id: string;
  title: string;
  messages: ConversationMessage[];
  createdAt: string;
  updatedAt: string;
  pinned: boolean;
  spaceId?: string;
  spaceName?: string;
  systemPrompt?: string;
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
  // One debounce timer per conversation: a save for conversation A must not
  // be cancelled by a save for conversation B scheduled within the window.
  const saveTimersRef = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());
  const loadedRef = useRef(false);
  const pendingDeleteRef = useRef<Map<string, { timeout: ReturnType<typeof setTimeout>; conversation: Conversation }>>(new Map());

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

  // Debounced save, per conversation
  const scheduleSave = useCallback((conv: Conversation) => {
    const timers = saveTimersRef.current;
    const existing = timers.get(conv.id);
    if (existing) clearTimeout(existing);
    timers.set(conv.id, setTimeout(() => {
      timers.delete(conv.id);
      invoke('save_conversation', { conversation: conv }).catch(console.error);
    }, 500));
  }, []);

  const createConversation = useCallback((opts?: { spaceId?: string; spaceName?: string }): string => {
    const id = generateId();
    const now = new Date().toISOString();
    const fresh: Conversation = {
      id,
      title: 'New Chat',
      messages: [],
      createdAt: now,
      updatedAt: now,
      pinned: false,
      spaceId: opts?.spaceId,
      spaceName: opts?.spaceName,
    };
    setConversations(prev => [fresh, ...prev]);
    setActiveConversationId(id);
    invoke('save_conversation', { conversation: fresh }).catch(console.error);
    return id;
  }, []);

  const switchConversation = useCallback((id: string) => {
    setActiveConversationId(id);
  }, []);

  /**
   * Update the messages of a specific conversation. Takes an explicit id so a
   * response that completes after the user switched conversations is written
   * to the conversation it belongs to.
   */
  const updateConversationMessages = useCallback(
    (conversationId: string, updater: (prev: ConversationMessage[]) => ConversationMessage[]) => {
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
          scheduleSave(updated);
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
      // Cancel any existing pending delete for this ID
      const existing = pendingDeleteRef.current.get(id);
      if (existing) {
        clearTimeout(existing.timeout);
        pendingDeleteRef.current.delete(id);
      }

      // Save the conversation data for potential undo
      let deletedConversation: Conversation | undefined;

      setConversations(prev => {
        deletedConversation = prev.find(c => c.id === id);
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
        if (id === activeConversationId) {
          setActiveConversationId(remaining[0].id);
        }
        return remaining;
      });

      if (!deletedConversation) return;

      // Schedule actual backend deletion after 5s
      const conv = deletedConversation;
      const timeout = setTimeout(() => {
        pendingDeleteRef.current.delete(id);
        invoke('delete_conversation', { conversationId: id }).catch(console.error);
      }, 5000);

      pendingDeleteRef.current.set(id, { timeout, conversation: conv });

      toast('Conversation deleted', {
        description: conv.title !== 'New Chat' ? conv.title : undefined,
        action: {
          label: 'Undo',
          onClick: () => {
            const pending = pendingDeleteRef.current.get(id);
            if (pending) {
              clearTimeout(pending.timeout);
              pendingDeleteRef.current.delete(id);
              setConversations(prev => {
                // Insert back in original position (prepend for simplicity)
                return [pending.conversation, ...prev];
              });
              setActiveConversationId(id);
              notify.success('Conversation restored');
            }
          },
        },
        duration: 5000,
      });
    },
    [activeConversationId]
  );

  const updateConversationMeta = useCallback((id: string, meta: Partial<Pick<Conversation, 'spaceId' | 'spaceName' | 'systemPrompt'>>) => {
    setConversations(prev =>
      prev.map(c => {
        if (c.id !== id) return c;
        const updated = { ...c, ...meta, updatedAt: new Date().toISOString() };
        scheduleSave(updated);
        return updated;
      })
    );
  }, [scheduleSave]);

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
    reorderConversations,
  };
}

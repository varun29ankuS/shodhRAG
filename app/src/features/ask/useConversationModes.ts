/**
 * The active conversation's mode, as the composer's one mode menu shows and changes it:
 * what the answer works on (Research, Code, Creator) and its permission (Ask first, Accept
 * edits, Plan, Auto). Ask, the in-page chat and the floating chat all read and change it
 * here, so the three never disagree. Only the user changes it: the menu, Shift+Tab in the
 * composer, or a suggestion's button.
 */
import { useCallback, useRef, useState } from 'react';
import { useChatSession } from './ChatSessionContext';
import { useCodeModeEnabled } from '../../hooks/useCodeModeEnabled';
import { useDefaultAgentMode } from '../../hooks/useDefaultAgentMode';
import { confirmCreator, creatorConfirmed, nextAgentMode, resolveAgentMode } from '../agent/agentModes';
import type { AgentMode } from '../agent/agentModes';
import { isPermissionMode, workChange, workKindOf } from '../agent/composerRow';
import type { ConversationWork, WorkKind } from '../agent/composerRow';

/**
 * The permission each conversation had before it was switched to Creator, for this app
 * session: leaving Creator gives it back.
 */
const permissionBeforeCreator = new Map<string, AgentMode>();

export interface ConversationModes {
  conversationId: string | null;
  /** Research or Code, the conversation's own mode. */
  conversationMode: ConversationWork;
  /** The agent mode the next answer runs in; null while the default is read. */
  agentMode: AgentMode | null;
  /** What the answer works on (the menu's first section). */
  work: WorkKind;
  /** The permission (never Creator); null while the default is read. */
  permission: AgentMode | null;
  /** Code mode in Settings; null until read. */
  codeAvailable: boolean | null;
  /** The default agent mode in Settings; null until read. */
  defaultAgentMode: AgentMode | null;
  pickWork: (work: WorkKind) => void;
  /** Set the agent mode (a permission, or Creator after its confirming click). */
  changeAgentMode: (mode: AgentMode) => void;
  /** Shift+Tab: the next of Ask first, Accept edits and Plan. */
  cycle: () => void;
}

export function useConversationModes(): ConversationModes {
  const session = useChatSession();
  const conversationId = session.activeConversationId;
  const { updateConversationMeta } = session;
  const conversationMode: ConversationWork = session.activeConversation?.mode ?? 'research';
  const codeAvailable = useCodeModeEnabled();
  const defaultAgentMode = useDefaultAgentMode();
  // Bumped when the user turns Creator on, so the view reads the confirmation.
  const [creatorTurnedOn, setCreatorTurnedOn] = useState(0);
  const agentMode: AgentMode | null = resolveAgentMode(
    session.activeConversation?.agentMode,
    session.messages.length > 0,
    defaultAgentMode,
    creatorTurnedOn >= 0 && creatorConfirmed(conversationId),
  );
  const work = workKindOf(conversationMode, agentMode);
  const permission = agentMode === null || agentMode === 'creator' ? null : agentMode;

  const changeAgentMode = useCallback((next: AgentMode) => {
    const id = conversationId;
    if (!id) return;
    // The menu hands Creator over only after its confirming click.
    if (next === 'creator') {
      confirmCreator(id);
      setCreatorTurnedOn(n => n + 1);
    }
    updateConversationMeta(id, { agentMode: next });
  }, [conversationId, updateConversationMeta]);

  const state = useRef({ conversationMode, agentMode, defaultAgentMode });
  state.current = { conversationMode, agentMode, defaultAgentMode };

  const pickWork = useCallback((next: WorkKind) => {
    const id = conversationId;
    if (!id) return;
    const now = state.current;
    if (next === 'creator' && isPermissionMode(now.agentMode)) permissionBeforeCreator.set(id, now.agentMode);
    const change = workChange(next, now, permissionBeforeCreator.get(id) ?? null, now.defaultAgentMode);
    if (change.conversationMode) updateConversationMeta(id, { mode: change.conversationMode });
    if (change.agentMode) changeAgentMode(change.agentMode);
  }, [conversationId, updateConversationMeta, changeAgentMode]);

  const cycle = useCallback(() => {
    const current = state.current.agentMode;
    if (current) changeAgentMode(nextAgentMode(current));
  }, [changeAgentMode]);

  return { conversationId, conversationMode, agentMode, work, permission, codeAvailable, defaultAgentMode, pickWork, changeAgentMode, cycle };
}

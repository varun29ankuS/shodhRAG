import type { AgentMode } from '../agent/agentModes';

/** What the composer needs to know to decide whether Send works now. */
export interface SendBlockInput {
  /** The question waits for the search models and the answer engine. */
  waiting: boolean;
  /** The conversation's mode; null while the default is being read. */
  agentMode: AgentMode | null;
  /** The label of a side question being answered, if any. */
  sideRunLabel: string | null;
  /** An answer is being written in another conversation. */
  busyElsewhere: boolean;
  /** Research or Code, for this conversation. */
  conversationMode: 'research' | 'code';
  /** Code mode in Settings → Privacy; null until read (nothing is blocked on a guess). */
  codeModeEnabled: boolean | null;
  /** Why the workspace's code folder cannot be used, from the Code switch. */
  codeProblem: string | null;
  /** An answer is running in this conversation (it can be steered, not blocked). */
  streaming: boolean;
}

/** Why Send is blocked, and the one-click fix when there is one. */
export interface SendBlock {
  reason: string;
  /** Turning Code mode on clears the block. */
  fix: 'turnOnCodeMode' | null;
}

export const CODE_MODE_OFF_REASON = 'Code mode is off.';
export const CREATOR_CODE_MODE_OFF_REASON = 'Creator needs Code mode, which is off.';

/**
 * Whether Send is blocked. Creator builds in the conversation's own creation folder,
 * whatever the workspace, so the workspace's code-folder problem never blocks it
 * (the backend's `agent_start` does not read that folder for Creator either). Both
 * need Code mode on; that block comes with a fix the composer offers as a button.
 */
export function sendBlock(input: SendBlockInput): SendBlock | null {
  const plain = (reason: string): SendBlock => ({ reason, fix: null });
  if (input.waiting) return plain('Sends when Shodh is ready.');
  if (input.agentMode === null) return plain('Reading your default mode…');
  if (input.sideRunLabel !== null) {
    return plain(`Answering your side question about “${input.sideRunLabel}”.`);
  }
  if (input.busyElsewhere) {
    return plain('Answering in another conversation.');
  }
  if (input.streaming) return null;
  const creator = input.agentMode === 'creator';
  if (input.codeModeEnabled === false && (creator || input.conversationMode === 'code')) {
    return { reason: creator ? CREATOR_CODE_MODE_OFF_REASON : CODE_MODE_OFF_REASON, fix: 'turnOnCodeMode' };
  }
  if (!creator && input.conversationMode === 'code' && input.codeProblem) {
    return plain(input.codeProblem);
  }
  return null;
}

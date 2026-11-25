import { useCallback, useEffect, useRef, useState } from 'react';
import type React from 'react';
import { GitBranch } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import type { ConversationMode } from '../../hooks/useConversations';
import { agentApi, toAgentError } from './agentApi';
import type { CodeStatus } from './agentApi';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

/** How long "Discard changes" waits for its confirming second press. */
const CONFIRM_WINDOW_MS = 5000;

/**
 * The code folder's state for a Code conversation, read when Code mode is on and again
 * after each answer: the folder, why it cannot be used (`problem`, which blocks Send),
 * and the branch the conversation's changes are on.
 */
export function useCodeStatus(
  conversationId: string | null,
  workspaceId: string | null,
  mode: ConversationMode,
  answerRunning: boolean,
): { status: CodeStatus | null; setStatus: React.Dispatch<React.SetStateAction<CodeStatus | null>> } {
  const [status, setStatus] = useState<CodeStatus | null>(null);

  useEffect(() => {
    setStatus(null);
  }, [conversationId]);

  useEffect(() => {
    if (mode !== 'code' || !conversationId || answerRunning) return;
    let active = true;
    agentApi
      .codeStatus(conversationId, workspaceId)
      .then(next => { if (active) setStatus(next); })
      .catch(error => {
        if (active) setStatus({ folder: null, problem: toAgentError(error).message, branch: null });
      });
    return () => { active = false; };
  }, [mode, conversationId, workspaceId, answerRunning]);

  return { status: mode === 'code' ? status : null, setStatus };
}

interface CodeBranchChipProps {
  conversationId: string;
  status: CodeStatus;
  /** The changes were discarded: the conversation is back on its base branch. */
  onDiscarded: () => void;
  answerRunning: boolean;
}

/**
 * The branch a Code conversation's changes are on, in Ask's top bar, with "Discard
 * changes" (the branch is kept for inspection). Renders nothing before there is a branch.
 */
export function CodeBranchChip({ conversationId, status, onDiscarded, answerRunning }: CodeBranchChipProps) {
  const [confirming, setConfirming] = useState(false);
  const [discarding, setDiscarding] = useState(false);
  const confirmTimer = useRef<number | null>(null);

  useEffect(() => () => {
    if (confirmTimer.current !== null) window.clearTimeout(confirmTimer.current);
  }, []);

  useEffect(() => {
    setConfirming(false);
  }, [conversationId]);

  const discard = useCallback(() => {
    if (discarding) return;
    if (!confirming) {
      setConfirming(true);
      confirmTimer.current = window.setTimeout(() => setConfirming(false), CONFIRM_WINDOW_MS);
      return;
    }
    if (confirmTimer.current !== null) window.clearTimeout(confirmTimer.current);
    setConfirming(false);
    setDiscarding(true);
    agentApi
      .discardCodeChanges(conversationId)
      .then(branch => {
        notify.success(`Changes discarded: back on ${branch.base}`, {
          description: `The work is kept on ${branch.branch}.`,
        });
        onDiscarded();
      })
      .catch(error => notify.error('Could not discard the changes', { description: toAgentError(error).message }))
      .finally(() => setDiscarding(false));
  }, [conversationId, confirming, discarding, onDiscarded]);

  const branch = status.branch;
  if (!branch) return null;

  return (
    <span
      data-testid="topbar-code-branch"
      className="inline-flex items-center gap-1.5 min-w-0 h-[26px] text-caption text-shodh-text-muted"
      title={status.folder ? `${branch.branch} (from ${branch.base}) · ${status.folder}` : `${branch.branch} (from ${branch.base})`}
    >
      <GitBranch className="w-3 h-3 shrink-0" aria-hidden="true" />
      <span className="truncate max-w-[140px] font-mono">{branch.branch}</span>
      <button
        type="button"
        onClick={discard}
        disabled={discarding || answerRunning}
        aria-live="polite"
        title={`Back to ${branch.base}; work kept on ${branch.branch}`}
        className={cn(
          'shrink-0 whitespace-nowrap h-6 px-2 rounded-full text-caption font-medium border transition-colors duration-micro disabled:opacity-50 disabled:cursor-not-allowed',
          confirming
            ? 'border-shodh-error text-shodh-error hover:bg-shodh-raised'
            : 'border-shodh-border text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised',
          FOCUS_RING,
        )}
      >
        {discarding ? 'Discarding…' : confirming ? 'Confirm discard' : 'Discard changes'}
      </button>
    </span>
  );
}

export default CodeBranchChip;

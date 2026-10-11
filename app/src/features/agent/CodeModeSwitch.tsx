import { useCallback, useEffect, useRef, useState } from 'react';
import { Code2, GitBranch, Search } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import type { ConversationMode } from '../../hooks/useConversations';
import { agentApi, toAgentError } from './useAgentSession';
import type { ApprovalLevel, CodeStatus } from './useAgentSession';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

/** The approval level as the mode chip's tooltip states it. */
const APPROVAL_SUMMARY: Record<ApprovalLevel, string> = {
  askEveryTime: 'ask every time',
  autoApplyEdits: "edits apply by themselves on the conversation's branch; commands ask",
  trusted: 'edits apply by themselves; allowed commands run without asking',
};

/** How long "Discard changes" waits for its confirming second press. */
const CONFIRM_WINDOW_MS = 5000;

const MODES: readonly { id: ConversationMode; label: string; hint: string; Icon: typeof Search }[] = [
  { id: 'research', label: 'Research', hint: 'Answers from your library and the web', Icon: Search },
  { id: 'code', label: 'Code', hint: "Reads and changes the workspace's code folder", Icon: Code2 },
];

interface CodeModeSwitchProps {
  conversationId: string | null;
  workspaceId: string | null;
  mode: ConversationMode;
  onChange: (mode: ConversationMode) => void;
  /** An answer is running: a switch applies to the next one. */
  answerRunning: boolean;
  /** Why Code mode cannot be used right now (null when it can), for the composer. */
  onProblem: (problem: string | null) => void;
}

/**
 * "Research | Code" in the Ask composer, per conversation. In Code mode it
 * shows the branch the conversation's changes are on and offers to discard
 * them (the branch is kept for inspection).
 */
export function CodeModeSwitch({ conversationId, workspaceId, mode, onChange, answerRunning, onProblem }: CodeModeSwitchProps) {
  const [status, setStatus] = useState<CodeStatus | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [discarding, setDiscarding] = useState(false);
  const confirmTimer = useRef<number | null>(null);

  // Read the code folder's state when Code mode is on, and again after each answer.
  useEffect(() => {
    if (mode !== 'code' || !conversationId || answerRunning) return;
    let active = true;
    agentApi
      .codeStatus(conversationId, workspaceId)
      .then(next => { if (active) setStatus(next); })
      .catch(error => {
        if (active) setStatus({ folder: null, problem: toAgentError(error).message, branch: null, approval: 'askEveryTime' });
      });
    return () => { active = false; };
  }, [mode, conversationId, workspaceId, answerRunning]);

  useEffect(() => {
    onProblem(mode === 'code' ? status?.problem ?? null : null);
  }, [mode, status, onProblem]);

  useEffect(() => () => {
    if (confirmTimer.current !== null) window.clearTimeout(confirmTimer.current);
  }, []);

  useEffect(() => {
    setConfirming(false);
    setStatus(null);
  }, [conversationId]);

  const discard = useCallback(() => {
    if (!conversationId || discarding) return;
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
        setStatus(prev => (prev ? { ...prev, branch: null } : prev));
      })
      .catch(error => notify.error('Could not discard the changes', { description: toAgentError(error).message }))
      .finally(() => setDiscarding(false));
  }, [conversationId, confirming, discarding]);

  const branch = mode === 'code' ? status?.branch ?? null : null;
  const folderTitle = status?.folder
    ? `Code folder: ${status.folder}. Approvals: ${APPROVAL_SUMMARY[status.approval] ?? APPROVAL_SUMMARY.askEveryTime} (set in the workspace's Overview)`
    : status?.problem ?? undefined;

  return (
    <div className="flex items-center gap-1.5 shrink-0">
      <div
        role="radiogroup"
        aria-label="Answer mode"
        className="inline-flex items-center h-[30px] p-0.5 rounded-full bg-shodh-raised-2 shrink-0"
        onKeyDown={e => {
          if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
          e.preventDefault();
          const next = mode === 'research' ? 'code' : 'research';
          onChange(next);
          const target = e.currentTarget.querySelector<HTMLButtonElement>(`[data-mode="${next}"]`);
          target?.focus();
        }}
      >
        {MODES.map(({ id, label, hint, Icon }) => {
          const selected = mode === id;
          const title = answerRunning && !selected
            ? `${hint}. Applies to the next answer.`
            : id === 'code' && folderTitle ? `${hint}. ${folderTitle}` : hint;
          return (
            <button
              key={id}
              type="button"
              role="radio"
              data-mode={id}
              aria-checked={selected}
              tabIndex={selected ? 0 : -1}
              title={title}
              onClick={() => { if (!selected) onChange(id); }}
              className={cn(
                'inline-flex items-center gap-1 h-[26px] px-2.5 rounded-full text-[12px] font-medium transition-colors duration-micro',
                selected
                  ? 'bg-shodh-surface text-shodh-text shadow-sm'
                  : 'text-shodh-text-muted hover:text-shodh-text',
                FOCUS_RING,
              )}
            >
              <Icon className="w-3 h-3" aria-hidden="true" />
              {label}
            </button>
          );
        })}
      </div>
      {branch && (
        <span className="inline-flex items-center gap-1 min-w-0 text-[11.5px] text-shodh-text-muted" title={`Changes are on ${branch.branch} (from ${branch.base})`}>
          <GitBranch className="w-3 h-3 shrink-0" aria-hidden="true" />
          <span className="truncate max-w-[140px] font-mono">{branch.branch}</span>
          <button
            type="button"
            onClick={discard}
            disabled={discarding || answerRunning}
            aria-live="polite"
            title={`Commit this conversation's changes on ${branch.branch} and go back to ${branch.base}`}
            className={cn(
              'h-6 px-2 rounded-full text-[11.5px] font-medium border transition-colors duration-micro disabled:opacity-50 disabled:cursor-not-allowed',
              confirming
                ? 'border-shodh-error text-shodh-error hover:bg-shodh-raised'
                : 'border-shodh-border text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised',
              FOCUS_RING,
            )}
          >
            {discarding ? 'Discarding…' : confirming ? 'Confirm discard' : 'Discard changes'}
          </button>
        </span>
      )}
    </div>
  );
}

export default CodeModeSwitch;

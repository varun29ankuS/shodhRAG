import { useState } from 'react';
import { cn } from '../../lib/utils';
import { AUTO_CONFIRM_TEXT, PLAN_RUN_MODES, agentModeInfo } from './agentModes';
import type { AgentMode, ModeSuggestion } from './agentModes';
import { MODE_ICONS } from './ModePicker';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

const BUTTON = cn(
  'inline-flex items-center gap-1.5 h-7 px-2.5 rounded-full text-caption font-medium border border-shodh-border text-shodh-text-secondary hover:text-shodh-text hover:bg-shodh-raised transition-colors duration-micro disabled:opacity-50 disabled:cursor-not-allowed',
  FOCUS_RING,
);

interface ModeActionsProps {
  /** The conversation's mode now. */
  mode: AgentMode;
  /** The answer was written in Plan mode and finished: it can be approved. */
  plan: boolean;
  /** The mode the assistant suggested in this answer, if any. */
  suggestion: ModeSuggestion | null;
  /** Sending is not possible right now (another answer runs). */
  disabled: boolean;
  /** Approve the plan and run it in `mode`. */
  onApprovePlan: (mode: AgentMode) => void;
  /** Switch to the suggested mode (nothing is sent). */
  onSwitch: (mode: AgentMode) => void;
}

/**
 * Under the newest answer: "Approve plan" in each mode that can carry it out (an
 * answer written in Plan mode), and the mode the assistant suggested. Every button is
 * the user's: the assistant cannot switch the mode. Nothing here is highlighted by
 * what the assistant wrote, and its reason is shown as its words, apart from the
 * button.
 */
export function ModeActions({ mode, plan, suggestion, disabled, onApprovePlan, onSwitch }: ModeActionsProps) {
  const [confirmingAuto, setConfirmingAuto] = useState(false);
  const showSuggestion = suggestion !== null && !plan && suggestion.mode !== mode;
  if (!plan && !showSuggestion) return null;
  const switchTo = (next: AgentMode) => {
    if (next === 'auto' && !confirmingAuto) {
      setConfirmingAuto(true);
      return;
    }
    setConfirmingAuto(false);
    onSwitch(next);
  };
  return (
    <div className="flex flex-col gap-2" data-mode-actions={mode}>
      {plan && (
        <div role="group" aria-label="Approve the plan" className="flex flex-wrap items-center gap-2">
          <span className="text-caption text-shodh-text-muted">Approve the plan and run it in:</span>
          {PLAN_RUN_MODES.map(next => {
            const Icon = MODE_ICONS[next];
            const info = agentModeInfo(next);
            return (
              <button
                key={next}
                type="button"
                disabled={disabled}
                onClick={() => onApprovePlan(next)}
                title={`Switch this conversation to ${info.label} and go ahead with the plan. ${info.hint}`}
                className={BUTTON}
              >
                <Icon className="w-3 h-3 shrink-0" aria-hidden="true" />
                {`Approve plan · ${info.label}`}
              </button>
            );
          })}
        </div>
      )}
      {showSuggestion && suggestion && (
        <div className="flex flex-col gap-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-caption text-shodh-text-secondary">
              {`The assistant suggests ${agentModeInfo(suggestion.mode).label} mode. You decide.`}
            </span>
            <SwitchButton mode={suggestion.mode} onSwitch={switchTo} />
          </div>
          {suggestion.reason && (
            <p className="m-0 text-caption text-shodh-text-muted">
              {`The assistant's reason, in its words: “${suggestion.reason}”`}
            </p>
          )}
          {confirmingAuto && (
            <div role="group" aria-label="Turn on Auto" className="flex flex-wrap items-center gap-2">
              <span className="text-caption text-shodh-text-secondary">{AUTO_CONFIRM_TEXT}</span>
              <button type="button" onClick={() => switchTo('auto')} className={BUTTON}>
                Turn on Auto
              </button>
              <button type="button" onClick={() => setConfirmingAuto(false)} className={BUTTON}>
                Cancel
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function SwitchButton({ mode, onSwitch }: { mode: AgentMode; onSwitch: (mode: AgentMode) => void }) {
  const Icon = MODE_ICONS[mode];
  const info = agentModeInfo(mode);
  return (
    <button type="button" onClick={() => onSwitch(mode)} title={info.hint} className={BUTTON}>
      <Icon className="w-3 h-3 shrink-0" aria-hidden="true" />
      {`Switch to ${info.label}`}
    </button>
  );
}

export default ModeActions;

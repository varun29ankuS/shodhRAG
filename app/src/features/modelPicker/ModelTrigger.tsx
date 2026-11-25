import { forwardRef } from 'react';
import type React from 'react';
import { AlertTriangle, ChevronDown, Lock } from 'lucide-react';
import { cn } from '../../lib/utils';
import { COMPOSER_CONTROL_HEIGHT, COMPOSER_TEST_IDS } from '../agent/composerRow';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

interface ModelTriggerProps extends Omit<React.ButtonHTMLAttributes<HTMLButtonElement>, 'children'> {
  /** The model's name, or what to do when there is none ("Choose a model"). */
  label: string;
  /** A model answers (one accent dot); false shows a warning icon and the label says why. */
  ready: boolean;
  /** Why the control is locked now, or null. */
  lockedReason?: string | null;
}

/**
 * The model control in the composer row: one look in Ask (which opens the model menu) and
 * in the floating chat. One state colour: the accent dot when a model answers; otherwise an
 * icon, and the label says what is wrong. Pure, so the floating chat renders it too.
 */
export const ModelTrigger = forwardRef<HTMLButtonElement, ModelTriggerProps>(function ModelTrigger(
  { label, ready, lockedReason = null, className, disabled, title, ...rest },
  ref,
) {
  const locked = lockedReason !== null;
  return (
    <button
      ref={ref}
      type="button"
      {...rest}
      disabled={locked || disabled}
      title={lockedReason !== null ? lockedReason : title ?? label}
      data-testid={COMPOSER_TEST_IDS.model}
      data-ready={ready}
      className={cn(
        'inline-flex items-center gap-[7px] min-w-0 max-w-full shrink px-2.5 rounded-full bg-shodh-raised-2 text-small text-shodh-text-secondary hover:bg-shodh-pressed hover:text-shodh-text transition-colors duration-micro disabled:opacity-60 disabled:cursor-not-allowed disabled:hover:bg-shodh-raised-2',
        COMPOSER_CONTROL_HEIGHT,
        FOCUS_RING,
        className,
      )}
    >
      {locked ? (
        <Lock className="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
      ) : ready ? (
        <span className="w-[7px] h-[7px] rounded-full shrink-0 bg-shodh-accent" aria-hidden="true" />
      ) : (
        <AlertTriangle className="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
      )}
      <span className="min-w-0 truncate">{label}</span>
      <ChevronDown className="w-3 h-3 shrink-0" aria-hidden="true" />
    </button>
  );
});

export default ModelTrigger;

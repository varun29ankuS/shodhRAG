import { forwardRef } from 'react';
import type React from 'react';
import { AlertTriangle, ChevronDown, Lock } from 'lucide-react';
import { cn } from '../../lib/utils';
import { COMPOSER_CONTROL_HEIGHT, COMPOSER_TEST_IDS } from '../agent/composerRow';
import { DOT_CLASS } from '../connection/connectionStatus';
import type { Dot } from '../connection/connectionStatus';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

interface ModelTriggerProps extends Omit<React.ButtonHTMLAttributes<HTMLButtonElement>, 'children'> {
  /** The model's name, or what to do when there is none ("Choose a model"). */
  label: string;
  /** A model is chosen (its state's dot shows); false shows a warning icon and the label says why. */
  ready: boolean;
  /**
   * The chosen model's state, the colour the sidebar's model status shows for it
   * (`modelDot`): working, rejected, limited, or not known.
   */
  dot: Dot;
  /** Why the control is locked now, or null. */
  lockedReason?: string | null;
}

/**
 * The model control in the composer row: one look in Ask and in the floating chat. A chosen
 * model shows the dot of its state, in the colours of the sidebar's model status (one
 * mapping, `DOT_CLASS`); with no model, an icon, and the label says what to do. Pure, so
 * the floating chat renders it too.
 */
export const ModelTrigger = forwardRef<HTMLButtonElement, ModelTriggerProps>(function ModelTrigger(
  { label, ready, dot, lockedReason = null, className, disabled, title, ...rest },
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
      data-model-state={ready ? dot : 'none'}
      className={cn(
        'inline-flex items-center gap-1.5 min-w-0 max-w-full shrink px-2.5 rounded-full bg-shodh-raised-2 text-small text-shodh-text-secondary hover:bg-shodh-pressed hover:text-shodh-text transition-colors duration-micro disabled:opacity-60 disabled:cursor-not-allowed disabled:hover:bg-shodh-raised-2',
        COMPOSER_CONTROL_HEIGHT,
        FOCUS_RING,
        className,
      )}
    >
      {locked ? (
        <Lock className="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
      ) : ready ? (
        <span className={cn('w-2 h-2 rounded-full shrink-0', DOT_CLASS[dot])} aria-hidden="true" />
      ) : (
        <AlertTriangle className="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
      )}
      <span className="min-w-0 truncate">{label}</span>
      <ChevronDown className="w-3 h-3 shrink-0" aria-hidden="true" />
    </button>
  );
});

export default ModelTrigger;

import type React from 'react';
import { AlertTriangle, Inbox, Loader2, RotateCcw, WifiOff } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { cn } from '../../lib/utils';

/** What a view shows in place of its content. */
export type ViewStateKind = 'empty' | 'loading' | 'error' | 'offline';

const ICONS: Record<ViewStateKind, LucideIcon> = {
  empty: Inbox,
  loading: Loader2,
  error: AlertTriangle,
  offline: WifiOff,
};

interface ViewStateProps {
  kind: ViewStateKind;
  /** One line: what the state is ("No workspaces yet", "Workspaces could not be loaded"). */
  title: string;
  /** One more line when the state needs its reason (an error's message). */
  detail?: string | null;
  /** The one thing to do about it ("Add folder"). */
  action?: React.ReactNode;
  /** Read again: drawn as "Try again", the same button wherever a view could not load. */
  onRetry?: () => void;
  /** The empty state's own picture, in place of the tray. */
  icon?: LucideIcon;
  className?: string;
}

/**
 * The four states every main view has besides its content, drawn one way: nothing here yet,
 * loading, could not load, and cannot reach what it needs. One line, one reason at most, and
 * one action. An error is announced at once (`alert`); the others when the reader gets
 * there (`status`).
 */
export function ViewState({ kind, title, detail = null, action, onRetry, icon, className }: ViewStateProps) {
  const Icon = icon ?? ICONS[kind];
  return (
    <div
      role={kind === 'error' ? 'alert' : 'status'}
      data-view-state={kind}
      className={cn(
        'flex flex-col items-start gap-3 rounded-xl border border-dashed border-shodh-border px-6 py-6',
        kind === 'error' && 'border-solid border-shodh-error/40 bg-shodh-error/10',
        className,
      )}
    >
      <Icon
        className={cn(
          'w-6 h-6',
          kind === 'error' ? 'text-shodh-error' : kind === 'offline' ? 'text-shodh-warning' : 'text-shodh-text-muted',
          kind === 'loading' && 'animate-spin motion-reduce:animate-none',
        )}
        aria-hidden="true"
      />
      <p className="m-0 text-body-lg font-semibold text-shodh-text">{title}</p>
      {detail && <p className="m-0 max-w-[640px] text-small text-shodh-text-secondary break-words">{detail}</p>}
      {action}
      {onRetry && (
        <button
          type="button"
          onClick={onRetry}
          className="inline-flex items-center gap-1.5 h-8 px-3 rounded-lg border border-shodh-border bg-shodh-surface text-small font-medium text-shodh-text hover:bg-shodh-raised transition-colors duration-micro focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
          Try again
        </button>
      )}
    </div>
  );
}

export default ViewState;

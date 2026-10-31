import React, { useId, useState } from 'react';
import { ChevronDown } from 'lucide-react';
import { cn } from '../../lib/utils';
import { FOCUS_RING } from '../workspaces/ui';
import type { Pill, Tone } from './slackModel';

export { FOCUS_RING, INPUT, LABEL, OUTLINE_BUTTON, PRIMARY_BUTTON, QUIET_BUTTON } from '../workspaces/ui';

export const DANGER_BUTTON = cn(
  'h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border text-small text-shodh-error hover:bg-shodh-raised disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
  FOCUS_RING,
);

export const TONE_DOT: Record<Tone, string> = {
  neutral: 'bg-shodh-text-faint',
  active: 'bg-shodh-info animate-pulse motion-reduce:animate-none',
  ok: 'bg-shodh-success',
  warning: 'bg-shodh-warning',
  error: 'bg-shodh-error',
};

export const TONE_TEXT: Record<Tone, string> = {
  neutral: 'text-shodh-text-muted',
  active: 'text-shodh-text-muted',
  ok: 'text-shodh-text-muted',
  warning: 'text-shodh-warning',
  error: 'text-shodh-error',
};

const PILL_SURFACE: Record<Tone, string> = {
  neutral: 'bg-shodh-raised text-shodh-text-secondary',
  active: 'bg-shodh-raised text-shodh-text-secondary',
  ok: 'bg-shodh-success-soft text-shodh-success',
  warning: 'bg-shodh-warning-soft text-shodh-warning',
  error: 'bg-shodh-warning-soft text-shodh-error',
};

/**
 * The connector's status, as a pill: a dot, the word, and the reason when there is one.
 * `wrap`: the reason wraps onto further lines instead of being cut (a narrow tile, where
 * a cut reason would hide what needs the user).
 */
export function StatusPill({ pill, className, wrap = false }: { pill: Pill; className?: string; wrap?: boolean }) {
  return (
    <span
      className={cn(
        'inline-flex gap-1.5 max-w-full px-2 text-caption font-medium',
        wrap ? 'items-start min-h-6 py-[5px] leading-[14px] rounded-xl' : 'items-center h-6 rounded-full',
        PILL_SURFACE[pill.tone],
        className,
      )}
    >
      <span aria-hidden="true" className={cn('w-1.5 h-1.5 rounded-full shrink-0', wrap && 'mt-1', TONE_DOT[pill.tone])} />
      <span className={wrap ? 'min-w-0 break-words' : 'truncate'}>
        {pill.label}
        {pill.reason && <span className="font-normal">{`: ${pill.reason}`}</span>}
      </span>
    </span>
  );
}

/**
 * A connector page's row of tabs. It scrolls sideways when the page is too narrow for
 * it, and never up and down: the baseline is an inset shadow the tabs' underline paints
 * over, so no tab reaches below the row (a `-mb-px` over a `border-b` did, and with
 * `overflow-x: auto` the row then scrolled vertically by that pixel), and the tabs' focus
 * ring is drawn inside them.
 */
export const TABLIST = 'flex gap-1 overflow-x-auto overflow-y-hidden scrollbar-thin shadow-[inset_0_-1px_0_0_var(--c-border-subtle)]';

/** One tab of {@link TABLIST}. */
export function tabClass(selected: boolean): string {
  return cn(
    'shrink-0 h-9 px-3 inline-flex items-center gap-1.5 text-small border-b-2 transition-colors duration-micro',
    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring',
    selected ? 'border-shodh-accent text-shodh-text font-semibold' : 'border-transparent text-shodh-text-muted hover:text-shodh-text',
  );
}

/** A short line with the long text one click away. */
export function LearnMore({ summary, children, label = 'Learn more' }: { summary?: React.ReactNode; children: React.ReactNode; label?: string }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  return (
    <div className="flex flex-col gap-1">
      <p className="m-0 text-small text-shodh-text-muted">
        {summary}
        {summary ? ' ' : null}
        <button
          type="button"
          aria-expanded={open}
          aria-controls={id}
          onClick={() => setOpen(o => !o)}
          className={cn('inline-flex items-center gap-0.5 rounded text-shodh-accent-text hover:underline', FOCUS_RING)}
        >
          {open ? 'Show less' : label}
          <ChevronDown className={cn('w-3 h-3 transition-transform duration-micro motion-reduce:transition-none', open && 'rotate-180')} aria-hidden="true" />
        </button>
      </p>
      <div id={id} hidden={!open} className="text-small leading-[1.55] text-shodh-text-secondary max-w-[68ch]">
        {children}
      </div>
    </div>
  );
}

/** A grey bar standing in for text while it loads. */
export function Skeleton({ className }: { className?: string }) {
  return <span aria-hidden="true" className={cn('block rounded bg-shodh-raised animate-pulse motion-reduce:animate-none', className)} />;
}

/** A titled part of a tab. */
export function Section({
  title,
  description,
  children,
  actions,
}: {
  title: string;
  description?: React.ReactNode;
  children?: React.ReactNode;
  actions?: React.ReactNode;
}) {
  const id = useId();
  return (
    <section aria-labelledby={id} className="flex flex-col gap-3">
      <div className="flex items-start gap-3">
        <div className="flex-1 min-w-0">
          <h3 id={id} className="m-0 text-body font-semibold text-shodh-text">{title}</h3>
          {description && <div className="m-0 mt-0.5 text-small text-shodh-text-muted max-w-[68ch]">{description}</div>}
        </div>
        {actions && <div className="shrink-0 flex items-center gap-2">{actions}</div>}
      </div>
      {children}
    </section>
  );
}

/** A note that something needs the user (or is just worth knowing). */
export function Callout({ tone, children, role }: { tone: Tone; children: React.ReactNode; role?: 'alert' | 'status' }) {
  return (
    <div
      role={role}
      className={cn(
        'flex items-start gap-2.5 rounded-lg border px-3.5 py-3 text-small',
        tone === 'error'
          ? 'border-shodh-error/40 bg-shodh-surface-2 text-shodh-text-secondary'
          : tone === 'warning'
            ? 'border-shodh-warning/40 bg-shodh-warning-soft text-shodh-text-secondary'
            : 'border-shodh-border bg-shodh-surface-2 text-shodh-text-secondary',
      )}
    >
      <span aria-hidden="true" className={cn('mt-1.5 w-1.5 h-1.5 rounded-full shrink-0', TONE_DOT[tone])} />
      <div className="min-w-0 flex-1">{children}</div>
    </div>
  );
}

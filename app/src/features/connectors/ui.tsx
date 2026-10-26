import React, { useId, useState } from 'react';
import { ChevronDown } from 'lucide-react';
import { cn } from '../../lib/utils';
import { FOCUS_RING } from '../workspaces/ui';
import type { Pill, Tone } from './slackModel';

export { FOCUS_RING, INPUT, LABEL, OUTLINE_BUTTON, PRIMARY_BUTTON, QUIET_BUTTON } from '../workspaces/ui';

export const DANGER_BUTTON = cn(
  'h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border text-[12.5px] text-shodh-error hover:bg-shodh-raised disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
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

/** The connector's status, as a pill: a dot, the word, and the reason when there is one. */
export function StatusPill({ pill, className }: { pill: Pill; className?: string }) {
  return (
    <span className={cn('inline-flex items-center gap-1.5 h-6 max-w-full px-2 rounded-full text-[11.5px] font-medium', PILL_SURFACE[pill.tone], className)}>
      <span aria-hidden="true" className={cn('w-1.5 h-1.5 rounded-full shrink-0', TONE_DOT[pill.tone])} />
      <span className="truncate">
        {pill.label}
        {pill.reason && <span className="font-normal">{`: ${pill.reason}`}</span>}
      </span>
    </span>
  );
}

/** A short line with the long text one click away. */
export function LearnMore({ summary, children, label = 'Learn more' }: { summary?: React.ReactNode; children: React.ReactNode; label?: string }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  return (
    <div className="flex flex-col gap-1">
      <p className="m-0 text-[12.5px] text-shodh-text-muted">
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
      <div id={id} hidden={!open} className="text-[12.5px] leading-[1.55] text-shodh-text-secondary max-w-[68ch]">
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
          <h3 id={id} className="m-0 text-[14px] font-semibold text-shodh-text">{title}</h3>
          {description && <div className="m-0 mt-0.5 text-[12.5px] text-shodh-text-muted max-w-[68ch]">{description}</div>}
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
        'flex items-start gap-2.5 rounded-lg border px-3.5 py-3 text-[12.5px]',
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

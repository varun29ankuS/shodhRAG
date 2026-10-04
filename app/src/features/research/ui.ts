import { cn } from '../../lib/utils';

/** Shared class names of the research views (the app's tokens). */
export const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

export const BUTTON = cn(
  'h-8 px-2.5 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border text-[12.5px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-50 disabled:cursor-not-allowed aria-disabled:opacity-50 aria-disabled:cursor-not-allowed transition-colors duration-micro',
  FOCUS_RING,
);

export const PRIMARY_BUTTON = cn(
  'h-8 px-3 inline-flex items-center gap-1.5 rounded-lg bg-shodh-accent text-shodh-on-accent text-[12.5px] font-semibold hover:bg-shodh-accent-hover disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
  FOCUS_RING,
);

export const ICON_BUTTON = cn(
  'w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-40 disabled:cursor-not-allowed transition-colors duration-micro',
  FOCUS_RING,
);

export const INPUT = cn(
  'w-full h-8 px-2.5 rounded-lg border border-shodh-border bg-shodh-surface-2 text-[12.5px] text-shodh-text placeholder:text-shodh-text-faint',
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
);

export const SECTION_TITLE = 'text-[11px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint';

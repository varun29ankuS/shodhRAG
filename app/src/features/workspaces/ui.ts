import { cn } from '../../lib/utils';

export const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

export const PRIMARY_BUTTON = cn(
  'h-9 px-3.5 inline-flex items-center gap-2 rounded-lg bg-shodh-accent text-shodh-on-accent text-[13px] font-semibold hover:bg-shodh-accent-hover disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
  FOCUS_RING,
);

export const QUIET_BUTTON = cn(
  'h-8 px-2.5 inline-flex items-center gap-1.5 rounded-lg text-[12.5px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
  FOCUS_RING,
);

export const OUTLINE_BUTTON = cn(
  'h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border text-[12.5px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
  FOCUS_RING,
);

export const INPUT = cn(
  'w-full h-9 px-3 rounded-lg border border-shodh-border bg-shodh-surface text-[13px] text-shodh-text placeholder:text-shodh-text-faint',
  FOCUS_RING,
);

export const TEXTAREA = cn(
  'w-full px-3 py-2 rounded-lg border border-shodh-border bg-shodh-surface text-[13px] leading-[1.5] text-shodh-text placeholder:text-shodh-text-faint resize-y scrollbar-thin',
  FOCUS_RING,
);

export const LABEL = 'text-[12px] font-medium text-shodh-text-secondary';

export const SECTION_TITLE = 'm-0 text-[13px] font-bold text-shodh-text-secondary';

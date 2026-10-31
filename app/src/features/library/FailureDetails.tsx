import React from 'react';
import { cn } from '../../lib/utils';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

/**
 * What the indexer said about a failure, folded behind "Details": the plain
 * reason is shown, the raw one is there for whoever needs it.
 */
export function FailureDetails({ detail, className }: { detail: string; className?: string }) {
  if (!detail) return null;
  return (
    <details className={cn('group/detail min-w-0', className)}>
      <summary
        className={cn(
          'list-none [&::-webkit-details-marker]:hidden w-fit cursor-pointer select-none rounded text-caption text-shodh-text-muted hover:text-shodh-text underline-offset-2 hover:underline',
          FOCUS_RING,
        )}
      >
        <span className="group-open/detail:hidden">Details</span>
        <span className="hidden group-open/detail:inline">Hide details</span>
      </summary>
      <code className="mt-1 block whitespace-pre-wrap break-words rounded bg-shodh-raised-2 px-1.5 py-1 font-mono text-caption leading-snug text-shodh-text-faint">
        {detail}
      </code>
    </details>
  );
}

export default FailureDetails;

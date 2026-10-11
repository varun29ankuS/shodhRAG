import React from 'react';
import { Skeleton } from '../ui/skeleton';
import type { ViewTab } from '../../lib/viewTabs';

/**
 * Placeholder with the rough shape of a view while its code or data loads,
 * so switching never shows a blank pane or shifts the layout when it lands.
 */
export function ViewSkeleton({ view }: { view: ViewTab }) {
  return (
    <div className="h-full overflow-hidden" role="status" aria-label="Loading">
      {view === 'settings' ? (
        <div className="h-full flex">
          <div className="w-[220px] shrink-0 border-r border-shodh-border-subtle px-6 py-7 flex flex-col gap-3">
            <Skeleton className="h-5 w-24" />
            <Skeleton className="h-8 w-full" />
            <Skeleton className="h-8 w-full" />
            <Skeleton className="h-8 w-full" />
          </div>
          <div className="flex-1 px-10 py-8 flex flex-col gap-4 max-w-[820px]">
            <Skeleton className="h-7 w-40" />
            <Skeleton className="h-4 w-3/4" />
            <Skeleton className="h-48 w-full rounded-xl" />
          </div>
        </div>
      ) : view === 'tasks' ? (
        <div className="px-6 pt-4 flex flex-col gap-4">
          <Skeleton className="h-8 w-44 rounded-lg" />
          <Skeleton className="h-6 w-28" />
          {Array.from({ length: 6 }, (_, i) => (
            <Skeleton key={i} className="h-11 w-full rounded-lg" />
          ))}
        </div>
      ) : (
        <div className="max-w-5xl mx-auto px-8 py-7 flex flex-col gap-5">
          <Skeleton className="h-7 w-36" />
          <Skeleton className="h-4 w-80" />
          <div className="grid gap-3 grid-cols-[repeat(auto-fill,minmax(300px,1fr))]">
            {Array.from({ length: 3 }, (_, i) => (
              <Skeleton key={i} className="h-36 rounded-xl" />
            ))}
          </div>
        </div>
      )}
      <span className="sr-only">Loading…</span>
    </div>
  );
}

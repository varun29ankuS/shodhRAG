import React from 'react';
import { ChevronRight } from 'lucide-react';
import { cn } from '../../lib/utils';
import type { TechnicalRow } from './verification';

interface TechnicalDetailsProps {
  rows: TechnicalRow[];
  className?: string;
}

/**
 * "Technical details": the raw checksums (and similar diagnostics) behind a
 * closed disclosure, so the card's text says what verification means and the
 * values are there for whoever needs to compare them.
 */
export function TechnicalDetails({ rows, className }: TechnicalDetailsProps) {
  if (rows.length === 0) return null;
  return (
    <details className={cn('group text-[12px] text-shodh-text-muted', className)}>
      <summary
        className="inline-flex items-center gap-1 cursor-pointer select-none rounded-md list-none text-shodh-text-muted hover:text-shodh-text focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring [&::-webkit-details-marker]:hidden"
      >
        <ChevronRight className="w-3.5 h-3.5 transition-transform duration-micro group-open:rotate-90" aria-hidden="true" />
        Technical details
      </summary>
      <dl className="mt-1.5 flex flex-col gap-1">
        {rows.map(row => (
          <div key={`${row.label}:${row.value}`} className="flex flex-col gap-0.5">
            <dt className="text-shodh-text-secondary">{row.label}</dt>
            <dd className="m-0 font-mono break-all text-shodh-text-muted">{row.value}</dd>
          </div>
        ))}
      </dl>
    </details>
  );
}

export default TechnicalDetails;

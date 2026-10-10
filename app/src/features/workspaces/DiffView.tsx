import React from 'react';
import { cn } from '../../lib/utils';
import type { DiffLine } from './model';

/**
 * A line diff of two instruction versions: added lines marked “+”, removed lines “−” and
 * struck through, unchanged lines plain. Screen readers hear “Added” or “Removed” per
 * line; colour is never the only signal.
 */
export function DiffView({ lines, label, className }: { lines: DiffLine[]; label: string; className?: string }) {
  if (lines.length === 0) {
    return <p className={cn('text-[12.5px] text-shodh-text-muted', className)}>Both versions are empty.</p>;
  }
  const changed = lines.filter(l => l.op !== 'same').length;
  return (
    <div className={cn('rounded-lg border border-shodh-border-subtle bg-shodh-raised overflow-hidden', className)}>
      <p className="sr-only">{`${label}: ${changed} ${changed === 1 ? 'line changes' : 'lines change'}.`}</p>
      <ol aria-label={label} className="m-0 p-0 list-none font-mono text-[12px] leading-[1.55] max-h-[320px] overflow-y-auto scrollbar-thin">
        {lines.map((line, i) => (
          <li
            key={i}
            className={cn(
              'flex gap-2 px-2.5 whitespace-pre-wrap break-words',
              line.op === 'added' && 'bg-shodh-success-soft text-shodh-text',
              line.op === 'removed' && 'bg-shodh-error/10 text-shodh-text-muted line-through decoration-shodh-text-faint',
              line.op === 'same' && 'text-shodh-text-secondary',
            )}
          >
            <span aria-hidden="true" className="select-none w-3 shrink-0 text-shodh-text-faint">
              {line.op === 'added' ? '+' : line.op === 'removed' ? '−' : ' '}
            </span>
            {line.op !== 'same' && <span className="sr-only">{line.op === 'added' ? 'Added: ' : 'Removed: '}</span>}
            <span className="min-w-0">{line.text === '' ? ' ' : line.text}</span>
          </li>
        ))}
      </ol>
    </div>
  );
}

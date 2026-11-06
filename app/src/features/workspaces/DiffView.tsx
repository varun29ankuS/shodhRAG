import React from 'react';
import { cn } from '../../lib/utils';
import type { DiffLine } from './model';
import { CodeDiff } from '../code/CodeDiff';
import { opsAsChange } from '../code/diffModel';

/**
 * A line diff of two instruction versions, in the app's one diff view (`CodeDiff`):
 * added lines marked “+”, removed lines “−”, unchanged lines plain, each named for
 * screen readers; colour is never the only signal.
 */
export function DiffView({ lines, label, className }: { lines: DiffLine[]; label: string; className?: string }) {
  if (lines.length === 0) {
    return <p className={cn('text-small text-shodh-text-muted', className)}>Both versions are empty.</p>;
  }
  return (
    <CodeDiff
      changes={[opsAsChange('Instructions', lines)]}
      label={label}
      language="text"
      className={className}
      maxHeight={320}
    />
  );
}

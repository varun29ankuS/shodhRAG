import React, { useEffect, useRef } from 'react';
import { ShieldAlert } from 'lucide-react';
import { cn } from '../../lib/utils';
import type { StepApproval } from './reducer';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function humanKey(key: string): string {
  const spaced = key.replace(/_/g, ' ').replace(/([a-z])([A-Z])/g, '$1 $2');
  return spaced.charAt(0).toUpperCase() + spaced.slice(1).toLowerCase();
}

function displayValue(value: unknown): string {
  if (value === null || value === undefined || value === '') return '—';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

/** The preview as label/value rows; non-object previews become one row. */
function previewRows(preview: unknown): Array<[string, string]> {
  if (isRecord(preview)) {
    return Object.entries(preview)
      .filter(([, v]) => v !== null && v !== undefined && v !== '')
      .map(([k, v]) => [humanKey(k), displayValue(v)]);
  }
  if (preview === null || preview === undefined) return [];
  return [['Details', displayValue(preview)]];
}

function isTextEntry(element: Element | null): boolean {
  if (!element) return false;
  const tag = element.tagName;
  return tag === 'TEXTAREA' || tag === 'INPUT' || (element as HTMLElement).isContentEditable;
}

interface ApprovalPromptProps {
  approval: StepApproval;
  onDecide: (approved: boolean) => void;
  compact?: boolean;
}

/**
 * Inline approval for a write or destructive step. Deny with Esc (handled by
 * the hosting view, so it never also interrupts the run). For a write step
 * the Approve button takes focus (Enter approves) unless the user is typing;
 * a destructive step focuses Deny, so approving it is always deliberate.
 */
export function ApprovalPrompt({ approval, onDecide, compact = false }: ApprovalPromptProps) {
  const approveRef = useRef<HTMLButtonElement>(null);
  const denyRef = useRef<HTMLButtonElement>(null);
  const pending = approval.decision === 'pending';
  const destructive = approval.tier === 'destructive';
  const rows = previewRows(approval.preview);

  useEffect(() => {
    if (!pending) return;
    if (isTextEntry(document.activeElement)) return;
    (destructive ? denyRef : approveRef).current?.focus({ preventScroll: true });
  }, [pending, destructive]);

  if (!pending) {
    return (
      <p className={cn('text-[12px]', approval.decision === 'approved' ? 'text-shodh-success' : 'text-shodh-text-muted')}>
        {approval.decision === 'approved' ? 'You approved this.' : 'You declined this.'}
      </p>
    );
  }

  return (
    <div
      role="group"
      aria-label={`Approval needed: ${approval.label}`}
      className={cn(
        'ask-rise rounded-xl border bg-shodh-surface flex flex-col gap-2.5',
        destructive ? 'border-shodh-error/60' : 'border-shodh-warning/60',
        compact ? 'p-2.5' : 'p-3.5',
      )}
    >
      <div className="flex items-start gap-2">
        <ShieldAlert
          className={cn('w-4 h-4 mt-px shrink-0', destructive ? 'text-shodh-error' : 'text-shodh-warning')}
          aria-hidden="true"
        />
        <div className="min-w-0">
          <p className="text-[13px] font-semibold text-shodh-text break-words">{approval.label}</p>
          <p className="text-[11.5px] text-shodh-text-muted">
            {destructive ? 'This removes data. It needs your approval.' : 'This changes your data. It needs your approval.'}
          </p>
        </div>
      </div>
      {rows.length > 0 && (
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 rounded-lg bg-shodh-raised px-3 py-2 text-[12.5px]">
          {rows.map(([key, value]) => (
            <React.Fragment key={key}>
              <dt className="text-shodh-text-muted whitespace-nowrap">{key}</dt>
              <dd className="text-shodh-text break-words min-w-0">{value}</dd>
            </React.Fragment>
          ))}
        </dl>
      )}
      <div className="flex items-center gap-2">
        <button
          ref={approveRef}
          type="button"
          onClick={() => onDecide(true)}
          className={cn(
            'inline-flex items-center gap-2 h-8 px-3 rounded-lg text-[12.5px] font-semibold transition-colors duration-micro',
            destructive
              ? 'bg-shodh-error text-white hover:opacity-90'
              : 'bg-shodh-accent text-shodh-on-accent hover:bg-shodh-accent-hover',
            FOCUS_RING,
          )}
        >
          Approve
          {!destructive && <kbd className="font-mono text-[10.5px] opacity-80">Enter</kbd>}
        </button>
        <button
          ref={denyRef}
          type="button"
          onClick={() => onDecide(false)}
          className={cn(
            'inline-flex items-center gap-2 h-8 px-3 rounded-lg text-[12.5px] font-medium text-shodh-text-secondary bg-shodh-raised-2 hover:bg-shodh-pressed hover:text-shodh-text transition-colors duration-micro',
            FOCUS_RING,
          )}
        >
          Deny
          <kbd className="font-mono text-[10.5px] opacity-70">Esc</kbd>
        </button>
      </div>
    </div>
  );
}

export default ApprovalPrompt;

import { formatCost, formatTokens } from '../agent/format';
import type { AuditStats } from './types';

function Card({ label, value, detail }: { label: string; value: string; detail?: string }) {
  return (
    <div className="flex flex-col gap-1 rounded-xl border border-shodh-border-subtle bg-shodh-surface px-4 py-3 min-w-0">
      <dt className="text-[11.5px] text-shodh-text-muted">{label}</dt>
      <dd className="m-0 text-[20px] font-semibold tabular-nums text-shodh-text leading-tight">{value}</dd>
      {detail && <dd className="m-0 text-[11.5px] text-shodh-text-faint tabular-nums truncate">{detail}</dd>}
    </div>
  );
}

/** This month's usage, from the audit log (local calendar month). */
export function UsageCards({ stats }: { stats: AuditStats | null }) {
  const month = stats?.month;
  const placeholder = '—';
  const tokens = month ? month.cloudInputTokens + month.cloudOutputTokens : 0;
  return (
    <dl aria-label="Usage this month" className="grid grid-cols-2 lg:grid-cols-4 gap-3 m-0">
      <Card label="Questions this month" value={month ? month.questions.toLocaleString() : placeholder} />
      <Card label="Tool calls" value={month ? month.toolCalls.toLocaleString() : placeholder} />
      <Card label="Approvals" value={month ? month.approvals.toLocaleString() : placeholder} />
      <Card
        label="Cloud tokens & cost"
        value={month ? formatTokens(tokens) : placeholder}
        detail={
          month
            ? `${formatTokens(month.cloudInputTokens)} in · ${formatTokens(month.cloudOutputTokens)} out · ${
                formatCost(month.cloudCostUsd) ?? '$0.00'
              }`
            : undefined
        }
      />
    </dl>
  );
}

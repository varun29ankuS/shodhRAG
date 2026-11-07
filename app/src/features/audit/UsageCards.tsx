import { useEffect, useState } from 'react';
import { formatCost, formatTokens } from '../agent/format';
import { codeIntelApi, errorText } from '../codeIntel/api';
import type { TotalSavings } from '../codeIntel/api';
import { approxTokens, totalDetail } from '../codeIntel/format';
import type { AuditStats } from './types';

function Card({ label, value, detail, title }: { label: string; value: string; detail?: string; title?: string }) {
  return (
    <div className="flex flex-col gap-1 rounded-xl border border-shodh-border-subtle bg-shodh-surface px-4 py-3 min-w-0" title={title}>
      <dt className="text-caption text-shodh-text-muted">{label}</dt>
      <dd className="m-0 text-[20px] font-semibold tabular-nums text-shodh-text leading-tight">{value}</dd>
      {detail && <dd className="m-0 text-caption text-shodh-text-faint tabular-nums truncate">{detail}</dd>}
    </div>
  );
}

/** The running total of tokens Code mode's output shortening saved (every conversation, deleted ones too). */
function useTotalSavings(): TotalSavings | null {
  const [total, setTotal] = useState<TotalSavings | null>(null);
  useEffect(() => {
    let active = true;
    codeIntelApi
      .total()
      .then(next => {
        if (active) setTotal(next);
      })
      .catch(err => console.error('Tokens saved unavailable:', errorText(err)));
    return () => {
      active = false;
    };
  }, []);
  return total;
}

/** This month's usage, from the audit log (local calendar month), and the tokens Code mode saved in all. */
export function UsageCards({ stats }: { stats: AuditStats | null }) {
  const month = stats?.month;
  const saved = useTotalSavings();
  const placeholder = '—';
  const tokens = month ? month.cloudInputTokens + month.cloudOutputTokens : 0;
  return (
    <div className="flex flex-col gap-3">
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
      <dl aria-label="Code mode, in all" className="grid grid-cols-2 lg:grid-cols-4 gap-3 m-0">
        <Card
          label="Tokens saved in Code mode, in all"
          value={saved ? approxTokens(saved.savedTokens) : placeholder}
          detail={saved ? totalDetail(saved) : undefined}
          title={saved ? `Since the first Code mode command. ${saved.method}` : undefined}
        />
      </dl>
    </div>
  );
}

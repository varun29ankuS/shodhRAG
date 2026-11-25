/**
 * The one line that counts Shodh's model calls today (`model_usage_today`): Settings →
 * Model, and later the Automations view. Pure (type-only imports), so `node --test`
 * loads it directly (`app/tests/modelCalls.test.ts`).
 */

import { localTime } from '../tables/format.ts';
import type { TimeOptions } from '../tables/format.ts';

export type Spender = 'chat' | 'tables' | 'automations' | 'review' | 'slack' | 'telegram' | 'learning';

export interface SpenderCalls {
  spender: Spender;
  calls: number;
}

/** `ModelUsageToday` (camelCase). */
export interface ModelUsageToday {
  day: string;
  total: number;
  bySpender: SpenderCalls[];
  /** RFC 3339: the next 00:00 UTC. */
  resetsAt: string;
}

const SPENDERS: readonly Spender[] = ['chat', 'tables', 'automations', 'review', 'slack', 'telegram', 'learning'];

/** The reply of `model_usage_today`, or null when it is not one. */
export function readUsageToday(value: unknown): ModelUsageToday | null {
  if (typeof value !== 'object' || value === null) return null;
  const v = value as Record<string, unknown>;
  if (typeof v.day !== 'string' || typeof v.total !== 'number' || typeof v.resetsAt !== 'string' || !Array.isArray(v.bySpender)) return null;
  const bySpender: SpenderCalls[] = [];
  for (const row of v.bySpender) {
    if (typeof row !== 'object' || row === null) return null;
    const r = row as Record<string, unknown>;
    if (typeof r.spender !== 'string' || !(SPENDERS as readonly string[]).includes(r.spender) || typeof r.calls !== 'number') return null;
    bySpender.push({ spender: r.spender as Spender, calls: r.calls });
  }
  return { day: v.day, total: v.total, bySpender, resetsAt: v.resetsAt };
}

/** "Today: 23 model calls (chat 9, tables 8, learning 6). Resets 05:30." */
export function callsTodayLine(usage: ModelUsageToday, now: number, options: TimeOptions = {}): string {
  const resets = Date.parse(usage.resetsAt);
  const reset = Number.isNaN(resets) ? '' : ` Resets ${localTime(resets, now, options)}.`;
  if (usage.total === 0) return `Today: no model calls yet.${reset}`;
  const parts = usage.bySpender.filter((s) => s.calls > 0).map((s) => `${s.spender} ${s.calls}`);
  const noun = usage.total === 1 ? 'model call' : 'model calls';
  return `Today: ${usage.total} ${noun} (${parts.join(', ')}).${reset}`;
}

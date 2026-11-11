// Time helpers of the Activity log. The entries' sentences are in sentence.ts.

/** "Oct 3, 14:05:09" in local time. */
export function formatTimestamp(ts: string): string {
  const date = new Date(ts);
  if (Number.isNaN(date.getTime())) return ts;
  return date.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
}

/** Local `YYYY-MM-DD` (for date inputs and file names). */
export function localDate(date: Date): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, '0');
  const d = String(date.getDate()).padStart(2, '0');
  return `${y}-${m}-${d}`;
}

/** Start (00:00) or end (23:59:59.999) of a local `YYYY-MM-DD` as ISO. */
export function dayBoundary(value: string, edge: 'start' | 'end'): string | undefined {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!match) return undefined;
  const [, y, m, d] = match;
  const date =
    edge === 'start'
      ? new Date(Number(y), Number(m) - 1, Number(d), 0, 0, 0, 0)
      : new Date(Number(y), Number(m) - 1, Number(d), 23, 59, 59, 999);
  return Number.isNaN(date.getTime()) ? undefined : date.toISOString();
}

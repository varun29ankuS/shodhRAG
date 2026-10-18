/**
 * How Settings → Memory words today's learning use: why the learning model's answers, or
 * memories in them, were not used ("1 invalid: subject did not fit the class"). Pure and
 * import-free, so Node's strip-types runner tests it (`app/tests/learnUsage.test.ts`).
 */

/** Today's rejections for one reason, as `memory_learn_status` sends them. */
export interface RejectionCount {
  code: string;
  /** The reason in plain words. */
  reason: string;
  count: number;
}

/** Candidates refused for not quoting the user are counted as "refused", not invalid. */
const REFUSED_CODE = 'ungrounded';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Validate the status's `rejections`; malformed entries are dropped, a missing list is empty. */
export function parseRejections(value: unknown): RejectionCount[] {
  if (!Array.isArray(value)) return [];
  const out: RejectionCount[] = [];
  for (const entry of value) {
    if (!isRecord(entry)) continue;
    const { code, reason, count } = entry;
    if (typeof code !== 'string' || typeof reason !== 'string') continue;
    if (typeof count !== 'number' || !Number.isInteger(count) || count <= 0) continue;
    out.push({ code, reason, count });
  }
  return out;
}

/**
 * "1 invalid: subject did not fit the class", "3 invalid: a value of the wrong type (2);
 * the answer was cut off at the length limit (1)". Empty when nothing was invalid; the
 * bare count when no reason was kept (a day before reasons were kept).
 */
export function invalidSummary(invalid: number, rejections: readonly RejectionCount[]): string {
  if (!Number.isFinite(invalid) || invalid <= 0) return '';
  const reasons = rejections.filter(r => r.code !== REFUSED_CODE && r.count > 0);
  if (reasons.length === 0) return `${invalid} invalid dropped`;
  if (reasons.length === 1) return `${invalid} invalid: ${reasons[0].reason}`;
  return `${invalid} invalid: ${reasons.map(r => `${r.reason} (${r.count})`).join('; ')}`;
}

/**
 * Month grid for the Tasks calendar view. Pure module (imports only other
 * pure modules) so it is unit-tested directly with Node
 * (`app/tests/calendarGrid.test.ts`).
 */
import { storedDayKey } from './dueDate.ts';

/** Local calendar day key, e.g. "2026-10-03". */
export function dayKey(date: Date): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, '0');
  const d = String(date.getDate()).padStart(2, '0');
  return `${y}-${m}-${d}`;
}

/**
 * Local day key of a stored date/time, or null when it does not parse.
 * Date-only values (`2026-10-03`) are local days, not UTC midnight.
 */
export function isoDayKey(iso: string | null | undefined): string | null {
  return storedDayKey(iso);
}

export interface GridDay {
  date: Date;
  key: string;
  inMonth: boolean;
}

/**
 * Whole weeks covering `month` of `year` (month 0–11), starting on
 * `weekStart` (0 = Sunday). Always 6 rows so the grid height never jumps
 * between months.
 */
export function monthGrid(year: number, month: number, weekStart = 0): GridDay[][] {
  const first = new Date(year, month, 1);
  const lead = (first.getDay() - weekStart + 7) % 7;
  const weeks: GridDay[][] = [];
  for (let w = 0; w < 6; w++) {
    const week: GridDay[] = [];
    for (let d = 0; d < 7; d++) {
      // Constructing from parts keeps every cell at local midnight across DST.
      const date = new Date(year, month, 1 - lead + w * 7 + d);
      week.push({ date, key: dayKey(date), inMonth: date.getMonth() === month });
    }
    weeks.push(week);
  }
  return weeks;
}

/** Group items by the local day of the timestamp `at` returns; undated items are skipped. */
export function groupByDay<T>(items: readonly T[], at: (item: T) => string | null | undefined): Map<string, T[]> {
  const map = new Map<string, T[]>();
  for (const item of items) {
    const key = isoDayKey(at(item));
    if (!key) continue;
    const list = map.get(key);
    if (list) list.push(item);
    else map.set(key, [item]);
  }
  return map;
}

/** `date` moved by `days` calendar days (DST-safe). */
export function addDays(date: Date, days: number): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate() + days);
}

/**
 * The counter of today's model calls (Settings → Model). Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/modelCalls.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { callsTodayLine, readUsageToday } from '../src/features/modelPicker/modelCalls.ts';

const INDIA = { locale: 'en-GB', timeZone: 'Asia/Kolkata' };
const EVENING = Date.UTC(2026, 10, 25, 14, 0);

test('the counter names each spender in order and the reset in local time', () => {
  const usage = readUsageToday({
    day: '2026-11-25',
    total: 23,
    bySpender: [
      { spender: 'chat', calls: 9 },
      { spender: 'tables', calls: 8 },
      { spender: 'learning', calls: 6 },
    ],
    resetsAt: '2026-11-26T00:00:00+00:00',
  });
  assert.ok(usage);
  assert.match(callsTodayLine(usage, EVENING, INDIA), /^Today: 23 model calls \(chat 9, tables 8, learning 6\)\. Resets 0?5:30 tomorrow\.$/);
});

test('one call and no calls read plainly', () => {
  const one = { day: '2026-11-25', total: 1, bySpender: [{ spender: 'chat' as const, calls: 1 }], resetsAt: '2026-11-26T00:00:00Z' };
  assert.match(callsTodayLine(one, EVENING, INDIA), /^Today: 1 model call \(chat 1\)\. Resets/);
  const none = { day: '2026-11-25', total: 0, bySpender: [], resetsAt: '2026-11-26T00:00:00Z' };
  assert.match(callsTodayLine(none, EVENING, INDIA), /^Today: no model calls yet\. Resets 0?5:30 tomorrow\.$/);
});

test('a reply that is not a usage report is refused', () => {
  assert.equal(readUsageToday(null), null);
  assert.equal(readUsageToday({ day: 'd', total: 1, bySpender: [{ spender: 'cron', calls: 1 }], resetsAt: 'x' }), null);
  assert.equal(readUsageToday({ day: 'd', total: '1', bySpender: [], resetsAt: 'x' }), null);
});

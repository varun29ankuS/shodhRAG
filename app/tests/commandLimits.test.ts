/**
 * Code-mode command limit tests. Run with Node 22.6+ (no extra dependencies):
 *   node --experimental-strip-types --test app/tests/commandLimits.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { COMMAND_LIMITS, commandLimitHint, parseCommandLimit } from '../src/features/workspaces/commandLimits.ts';
import type { CommandLimitKey, CommandLimitSpec } from '../src/features/workspaces/commandLimits.ts';

function spec(key: CommandLimitKey): CommandLimitSpec {
  const found = COMMAND_LIMITS.find(s => s.key === key);
  assert.ok(found);
  return found;
}

test('the limits have the backend ranges and defaults', () => {
  assert.deepEqual(
    COMMAND_LIMITS.map(s => [s.key, s.min, s.max, s.defaultValue]),
    [
      ['commandTimeoutSecs', 10, 3600, 600],
      ['memoryLimitMb', 256, 65536, 4096],
      ['cpuPercent', 10, 100, 80],
    ],
  );
});

test('whole numbers inside the range are accepted, ends included', () => {
  const time = spec('commandTimeoutSecs');
  assert.deepEqual(parseCommandLimit(time, '10'), { ok: true, value: 10 });
  assert.deepEqual(parseCommandLimit(time, ' 3600 '), { ok: true, value: 3600 });
  assert.deepEqual(parseCommandLimit(spec('memoryLimitMb'), '65536'), { ok: true, value: 65536 });
  assert.deepEqual(parseCommandLimit(spec('cpuPercent'), '100'), { ok: true, value: 100 });
});

test('values outside the range or not whole numbers are refused with the range', () => {
  const time = spec('commandTimeoutSecs');
  for (const text of ['9', '3601', '', '  ', '1.5', '-20', '1e3', 'ten']) {
    const parsed = parseCommandLimit(time, text);
    assert.equal(parsed.ok, false, text);
    if (!parsed.ok) assert.match(parsed.error, /10 to 3,600 seconds/);
  }
  const cpu = parseCommandLimit(spec('cpuPercent'), '101');
  assert.equal(cpu.ok, false);
  if (!cpu.ok) assert.equal(cpu.error, 'Must be from 10 to 100 percent.');
  assert.equal(parseCommandLimit(spec('memoryLimitMb'), '255').ok, false);
});

test('hints put seconds in minutes and MB in GB', () => {
  assert.equal(commandLimitHint('commandTimeoutSecs', 30), null);
  assert.equal(commandLimitHint('commandTimeoutSecs', 600), '10 min');
  assert.equal(commandLimitHint('commandTimeoutSecs', 90), '1.5 min');
  assert.equal(commandLimitHint('commandTimeoutSecs', 3600), '1 hour');
  assert.equal(commandLimitHint('memoryLimitMb', 512), null);
  assert.equal(commandLimitHint('memoryLimitMb', 4096), '4 GB');
  assert.equal(commandLimitHint('memoryLimitMb', 1536), '1.5 GB');
  assert.equal(commandLimitHint('cpuPercent', 80), null);
});

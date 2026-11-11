/**
 * The task list's due-date button stays inside its column: it is never wider than
 * the column, its date is cut with an ellipsis, and the whole date is on hover.
 * (A long date such as "Overdue since 12 Oct 2026 · overdue" pushed the priority
 * column off the row.) Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/dueMenu.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../src/features/tasks/QuickMenus.tsx', import.meta.url), 'utf8');

/** The JSX of DueMenu's trigger button, from `<button` to `</button>`. */
function dueTrigger(): string {
  const start = source.indexOf('export function DueMenu(');
  assert.ok(start >= 0, 'DueMenu is defined');
  const open = source.indexOf('<button', start);
  const close = source.indexOf('</button>', open);
  assert.ok(open > start && close > open, 'DueMenu has a trigger button');
  return source.slice(open, close);
}

test('the due-date button is bounded by its column and cuts its date', () => {
  const trigger = dueTrigger();
  assert.match(trigger, /className=\{cn\(\s*(?:\/\/[^\n]*\n\s*)*'[^']*\bmax-w-full\b[^']*\bmin-w-0\b/);
  assert.match(trigger, /<span className="truncate">/);
  assert.match(trigger, /<Clock className="[^"]*\bshrink-0\b/);
});

test('the whole due date is shown on hover', () => {
  assert.match(dueTrigger(), /title=\{`\$\{label\}/);
});

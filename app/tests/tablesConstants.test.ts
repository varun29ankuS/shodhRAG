/**
 * The limits the Tables tab mirrors from Rust, checked against the Rust sources, so a
 * change there (a prompt budget that changes MAX_COLUMNS, a new option limit, a longer
 * lease) fails here instead of desyncing the interface. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/tablesConstants.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  MAX_COLUMNS,
  MAX_EDIT_CHARS,
  MAX_INSTRUCTIONS_CHARS,
  MAX_NAME_CHARS,
  MAX_OPTION_CHARS,
  MAX_OPTIONS,
  MAX_QUESTION_CHARS,
  MAX_ROWS_PER_ADD,
  MAX_TEXT_CHARS,
  MIN_OPTIONS,
} from '../src/features/tables/model.ts';
import { LEAST_PAPER_CALLS, LEASE_MS, QUIET_RUN_MS } from '../src/features/tables/format.ts';

const source = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8');

const TABLES = '../../crates/shodh-rag/src/research/tables/';
const SOURCES = [
  `${TABLES}mod.rs`,
  `${TABLES}prompt.rs`,
  `${TABLES}budget.rs`,
  `${TABLES}jobs.rs`,
  '../src-tauri/src/table_commands.rs',
].map(source);

/** Every `const NAME: <int type> = <expr>;` of the sources, by name (module paths dropped). */
function constants(): Map<string, string> {
  const found = new Map<string, string>();
  for (const text of SOURCES) {
    for (const m of text.matchAll(/^\s*(?:pub(?:\([a-z]+\))? )?const ([A-Z_][A-Z0-9_]*): (?:usize|i64|u64|u32|i32) =\s*([^;]+);/gm)) {
      found.set(m[1], m[2].replace(/\s+/g, ' ').trim());
    }
  }
  return found;
}

const DEFINED = constants();

/** A Rust integer constant's value: literals, other constants and + - * / (integer division) with parentheses. */
function rustConst(name: string, depth = 0): number {
  assert.ok(depth < 10, `${name} resolves`);
  const expr = DEFINED.get(name);
  assert.ok(expr !== undefined, `the Rust sources define ${name}`);
  const tokens = expr.replace(/\b[a-z_]+::/g, '').match(/\d[\d_]*|[A-Z_][A-Z0-9_]*|[-+*/()]/g) ?? [];
  let at = 0;
  const atom = (): number => {
    const token = tokens[at++];
    if (token === '(') {
      const value = sum();
      assert.equal(tokens[at++], ')');
      return value;
    }
    if (/^\d/.test(token)) return Number(token.replace(/_/g, ''));
    return rustConst(token, depth + 1);
  };
  const product = (): number => {
    let value = atom();
    while (tokens[at] === '*' || tokens[at] === '/') {
      const op = tokens[at++];
      const right = atom();
      value = op === '*' ? value * right : Math.floor(value / right);
    }
    return value;
  };
  const sum = (): number => {
    let value = product();
    while (tokens[at] === '+' || tokens[at] === '-') {
      const op = tokens[at++];
      const right = product();
      value = op === '+' ? value + right : value - right;
    }
    return value;
  };
  const value = sum();
  assert.equal(at, tokens.length, `${name} = ${expr} is read whole`);
  return value;
}

test('the reader evaluates derived constants', () => {
  // MAX_COLUMNS is derived: MAX_COLUMNS_PER_CALL (a prompt budget) × MAX_CALLS_PER_PAPER.
  assert.equal(rustConst('MAX_COLUMNS'), rustConst('MAX_COLUMNS_PER_CALL') * rustConst('MAX_CALLS_PER_PAPER'));
  assert.ok(rustConst('MAX_COLUMNS_PER_CALL') > 0);
});

test('the column, option and text limits match research::tables', () => {
  assert.equal(MAX_COLUMNS, rustConst('MAX_COLUMNS'));
  assert.equal(MIN_OPTIONS, rustConst('MIN_OPTIONS'));
  assert.equal(MAX_OPTIONS, rustConst('MAX_OPTIONS'));
  assert.equal(MAX_TEXT_CHARS, rustConst('MAX_TEXT_CHARS'));
  assert.equal(MAX_QUESTION_CHARS, rustConst('MAX_QUESTION_CHARS'));
  assert.equal(MAX_INSTRUCTIONS_CHARS, rustConst('MAX_INSTRUCTIONS_CHARS'));
  assert.equal(MAX_OPTION_CHARS, rustConst('MAX_OPTION_CHARS'));
  assert.equal(MAX_NAME_CHARS, rustConst('MAX_NAME_CHARS'));
});

test('the command limits match table_commands.rs', () => {
  assert.equal(MAX_EDIT_CHARS, rustConst('MAX_EDIT_CHARS'));
  assert.equal(MAX_ROWS_PER_ADD, rustConst('MAX_ROWS_PER_ADD'));
  assert.equal(LEAST_PAPER_CALLS, rustConst('LEAST_PAPER_CALLS'));
});

test('the lease and the quiet-run threshold follow the runner and the HTTP timeout', () => {
  assert.equal(LEASE_MS, rustConst('LEASE_MINUTES') * 60_000);
  // A live run beats before every model call; one call times out after the client's timeout.
  const client = source('../../crates/shodh-rag/src/llm/simple_external.rs');
  const timeout = /\.timeout\(std::time::Duration::from_secs\((\d+)\)\)/.exec(client);
  assert.ok(timeout, 'the external client sets a request timeout');
  const timeoutMs = Number(timeout[1]) * 1000;
  assert.ok(QUIET_RUN_MS > timeoutMs, 'a run inside one slow call is never called not responding');
  assert.ok(QUIET_RUN_MS < LEASE_MS, 'not responding is said before the lease runs out');
});

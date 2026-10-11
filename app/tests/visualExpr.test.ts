/**
 * Safe expression language for plots and simulations.
 *   node --experimental-strip-types --test app/tests/visualExpr.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  GRAVITY,
  MAX_EXPR_CHARS,
  compileExpression,
  evaluate,
  freeVariables,
  isValidName,
  parseExpression,
  tryCompile,
} from '../src/features/ask/visual/expr.ts';

const close = (a: number, b: number, eps = 1e-12) => assert.ok(Math.abs(a - b) <= eps, `${a} != ${b}`);

test('expr: precedence and associativity', () => {
  assert.equal(evaluate('1 + 2 * 3'), 7);
  assert.equal(evaluate('(1 + 2) * 3'), 9);
  assert.equal(evaluate('10 - 4 - 3'), 3);
  assert.equal(evaluate('8 / 4 / 2'), 1);
  assert.equal(evaluate('2 ^ 3 ^ 2'), 512);
  assert.equal(evaluate('2 ** 3'), 8);
  assert.equal(evaluate('2 * 3 ^ 2'), 18);
});

test('expr: unary minus binds below power', () => {
  assert.equal(evaluate('-2 ^ 2'), -4);
  assert.equal(evaluate('(-2) ^ 2'), 4);
  assert.equal(evaluate('2 ^ -1'), 0.5);
  assert.equal(evaluate('3 * -2'), -6);
  assert.equal(evaluate('--3'), 3);
  assert.equal(evaluate('-x * y', { x: 2, y: 5 }), -10);
  assert.equal(evaluate('+4'), 4);
});

test('expr: numbers', () => {
  assert.equal(evaluate('.5 + 1.'), 1.5);
  assert.equal(evaluate('1e3 + 2.5E-1'), 1000.25);
});

test('expr: functions and constants', () => {
  close(evaluate('sin(pi / 2)'), 1);
  close(evaluate('atan2(1, 1)'), Math.PI / 4);
  close(evaluate('ln(e)'), 1);
  close(evaluate('log(1000)'), 3);
  assert.equal(evaluate('min(3, 1, 2) + max(4, 9)'), 10);
  assert.equal(evaluate('floor(2.7) + ceil(2.1) + abs(-1)'), 6);
  assert.equal(evaluate('sqrt(16) + exp(0)'), 5);
  assert.equal(evaluate('mod(-1, 3)'), 2);
  assert.equal(evaluate('g'), GRAVITY);
});

test('expr: comparisons give 1 or 0 and do not chain', () => {
  assert.equal(evaluate('y < 0', { y: -1 }), 1);
  assert.equal(evaluate('y >= 0', { y: -1 }), 0);
  assert.equal(evaluate('1 + 1 <= 2'), 1);
  assert.throws(() => parseExpression('1 < 2 < 3'), /chained/);
});

test('expr: named parameters, and a variable named g overrides the constant', () => {
  const fn = compileExpression('v0 * t - 0.5 * g * t ^ 2', ['v0', 't']);
  close(fn([20, 2]), 40 - 2 * GRAVITY);
  const moon = compileExpression('g', ['g']);
  assert.equal(moon([1.62]), 1.62);
});

test('expr: errors are readable', () => {
  assert.throws(() => evaluate('1 +'), /ends too early/);
  assert.throws(() => evaluate('(1 + 2'), /Missing "\)"/);
  assert.throws(() => evaluate('1 + 2)'), /Unexpected "\)"/);
  assert.throws(() => evaluate('2x'), /Write "\*"/);
  assert.throws(() => evaluate('sin(1, 2)'), /takes 1 argument/);
  assert.throws(() => evaluate('foo(1)'), /Unknown function "foo"/);
  assert.throws(() => evaluate('sin'), /is a function/);
  assert.throws(() => evaluate('1 ; 2'), /Unexpected character/);
  assert.throws(() => evaluate(''), /empty/);
  assert.throws(() => evaluate('1'.repeat(MAX_EXPR_CHARS + 1)), /longer than/);
  assert.throws(() => evaluate(`${'('.repeat(200)}1${')'.repeat(200)}`), /nested too deeply/);
});

test('expr: no access to globals or object properties', () => {
  for (const name of ['window', 'globalThis', 'process', 'constructor', '__proto__', 'Math', 'eval', 'Function', 'toString']) {
    const r = tryCompile(name, []);
    assert.equal(r.ok, false, `${name} must be unknown`);
  }
  assert.equal(tryCompile('constructor(1)', []).ok, false);
  // No member access, strings or brackets exist in the language.
  for (const src of ['a.b', 'x["y"]', "'s'", '`t`', 'a = 1', 'x => x', '[1]', '{}']) {
    assert.equal(tryCompile(src, ['a', 'x']).ok, false, src);
  }
});

test('expr: rejects identifiers that are not allowed', () => {
  assert.equal(tryCompile('y + 1', ['x']).ok, false);
  assert.throws(() => compileExpression('1', ['sin']), /cannot be used/);
  assert.throws(() => compileExpression('1', ['pi']), /cannot be used/);
  assert.ok(isValidName('v0') && isValidName('theta_1') && isValidName('constructor'));
  assert.ok(!isValidName('__proto__') && !isValidName('1a') && !isValidName('e') && !isValidName('max'));
  // A variable called constructor reads its slot, not Object.prototype.constructor.
  assert.equal(compileExpression('constructor * 2', ['constructor'])([4]), 8);
});

test('expr: numbers given directly compile', () => {
  const r = tryCompile(2.5, []);
  assert.ok(r.ok);
  if (r.ok) assert.equal(r.fn([]), 2.5);
  assert.equal(tryCompile(null, []).ok, false);
  assert.equal(tryCompile({}, []).ok, false);
});

test('expr: free variables', () => {
  assert.deepEqual(freeVariables('a*sin(b) + pi + g').sort(), ['a', 'b']);
});

test('expr: undefined math yields NaN or Infinity, not an exception', () => {
  assert.ok(Number.isNaN(evaluate('sqrt(-1)')));
  assert.equal(evaluate('1/0'), Infinity);
});

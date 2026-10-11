/**
 * A small, safe math expression language for model-written plots and
 * simulations. Source text is tokenized and parsed (Pratt parser) into a
 * tree, then compiled to closures over a fixed list of variable slots.
 * Nothing is ever evaluated as JavaScript: no `eval`, no `Function`, no
 * property access, so an expression can reach nothing but the numbers it is
 * given and the math functions listed here.
 *
 * Grammar (lowest precedence first):
 *   comparison  a < b, a <= b, a > b, a >= b   (1 when true, else 0; not chained)
 *   additive    a + b, a - b
 *   product     a * b, a / b
 *   unary       -a, +a                         (-x^2 is -(x^2))
 *   power       a ^ b                          (right associative)
 *   primary     number, name, name(args), (expr)
 *
 * Pure module, unit-tested with Node (`app/tests/visualExpr.test.ts`).
 */

/** Longest expression accepted, in characters. */
export const MAX_EXPR_CHARS = 600;
/** Deepest nesting accepted (parentheses, operators, calls). */
export const MAX_EXPR_DEPTH = 48;
/** Names a model may choose for parameters and state. */
export const NAME_PATTERN = /^[A-Za-z][A-Za-z0-9_]{0,23}$/;

/** Standard gravity used by the constant `g` (m/s²) unless a variable named g is given. */
export const GRAVITY = 9.81;

const CONSTANTS: ReadonlyMap<string, number> = new Map([
  ['pi', Math.PI],
  ['e', Math.E],
  ['g', GRAVITY],
]);

interface FunctionDef {
  min: number;
  max: number;
  fn: (args: number[]) => number;
}

const FUNCTIONS: ReadonlyMap<string, FunctionDef> = new Map<string, FunctionDef>([
  ['sin', { min: 1, max: 1, fn: a => Math.sin(a[0]) }],
  ['cos', { min: 1, max: 1, fn: a => Math.cos(a[0]) }],
  ['tan', { min: 1, max: 1, fn: a => Math.tan(a[0]) }],
  ['asin', { min: 1, max: 1, fn: a => Math.asin(a[0]) }],
  ['acos', { min: 1, max: 1, fn: a => Math.acos(a[0]) }],
  ['atan', { min: 1, max: 1, fn: a => Math.atan(a[0]) }],
  ['atan2', { min: 2, max: 2, fn: a => Math.atan2(a[0], a[1]) }],
  ['sinh', { min: 1, max: 1, fn: a => Math.sinh(a[0]) }],
  ['cosh', { min: 1, max: 1, fn: a => Math.cosh(a[0]) }],
  ['tanh', { min: 1, max: 1, fn: a => Math.tanh(a[0]) }],
  ['sqrt', { min: 1, max: 1, fn: a => Math.sqrt(a[0]) }],
  ['abs', { min: 1, max: 1, fn: a => Math.abs(a[0]) }],
  ['sign', { min: 1, max: 1, fn: a => Math.sign(a[0]) }],
  ['exp', { min: 1, max: 1, fn: a => Math.exp(a[0]) }],
  ['ln', { min: 1, max: 1, fn: a => Math.log(a[0]) }],
  ['log', { min: 1, max: 1, fn: a => Math.log10(a[0]) }],
  ['min', { min: 1, max: 16, fn: a => Math.min(...a) }],
  ['max', { min: 1, max: 16, fn: a => Math.max(...a) }],
  ['hypot', { min: 1, max: 16, fn: a => Math.hypot(...a) }],
  ['floor', { min: 1, max: 1, fn: a => Math.floor(a[0]) }],
  ['ceil', { min: 1, max: 1, fn: a => Math.ceil(a[0]) }],
  ['round', { min: 1, max: 1, fn: a => Math.round(a[0]) }],
  ['mod', { min: 2, max: 2, fn: a => a[0] - a[1] * Math.floor(a[0] / a[1]) }],
]);

/** Names a model may not use for its own variables. */
export function isReservedName(name: string): boolean {
  return FUNCTIONS.has(name) || name === 'pi' || name === 'e';
}

/** A usable variable name: well formed and not a function or fixed constant. */
export function isValidName(name: string): boolean {
  return NAME_PATTERN.test(name) && !isReservedName(name);
}

export const FUNCTION_NAMES: readonly string[] = Array.from(FUNCTIONS.keys());

type TokenKind = 'num' | 'name' | 'op' | 'lparen' | 'rparen' | 'comma' | 'end';

interface Token {
  kind: TokenKind;
  text: string;
  value: number;
  pos: number;
}

export class ExprError extends Error {
  readonly pos: number;
  constructor(message: string, pos: number) {
    super(message);
    this.name = 'ExprError';
    this.pos = pos;
  }
}

function tokenize(src: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === ' ' || c === '\t' || c === '\n' || c === '\r') {
      i++;
      continue;
    }
    if ((c >= '0' && c <= '9') || (c === '.' && src[i + 1] >= '0' && src[i + 1] <= '9')) {
      const m = /^(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?/.exec(src.slice(i));
      if (!m) throw new ExprError(`Unreadable number at position ${i + 1}.`, i);
      const value = Number(m[0]);
      if (!Number.isFinite(value)) throw new ExprError(`Number out of range at position ${i + 1}.`, i);
      tokens.push({ kind: 'num', text: m[0], value, pos: i });
      i += m[0].length;
      // "2x" is ambiguous in a plain-text language: ask for an explicit "*".
      if (/[A-Za-z_(]/.test(src[i] ?? '')) throw new ExprError(`Write "*" between ${m[0]} and what follows (position ${i + 1}).`, i);
      continue;
    }
    if (/[A-Za-z_]/.test(c)) {
      const m = /^[A-Za-z_][A-Za-z0-9_]*/.exec(src.slice(i));
      const text = m ? m[0] : c;
      tokens.push({ kind: 'name', text, value: 0, pos: i });
      i += text.length;
      continue;
    }
    if (c === '<' || c === '>') {
      const text = src[i + 1] === '=' ? `${c}=` : c;
      tokens.push({ kind: 'op', text, value: 0, pos: i });
      i += text.length;
      continue;
    }
    if (c === '*' && src[i + 1] === '*') {
      tokens.push({ kind: 'op', text: '^', value: 0, pos: i });
      i += 2;
      continue;
    }
    if ('+-*/^'.includes(c)) {
      tokens.push({ kind: 'op', text: c, value: 0, pos: i });
      i++;
      continue;
    }
    if (c === '(') {
      tokens.push({ kind: 'lparen', text: c, value: 0, pos: i });
      i++;
      continue;
    }
    if (c === ')') {
      tokens.push({ kind: 'rparen', text: c, value: 0, pos: i });
      i++;
      continue;
    }
    if (c === ',') {
      tokens.push({ kind: 'comma', text: c, value: 0, pos: i });
      i++;
      continue;
    }
    throw new ExprError(`Unexpected character "${c}" at position ${i + 1}.`, i);
  }
  tokens.push({ kind: 'end', text: '', value: 0, pos: src.length });
  return tokens;
}

/** Parsed expression tree. */
export type Expr =
  | { type: 'num'; value: number }
  | { type: 'var'; name: string; pos: number }
  | { type: 'neg'; arg: Expr }
  | { type: 'bin'; op: string; left: Expr; right: Expr }
  | { type: 'call'; name: string; args: Expr[]; pos: number };

const BINARY_BP: Record<string, number> = {
  '<': 10, '<=': 10, '>': 10, '>=': 10,
  '+': 20, '-': 20,
  '*': 30, '/': 30,
  '^': 50,
};
const UNARY_BP = 40;

class Parser {
  private tokens: Token[];
  private index = 0;
  private depth = 0;

  constructor(tokens: Token[]) {
    this.tokens = tokens;
  }

  private peek(): Token {
    return this.tokens[this.index];
  }

  private next(): Token {
    return this.tokens[this.index++];
  }

  private enter(pos: number) {
    this.depth++;
    if (this.depth > MAX_EXPR_DEPTH) throw new ExprError('The expression is nested too deeply.', pos);
  }

  parseAll(): Expr {
    const expr = this.parse(0);
    const tail = this.peek();
    if (tail.kind !== 'end') throw new ExprError(`Unexpected "${tail.text}" at position ${tail.pos + 1}.`, tail.pos);
    return expr;
  }

  parse(minBp: number): Expr {
    const start = this.peek();
    this.enter(start.pos);
    let left = this.prefix();
    for (;;) {
      const tok = this.peek();
      if (tok.kind !== 'op') break;
      const bp = BINARY_BP[tok.text];
      if (bp === undefined || bp <= minBp) break;
      this.next();
      if (bp === BINARY_BP['<']) {
        const right = this.parse(bp);
        const after = this.peek();
        if (after.kind === 'op' && BINARY_BP[after.text] === bp) {
          throw new ExprError('Comparisons cannot be chained.', after.pos);
        }
        left = { type: 'bin', op: tok.text, left, right };
        continue;
      }
      // ^ is right associative: parse the right side at a lower binding power.
      const right = this.parse(tok.text === '^' ? bp - 1 : bp);
      left = { type: 'bin', op: tok.text, left, right };
    }
    this.depth--;
    return left;
  }

  private prefix(): Expr {
    const tok = this.next();
    switch (tok.kind) {
      case 'num':
        return { type: 'num', value: tok.value };
      case 'op':
        if (tok.text === '-' || tok.text === '+') {
          const arg = this.parse(UNARY_BP);
          return tok.text === '-' ? { type: 'neg', arg } : arg;
        }
        throw new ExprError(`Unexpected "${tok.text}" at position ${tok.pos + 1}.`, tok.pos);
      case 'lparen': {
        const inner = this.parse(0);
        const close = this.next();
        if (close.kind !== 'rparen') throw new ExprError(`Missing ")" for "(" at position ${tok.pos + 1}.`, tok.pos);
        return inner;
      }
      case 'name': {
        if (this.peek().kind === 'lparen') {
          this.next();
          const args: Expr[] = [];
          if (this.peek().kind !== 'rparen') {
            for (;;) {
              args.push(this.parse(0));
              const sep = this.peek();
              if (sep.kind === 'comma') {
                this.next();
                continue;
              }
              break;
            }
          }
          const close = this.next();
          if (close.kind !== 'rparen') throw new ExprError(`Missing ")" after the arguments of ${tok.text} (position ${tok.pos + 1}).`, tok.pos);
          return { type: 'call', name: tok.text, args, pos: tok.pos };
        }
        return { type: 'var', name: tok.text, pos: tok.pos };
      }
      case 'end':
        throw new ExprError('The expression ends too early.', tok.pos);
      default:
        throw new ExprError(`Unexpected "${tok.text}" at position ${tok.pos + 1}.`, tok.pos);
    }
  }
}

/** Parse source text into a tree. Throws `ExprError` with a readable message. */
export function parseExpression(source: string): Expr {
  if (typeof source !== 'string') throw new ExprError('An expression must be text.', 0);
  if (source.length > MAX_EXPR_CHARS) throw new ExprError(`The expression is longer than ${MAX_EXPR_CHARS} characters.`, MAX_EXPR_CHARS);
  if (!source.trim()) throw new ExprError('The expression is empty.', 0);
  return new Parser(tokenize(source)).parseAll();
}

/** A compiled expression: reads its variables from `slots` in the order given to `compile`. */
export type Compiled = (slots: ArrayLike<number>) => number;

function compileNode(node: Expr, slotOf: ReadonlyMap<string, number>): Compiled {
  switch (node.type) {
    case 'num': {
      const v = node.value;
      return () => v;
    }
    case 'var': {
      const slot = slotOf.get(node.name);
      if (slot !== undefined) return s => s[slot];
      const constant = CONSTANTS.get(node.name);
      if (constant !== undefined) return () => constant;
      if (FUNCTIONS.has(node.name)) throw new ExprError(`${node.name} is a function; call it as ${node.name}(…).`, node.pos);
      throw new ExprError(`Unknown name "${node.name}".`, node.pos);
    }
    case 'neg': {
      const a = compileNode(node.arg, slotOf);
      return s => -a(s);
    }
    case 'bin': {
      const l = compileNode(node.left, slotOf);
      const r = compileNode(node.right, slotOf);
      switch (node.op) {
        case '+': return s => l(s) + r(s);
        case '-': return s => l(s) - r(s);
        case '*': return s => l(s) * r(s);
        case '/': return s => l(s) / r(s);
        case '^': return s => Math.pow(l(s), r(s));
        case '<': return s => (l(s) < r(s) ? 1 : 0);
        case '<=': return s => (l(s) <= r(s) ? 1 : 0);
        case '>': return s => (l(s) > r(s) ? 1 : 0);
        case '>=': return s => (l(s) >= r(s) ? 1 : 0);
        default: throw new ExprError(`Unknown operator "${node.op}".`, 0);
      }
    }
    case 'call': {
      const def = FUNCTIONS.get(node.name);
      if (!def) {
        if (slotOf.has(node.name) || CONSTANTS.has(node.name)) {
          throw new ExprError(`${node.name} is not a function; write ${node.name}*(…) to multiply.`, node.pos);
        }
        throw new ExprError(`Unknown function "${node.name}".`, node.pos);
      }
      if (node.args.length < def.min || node.args.length > def.max) {
        const want = def.min === def.max ? `${def.min}` : `${def.min} to ${def.max}`;
        throw new ExprError(`${node.name} takes ${want} argument${def.max === 1 ? '' : 's'}, got ${node.args.length}.`, node.pos);
      }
      const args = node.args.map(a => compileNode(a, slotOf));
      const fn = def.fn;
      if (args.length === 1) {
        const a0 = args[0];
        return s => fn([a0(s)]);
      }
      return s => fn(args.map(a => a(s)));
    }
  }
}

/**
 * Compile `source` against an ordered list of variable names. The result
 * reads variable `names[i]` from `slots[i]`. A variable named like a
 * constant (`g`) takes precedence over the constant.
 */
export function compileExpression(source: string, names: readonly string[]): Compiled {
  const slotOf = new Map<string, number>();
  names.forEach((name, i) => {
    if (!isValidName(name)) throw new ExprError(`"${name}" cannot be used as a variable name.`, 0);
    slotOf.set(name, i);
  });
  return compileNode(parseExpression(source), slotOf);
}

export type CompileResult = { ok: true; fn: Compiled } | { ok: false; error: string };

/** `compileExpression` that reports errors as values. */
export function tryCompile(source: unknown, names: readonly string[]): CompileResult {
  const text = typeof source === 'number' && Number.isFinite(source) ? String(source) : source;
  if (typeof text !== 'string') return { ok: false, error: 'An expression must be text or a number.' };
  try {
    return { ok: true, fn: compileExpression(text, names) };
  } catch (error) {
    return { ok: false, error: error instanceof Error ? error.message : 'The expression could not be read.' };
  }
}

/** Evaluate once with named values (convenience for tests and one-off use). */
export function evaluate(source: string, values: Readonly<Record<string, number>> = {}): number {
  const names = Object.keys(values);
  const fn = compileExpression(source, names);
  return fn(names.map(n => values[n]));
}

/** Names of the variables an expression reads (constants and functions excluded). */
export function freeVariables(source: string): string[] {
  const found = new Set<string>();
  const walk = (node: Expr) => {
    switch (node.type) {
      case 'var':
        if (!CONSTANTS.has(node.name)) found.add(node.name);
        break;
      case 'neg':
        walk(node.arg);
        break;
      case 'bin':
        walk(node.left);
        walk(node.right);
        break;
      case 'call':
        node.args.forEach(walk);
        break;
      default:
        break;
    }
  };
  walk(parseExpression(source));
  return Array.from(found);
}

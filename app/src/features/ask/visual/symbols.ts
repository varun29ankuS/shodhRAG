/**
 * Symbol meanings for equations: the agent writes a ```symbols block (JSON
 * list of {symbol, meaning, definedAt}) next to an equation, and every
 * occurrence of those symbols in the answer's math gets a hover/focus
 * explanation.
 *
 * Occurrences are found in the LaTeX source, not in KaTeX's output: the
 * equation's tokens are compared with the symbol's tokens (so `\Phi_{q}`
 * matches `\Phi_q` and `\mathbf{x}` matches `\mathbf x`), and each match is
 * wrapped as `{\htmlData{shodh-sym=N}{…}}`. KaTeX turns that into an element
 * with `data-shodh-sym="N"`; `katexTrust` lets KaTeX accept exactly this
 * command with exactly this attribute and nothing else. The wrapper is
 * removed again (`stripSymbolWrappers`) wherever the LaTeX is read back.
 *
 * Pure module, unit-tested with Node (`app/tests/visualSymbols.test.ts`).
 */

export const SYMBOL_ATTR = 'shodh-sym';
export const MAX_SYMBOLS = 40;
const MAX_SYMBOL_CHARS = 120;
const MAX_MEANING_CHARS = 600;
const MAX_PAPER_CHARS = 300;
/** Longest LaTeX annotated; longer equations are left as written. */
const MAX_TEX_CHARS = 8_000;

export interface SymbolNote {
  /** The symbol as written in the equation's LaTeX, e.g. `\Phi_q`. */
  symbol: string;
  meaning: string;
  definedAt: { paper: string; page: number | null } | null;
}

export type SymbolsParseResult = { ok: true; symbols: SymbolNote[] } | { ok: false; error: string };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function text(value: unknown, max: number): string {
  return typeof value === 'string' ? Array.from(value.replace(/\s+/g, ' ').trim()).slice(0, max).join('') : '';
}

function readDefinedAt(value: unknown): SymbolNote['definedAt'] {
  if (!isRecord(value)) return null;
  const paper = text(value.paper ?? value.file, MAX_PAPER_CHARS);
  const page = typeof value.page === 'number' && Number.isInteger(value.page) && value.page > 0 ? value.page : null;
  return paper || page !== null ? { paper, page } : null;
}

/** Valid symbol notes of a value (stored targets, parsed blocks), deduplicated by symbol. */
export function readSymbolNotes(value: unknown): SymbolNote[] {
  const list = Array.isArray(value) ? value : isRecord(value) && Array.isArray(value.symbols) ? value.symbols : [];
  const out: SymbolNote[] = [];
  for (const item of list) {
    if (out.length >= MAX_SYMBOLS) break;
    if (!isRecord(item)) continue;
    const symbol = text(item.symbol ?? item.latex, MAX_SYMBOL_CHARS);
    const meaning = text(item.meaning ?? item.description, MAX_MEANING_CHARS);
    if (!symbol || !meaning || symbolTokens(symbol).length === 0) continue;
    if (out.some(o => o.symbol === symbol)) continue;
    out.push({ symbol, meaning, definedAt: readDefinedAt(item.definedAt ?? item.defined_at) });
  }
  return out;
}

/** A ```symbols block: a JSON list (or {"symbols": [...]}) of symbol notes. */
export function parseSymbolsBlock(source: string): SymbolsParseResult {
  let parsed: unknown;
  try {
    parsed = JSON.parse(source);
  } catch {
    return { ok: false, error: 'The symbol list is not valid JSON.' };
  }
  if (!Array.isArray(parsed) && !(isRecord(parsed) && Array.isArray(parsed.symbols))) {
    return { ok: false, error: 'The symbol list must be a JSON list of {"symbol", "meaning"} entries.' };
  }
  const symbols = readSymbolNotes(parsed);
  if (symbols.length === 0) return { ok: false, error: 'No entry has both a "symbol" and a "meaning".' };
  return { ok: true, symbols };
}

/** Every ```symbols block of a message, merged in order (first meaning of a symbol wins). */
export function symbolsInMessage(content: string): SymbolNote[] {
  const out: SymbolNote[] = [];
  for (const match of content.matchAll(/(^|\n)[ \t]*(`{3,}|~{3,})symbols[ \t]*\n([\s\S]*?)\n[ \t]*\2/g)) {
    const result = parseSymbolsBlock(match[3]);
    if (!result.ok) continue;
    for (const s of result.symbols) {
      if (out.length >= MAX_SYMBOLS) return out;
      if (!out.some(o => o.symbol === s.symbol)) out.push(s);
    }
  }
  return out;
}

// ------------------------------------------------------------- tokens

export interface TexToken {
  /** Normalised text: a control word (`\Phi`), one character, `{` or `}`. */
  text: string;
  start: number;
  end: number;
}

/** Commands whose next brace group is a name or text, never math to annotate. */
const OPAQUE_ARG = new Set([
  '\\begin', '\\end', '\\text', '\\textrm', '\\textbf', '\\textit', '\\textsf', '\\texttt', '\\mbox',
  '\\operatorname', '\\label', '\\tag', '\\color', '\\textcolor', '\\href', '\\url', '\\htmlData',
  '\\htmlClass', '\\htmlId', '\\htmlStyle', '\\ref', '\\eqref',
]);
/** Commands whose single argument is part of the symbol (a styled or accented letter). */
const STYLE = new Set([
  '\\mathbf', '\\boldsymbol', '\\bm', '\\mathcal', '\\mathbb', '\\mathrm', '\\mathsf', '\\mathfrak',
  '\\mathit', '\\mathtt', '\\vec', '\\hat', '\\bar', '\\tilde', '\\dot', '\\ddot', '\\widehat',
  '\\widetilde', '\\overline', '\\check', '\\breve',
]);
/** Tokens a wrapped span may never contain (they belong to the surrounding structure). */
const STRUCTURAL = new Set(['&', '\\\\', '\\left', '\\right', '\\middle', '\\over', '\\choose', '\\atop', '\\cr', '\\nonumber', '\\notag']);

/** Raw tokens of LaTeX: control words, control symbols, single characters; spaces dropped. */
export function tokenizeTex(tex: string): TexToken[] {
  const tokens: TexToken[] = [];
  let i = 0;
  while (i < tex.length) {
    const c = tex[i];
    if (c === ' ' || c === '\t' || c === '\n' || c === '\r') {
      i++;
      continue;
    }
    if (c === '\\') {
      const word = /^\\[A-Za-z]+/.exec(tex.slice(i));
      const len = word ? word[0].length : Math.min(2, tex.length - i);
      tokens.push({ text: tex.slice(i, i + len), start: i, end: i + len });
      i += len;
      continue;
    }
    // One code point (surrogate pairs stay together).
    const cp = tex.codePointAt(i) ?? 0;
    const len = cp > 0xffff ? 2 : 1;
    tokens.push({ text: tex.slice(i, i + len), start: i, end: i + len });
    i += len;
  }
  return tokens;
}

/** Index of the `}` closing the `{` at `open`, or -1. */
function closing(tokens: readonly TexToken[], open: number): number {
  let depth = 0;
  for (let k = open; k < tokens.length; k++) {
    if (tokens[k].text === '{') depth++;
    else if (tokens[k].text === '}') {
      depth--;
      if (depth === 0) return k;
    }
  }
  return -1;
}

/**
 * Tokens with braces around a single token dropped after `_`, `^` and style
 * commands, so spellings of one symbol compare equal: `\Phi_{q}` → `\Phi _ q`,
 * `\mathbf{x}` → `\mathbf x`. `protect` marks tokens inside opaque arguments.
 */
function normalize(tokens: readonly TexToken[]): { tokens: TexToken[]; opaque: boolean[] } {
  const out: TexToken[] = [];
  const opaque: boolean[] = [];
  let k = 0;
  let opaqueUntil = -1;
  while (k < tokens.length) {
    const t = tokens[k];
    const inOpaque = k <= opaqueUntil;
    const prev = tokens[k - 1]?.text;
    if (t.text === '{' && prev !== undefined && (prev === '_' || prev === '^' || STYLE.has(prev))) {
      const close = closing(tokens, k);
      if (close === k + 2) {
        out.push({ text: tokens[k + 1].text, start: t.start, end: tokens[close].end });
        opaque.push(inOpaque);
        k = close + 1;
        continue;
      }
    }
    if (OPAQUE_ARG.has(t.text) && tokens[k + 1]?.text === '{') {
      const close = closing(tokens, k + 1);
      if (close > 0) opaqueUntil = Math.max(opaqueUntil, close);
    }
    out.push(t);
    opaque.push(inOpaque || OPAQUE_ARG.has(t.text));
    k++;
  }
  return { tokens: out, opaque };
}

/** Normalised tokens of a symbol's LaTeX. */
export function symbolTokens(symbol: string): string[] {
  return normalize(tokenizeTex(symbol)).tokens.map(t => t.text);
}

function balanced(tokens: readonly TexToken[]): boolean {
  let depth = 0;
  for (const t of tokens) {
    if (t.text === '{') depth++;
    else if (t.text === '}') {
      depth--;
      if (depth < 0) return false;
    }
  }
  return depth === 0;
}

export interface AnnotatedTex {
  tex: string;
  /** Indexes (into the symbol list) of the symbols found. */
  matched: number[];
}

/**
 * The LaTeX with every occurrence of the symbols wrapped for KaTeX, longest
 * symbols first, never overlapping, never inside a name/text argument, a
 * sub/superscript of another symbol or the argument of a style command.
 */
export function annotateTex(tex: string, symbols: readonly SymbolNote[]): AnnotatedTex {
  if (symbols.length === 0 || tex.length > MAX_TEX_CHARS || tex.includes('\\htmlData')) return { tex, matched: [] };
  const { tokens, opaque } = normalize(tokenizeTex(tex));
  const order = symbols
    .map((s, index) => ({ index, toks: symbolTokens(s.symbol) }))
    .filter(s => s.toks.length > 0)
    .sort((a, b) => b.toks.length - a.toks.length);
  const taken = new Array<boolean>(tokens.length).fill(false);
  const spans: { start: number; end: number; index: number }[] = [];
  for (const { index, toks } of order) {
    for (let k = 0; k + toks.length <= tokens.length; k++) {
      let same = true;
      for (let m = 0; m < toks.length && same; m++) same = tokens[k + m].text === toks[m] && !taken[k + m] && !opaque[k + m];
      if (!same) continue;
      const span = tokens.slice(k, k + toks.length);
      if (span.some(t => STRUCTURAL.has(t.text)) || !balanced(span)) continue;
      const before = tokens[k - 1]?.text;
      if (before === '_' || before === '^' || (before !== undefined && STYLE.has(before))) continue;
      // A control word must not continue into a following letter ("\in" in "\int" is one token anyway).
      for (let m = 0; m < toks.length; m++) taken[k + m] = true;
      spans.push({ start: span[0].start, end: span[span.length - 1].end, index });
    }
  }
  if (spans.length === 0) return { tex, matched: [] };
  spans.sort((a, b) => b.start - a.start);
  let out = tex;
  for (const s of spans) {
    out = `${out.slice(0, s.start)}{\\htmlData{${SYMBOL_ATTR}=${s.index}}{${out.slice(s.start, s.end)}}}${out.slice(s.end)}`;
  }
  const matched = Array.from(new Set(spans.map(s => s.index))).sort((a, b) => a - b);
  return { tex: out, matched };
}

/** The LaTeX as written: every symbol wrapper added by `annotateTex` removed. */
export function stripSymbolWrappers(tex: string): string {
  const marker = `{\\htmlData{${SYMBOL_ATTR}=`;
  let out = tex;
  for (let guard = 0; guard < 1_000; guard++) {
    const at = out.indexOf(marker);
    if (at < 0) break;
    const idEnd = out.indexOf('}{', at + marker.length);
    if (idEnd < 0 || !/^\d+$/.test(out.slice(at + marker.length, idEnd))) break;
    const bodyStart = idEnd + 2;
    let depth = 1;
    let k = bodyStart;
    for (; k < out.length && depth > 0; k++) {
      const c = out[k];
      if (c === '\\') {
        k++;
        continue;
      }
      if (c === '{') depth++;
      else if (c === '}') depth--;
    }
    // k is one past the body's closing brace; the wrapper's own closing brace follows.
    if (depth !== 0 || out[k] !== '}') break;
    out = `${out.slice(0, at)}${out.slice(bodyStart, k - 1)}${out.slice(k + 1)}`;
  }
  return out;
}

/**
 * KaTeX `trust`: only `\htmlData` with exactly one `shodh-sym` attribute
 * holding a number. KaTeX names `\htmlData` attributes with their `data-`
 * prefix in the context it asks about.
 */
export function katexTrust(context: { command?: string; attributes?: Record<string, unknown> }): boolean {
  if (context.command !== '\\htmlData') return false;
  const attributes = context.attributes ?? {};
  const keys = Object.keys(attributes);
  const key = `data-${SYMBOL_ATTR}`;
  return keys.length === 1 && keys[0] === key && /^\d{1,3}$/.test(String(attributes[key]));
}

/** KaTeX options shared by every renderer that shows symbol meanings. */
export const KATEX_SYMBOL_OPTIONS = { trust: katexTrust } as const;

/** One-line description of a symbol for tooltips and screen readers. */
export function symbolDescription(note: SymbolNote): string {
  const where = note.definedAt
    ? ` (defined in ${[note.definedAt.paper, note.definedAt.page !== null ? `page ${note.definedAt.page}` : ''].filter(Boolean).join(', ')})`
    : '';
  return `${note.meaning}${where}`;
}

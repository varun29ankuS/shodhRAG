/**
 * The frontend's static module graph, read the way `app/src-tauri/src/acl_manifest.rs`
 * reads it: every `from '…'` and `import('…')` of a relative or `@/` module, type-only
 * imports included (they can only add modules), with comments removed first. Used by the
 * import-boundary tests of the transcript and the floating chat.
 */
import { existsSync, readFileSync, statSync } from 'node:fs';
import { dirname, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const SRC = fileURLToPath(new URL('../src/', import.meta.url));

const IMPORT = /(?:\bfrom|\bimport)\s*\(?\s*["']([^"'\n]+)["']/g;
const EXTENSIONS = ['ts', 'tsx', 'js', 'jsx', 'mjs', 'cjs'];

/** `text` without its comments (a `//` after `:` or inside quotes, as in a URL, is kept). */
export function withoutComments(text: string): string {
  return text.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:'"`\\])\/\/.*$/gm, '$1');
}

function resolveImport(from: string, spec: string): string | null {
  const clean = spec.split(/[?#]/)[0];
  let base: string;
  if (clean.startsWith('@/')) base = resolve(SRC, clean.slice(2));
  else if (clean.startsWith('./') || clean.startsWith('../')) base = resolve(dirname(from), clean);
  else return null;
  const candidates = [base, ...EXTENSIONS.map(e => `${base}.${e}`), ...EXTENSIONS.map(e => resolve(base, `index.${e}`))];
  for (const candidate of candidates) {
    if (existsSync(candidate) && statSync(candidate).isFile()) {
      return EXTENSIONS.some(e => candidate.endsWith(`.${e}`)) ? candidate : null;
    }
  }
  throw new Error(`${relative(SRC, from)}: import '${spec}' names no file`);
}

/**
 * Every module reachable from `roots` (paths relative to `app/src`), as relative paths with
 * `/`, never entering `excluded`.
 */
export function moduleGraph(roots: readonly string[], excluded: readonly string[] = []): Map<string, string> {
  const out = new Map<string, string>();
  const queue = roots.map(root => resolve(SRC, root));
  while (queue.length > 0) {
    const file = queue.shift() as string;
    const name = relative(SRC, file).replace(/\\/g, '/');
    if (out.has(name) || excluded.includes(name)) continue;
    const text = withoutComments(readFileSync(file, 'utf8'));
    out.set(name, text);
    for (const match of text.matchAll(IMPORT)) {
      const next = resolveImport(file, match[1]);
      if (next) queue.push(next);
    }
  }
  return out;
}

/** The modules of `graph` whose code (comments removed) matches `pattern`, with what matched. */
export function naming(graph: Map<string, string>, pattern: RegExp): string[] {
  const found: string[] = [];
  for (const [name, text] of graph) {
    const hits = text.match(new RegExp(pattern.source, 'g'));
    if (hits) found.push(`${name}: ${[...new Set(hits)].join(', ')}`);
  }
  return found;
}

export function source(path: string): string {
  return readFileSync(resolve(SRC, path), 'utf8');
}

/**
 * Rendering LaTeX with symbol meanings: the annotated source is used only
 * when KaTeX accepts it, so a symbol annotation can never break an equation
 * that renders without one. Results are cached (answers re-render on every
 * streamed token).
 */

import { loadedKatex } from './katexRuntime';
import { annotateTex, katexTrust } from './symbols';
import type { SymbolNote } from './symbols';

const cache = new Map<string, string>();
const MAX_CACHED = 600;

function symbolsKey(symbols: readonly SymbolNote[]): string {
  return symbols.map(s => s.symbol).join('\u0001');
}

/** `tex` with the symbols wrapped for KaTeX, or `tex` itself when nothing matched or KaTeX refused. */
export function safeAnnotate(tex: string, symbols: readonly SymbolNote[], display: boolean): string {
  if (symbols.length === 0) return tex;
  // Callers render math after `useKatex(true)`; without KaTeX nothing can be checked.
  const katex = loadedKatex()?.katex;
  if (!katex) return tex;
  const key = `${display ? 'd' : 'i'}\u0002${symbolsKey(symbols)}\u0002${tex}`;
  const hit = cache.get(key);
  if (hit !== undefined) return hit;
  const { tex: wrapped, matched } = annotateTex(tex, symbols);
  let out = tex;
  if (matched.length > 0) {
    try {
      katex.renderToString(wrapped, { displayMode: display, throwOnError: true, trust: katexTrust });
      out = wrapped;
    } catch {
      out = tex;
    }
  }
  cache.set(key, out);
  if (cache.size > MAX_CACHED) {
    const oldest = cache.keys().next().value;
    if (oldest !== undefined) cache.delete(oldest);
  }
  return out;
}

function escapeHtml(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

/**
 * KaTeX HTML for `tex` with symbol annotations; errors render as KaTeX's red
 * source. Call after `useKatex(true)`; if KaTeX could not be loaded, the
 * LaTeX source is shown as code.
 */
export function renderTex(tex: string, display: boolean, symbols: readonly SymbolNote[] = []): string {
  const katex = loadedKatex()?.katex;
  if (!katex) return `<code>${escapeHtml(tex)}</code>`;
  const source = safeAnnotate(tex, symbols, display);
  return katex.renderToString(source, { displayMode: display, throwOnError: false, trust: katexTrust });
}

/**
 * KaTeX and rehype-katex, loaded with the first math on screen instead of at
 * startup (together they are one of the largest parts of the bundle).
 *
 * Components that render math call `useKatex(true)`: it returns the loaded
 * runtime (null only if it failed to load), or suspends the component
 * (React `use`) until it has loaded, so they need a Suspense boundary above
 * them.
 */
import { use, useEffect, useRef, useState } from 'react';
import type katexModule from 'katex';
import type rehypeKatexModule from 'rehype-katex';

export interface KatexRuntime {
  katex: typeof katexModule;
  rehypeKatex: typeof rehypeKatexModule;
}

let runtime: KatexRuntime | null = null;
let pending: Promise<KatexRuntime | null> | null = null;

/**
 * Load KaTeX (once). Resolves to null if the chunk cannot be loaded: math
 * then shows as its LaTeX source for the rest of the session rather than
 * failing the whole answer (and the settled promise is kept, so a render
 * never suspends on it again).
 */
export function loadKatex(): Promise<KatexRuntime | null> {
  if (!pending) {
    pending = Promise.all([import('katex'), import('rehype-katex')])
      .then(([katex, rehypeKatex]) => {
        runtime = { katex: katex.default, rehypeKatex: rehypeKatex.default };
        return runtime;
      })
      .catch(error => {
        console.error('KaTeX could not be loaded; math is shown as source:', error);
        return null;
      });
  }
  return pending;
}

/** The runtime if it has loaded, else null (never starts a load). */
export function loadedKatex(): KatexRuntime | null {
  return runtime;
}

/**
 * The runtime when `needed`, suspending until it has loaded; when not
 * needed, whatever has loaded already (null before the first math).
 */
export function useKatex(needed: boolean): KatexRuntime | null {
  if (runtime || !needed) return runtime;
  return use(loadKatex());
}


/**
 * For content already on screen that gains its first math (a streaming
 * answer): on the first render this is `useKatex(needed)`; once mounted it
 * never suspends (that would hide the visible content behind the Suspense
 * fallback) but starts the load and renders again when KaTeX has landed,
 * the math showing as its source until then.
 */
export function useKatexWhenNeeded(needed: boolean): KatexRuntime | null {
  const mounted = useRef(false);
  const [, setLoaded] = useState(0);
  useEffect(() => {
    mounted.current = true;
  }, []);
  useEffect(() => {
    if (!needed || runtime) return;
    let live = true;
    void loadKatex().then(() => {
      if (live) setLoaded(n => n + 1);
    });
    return () => {
      live = false;
    };
  }, [needed]);
  if (runtime || !needed) return runtime;
  return mounted.current ? null : use(loadKatex());
}

/**
 * Drawing mermaid source: the library loaded on first use, renders
 * serialised, and the deterministic repair (`mermaidRepair.ts`) tried
 * once when the parser rejects the source as written.
 */

import { repairMermaid } from './mermaidRepair';

type Mermaid = typeof import('mermaid').default;

let mermaidPromise: Promise<Mermaid> | null = null;

/** mermaid is large; load it on the first diagram only. */
function loadMermaid(): Promise<Mermaid> {
  if (!mermaidPromise) {
    mermaidPromise = import('mermaid')
      .then(mod => mod.default)
      .catch(error => {
        mermaidPromise = null;
        throw error;
      });
  }
  return mermaidPromise;
}

// mermaid.render is global-stateful; serialise renders so concurrent diagrams
// (several in one answer, or a streaming re-render) cannot interleave.
let renderQueue: Promise<unknown> = Promise.resolve();

function enqueue<T>(work: (mermaid: Mermaid) => Promise<T>): Promise<T> {
  const job = renderQueue.then(async () => work(await loadMermaid()));
  renderQueue = job.catch(() => undefined);
  return job;
}

/** The parser's message for a rejected source. */
export function diagramErrorMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim()) return error.message;
  if (typeof error === 'string' && error.trim()) return error;
  return 'The diagram could not be drawn.';
}

/**
 * The source mermaid accepts: as written, else its deterministic repair.
 * Rejects with the parser's error for the source as written.
 */
async function parseable(mermaid: Mermaid, source: string): Promise<{ source: string; repaired: boolean }> {
  try {
    await mermaid.parse(source);
    return { source, repaired: false };
  } catch (error) {
    const repair = repairMermaid(source);
    if (!repair.changed) throw error;
    try {
      await mermaid.parse(repair.source);
    } catch {
      // The repair did not help: report the problem in what was written.
      throw error;
    }
    return { source: repair.source, repaired: true };
  }
}

export interface DrawnDiagram {
  svg: string;
  /** The source drawn (the repaired one when `repaired`). */
  source: string;
  /** The source as written did not parse and its deterministic repair was drawn. */
  repaired: boolean;
}

/**
 * Draw mermaid source; `id` must be unique in the document. `textLabels`
 * draws labels as SVG text instead of HTML in <foreignObject>, so the markup
 * stands alone (export) and can be drawn to a canvas.
 */
export function drawDiagram(id: string, source: string, dark: boolean, textLabels = false): Promise<DrawnDiagram> {
  return enqueue(async mermaid => {
    mermaid.initialize({
      startOnLoad: false,
      // Diagrams come from model output: no scripts, no click handlers, labels escaped.
      securityLevel: 'strict',
      theme: dark ? 'dark' : 'default',
      fontFamily: '"Geist Variable", system-ui, sans-serif',
      ...(textLabels ? { htmlLabels: false, flowchart: { htmlLabels: false } } : {}),
    });
    const usable = await parseable(mermaid, source);
    const { svg } = await mermaid.render(id, usable.source);
    return { svg, ...usable };
  });
}

/** Render mermaid source to SVG markup (repaired when needed); `id` must be unique in the document. */
export function renderDiagram(id: string, source: string, dark: boolean, textLabels = false): Promise<string> {
  return drawDiagram(id, source, dark, textLabels).then(drawn => drawn.svg);
}

/** Whether `source` parses, as written or repaired; the source that does, or the parser's message. */
export function checkDiagram(source: string): Promise<{ ok: true; source: string } | { ok: false; error: string }> {
  return enqueue(async mermaid => {
    try {
      const usable = await parseable(mermaid, source);
      return { ok: true as const, source: usable.source };
    } catch (error) {
      return { ok: false as const, error: diagramErrorMessage(error) };
    }
  });
}

/**
 * Export of a gallery visual to a file the user picks in a save dialog.
 *
 * - Diagrams are drawn again for export with text labels (no HTML inside
 *   <foreignObject>), so the SVG stands alone and a PNG of it can be drawn
 *   to a canvas without tainting it.
 * - Sketches, plots and charts are taken from the drawing on screen (the
 *   pop-out or the card): the <svg> is copied with its computed colours and
 *   fonts written inline, since the app's theme variables do not exist
 *   outside the app.
 * - Simulations export their JSON spec with the slider positions; equations
 *   their TeX; tables their Markdown.
 */

import { save } from '@tauri-apps/plugin-dialog';
import { writeFile, writeTextFile } from '@tauri-apps/plugin-fs';
import { renderDiagram } from '../ask/visual/VisualBlocks';
import type { VisualRecord } from './model';
import { paramValuesOf } from './model';

export type ExportFormat = 'svg' | 'png' | 'json' | 'tex' | 'md';

/** Formats offered for a kind, the first being the default. */
export function exportFormats(kind: VisualRecord['kind']): ExportFormat[] {
  switch (kind) {
    case 'mermaid':
    case 'chart':
    case 'svg':
    case 'plot':
      return ['svg', 'png'];
    case 'simulation':
      return ['json'];
    case 'equation':
      return ['tex'];
    case 'table':
      return ['md'];
    default:
      return [];
  }
}

export const FORMAT_LABEL: Record<ExportFormat, string> = {
  svg: 'SVG image',
  png: 'PNG image',
  json: 'Simulation spec (JSON)',
  tex: 'TeX equation',
  md: 'Markdown table',
};

const SVG_NS = 'http://www.w3.org/2000/svg';
/** Styles written inline on every exported element. */
const INLINE_STYLES = [
  'fill', 'fill-opacity', 'stroke', 'stroke-width', 'stroke-opacity', 'stroke-dasharray', 'stroke-linecap',
  'stroke-linejoin', 'opacity', 'font-family', 'font-size', 'font-weight', 'font-style', 'text-anchor',
  'dominant-baseline', 'visibility', 'display', 'color',
];
/** PNG pixels per CSS pixel. */
const PNG_SCALE = 2;
/** Largest PNG side, in pixels. */
const MAX_PNG_SIDE = 8_000;

function fileStem(title: string): string {
  const stem = title.replace(/[\\/:*?"<>|\u0000-\u001f]+/g, ' ').replace(/\s+/g, ' ').trim().slice(0, 80);
  return stem || 'visual';
}

/** The largest <svg> drawn inside `container` (charts and plots have small icon SVGs too). */
export function mainSvg(container: Element): SVGSVGElement | null {
  let best: SVGSVGElement | null = null;
  let area = 0;
  for (const svg of Array.from(container.querySelectorAll('svg'))) {
    if (svg.parentElement?.closest('svg')) continue;
    const box = svg.getBoundingClientRect();
    const a = box.width * box.height;
    if (a > area) {
      area = a;
      best = svg;
    }
  }
  return best;
}

/** A standalone copy of a drawn SVG: computed styles inline, size and namespace set. */
export function standaloneSvg(svg: SVGSVGElement, background: string | null): string {
  const clone = svg.cloneNode(true) as SVGSVGElement;
  const source = [svg, ...Array.from(svg.querySelectorAll('*'))];
  const target = [clone, ...Array.from(clone.querySelectorAll('*'))];
  source.forEach((el, i) => {
    const copy = target[i] as SVGElement | undefined;
    if (!copy) return;
    const computed = window.getComputedStyle(el);
    const parts: string[] = [];
    for (const prop of INLINE_STYLES) {
      const value = computed.getPropertyValue(prop);
      if (value) parts.push(`${prop}:${value}`);
    }
    copy.setAttribute('style', parts.join(';'));
    copy.removeAttribute('class');
  });
  const box = svg.getBoundingClientRect();
  const width = Math.max(1, Math.round(box.width));
  const height = Math.max(1, Math.round(box.height));
  clone.setAttribute('xmlns', SVG_NS);
  clone.setAttribute('width', String(width));
  clone.setAttribute('height', String(height));
  if (!clone.getAttribute('viewBox')) clone.setAttribute('viewBox', `0 0 ${width} ${height}`);
  clone.removeAttribute('aria-hidden');
  if (background) {
    const rect = document.createElementNS(SVG_NS, 'rect');
    rect.setAttribute('width', '100%');
    rect.setAttribute('height', '100%');
    rect.setAttribute('fill', background);
    clone.insertBefore(rect, clone.firstChild);
  }
  return new XMLSerializer().serializeToString(clone);
}

/** Width and height of SVG markup (its width/height, else its viewBox). */
function svgSize(markup: string): { width: number; height: number } {
  const doc = new DOMParser().parseFromString(markup, 'image/svg+xml');
  const root = doc.documentElement;
  const num = (v: string | null) => {
    const n = v ? parseFloat(v) : NaN;
    return Number.isFinite(n) && n > 0 ? n : null;
  };
  const view = (root.getAttribute('viewBox') ?? '').split(/[\s,]+/).map(Number);
  const width = num(root.getAttribute('width')) ?? (view.length === 4 && view[2] > 0 ? view[2] : 800);
  const height = num(root.getAttribute('height')) ?? (view.length === 4 && view[3] > 0 ? view[3] : 600);
  return { width, height };
}

/** PNG bytes of SVG markup. */
export async function svgToPng(markup: string): Promise<Uint8Array> {
  const { width, height } = svgSize(markup);
  const scale = Math.min(PNG_SCALE, MAX_PNG_SIDE / Math.max(width, height));
  const image = new Image();
  const url = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(markup)}`;
  await new Promise<void>((resolve, reject) => {
    image.onload = () => resolve();
    image.onerror = () => reject(new Error('The drawing could not be converted to an image.'));
    image.src = url;
  });
  const canvas = document.createElement('canvas');
  canvas.width = Math.max(1, Math.round(width * scale));
  canvas.height = Math.max(1, Math.round(height * scale));
  const context = canvas.getContext('2d');
  if (!context) throw new Error('The image could not be drawn.');
  context.drawImage(image, 0, 0, canvas.width, canvas.height);
  const blob = await new Promise<Blob | null>(resolve => canvas.toBlob(resolve, 'image/png'));
  if (!blob) throw new Error('The image could not be encoded.');
  return new Uint8Array(await blob.arrayBuffer());
}

/** A diagram drawn again for export: text labels only, theme colours baked in. */
async function mermaidExportSvg(source: string, dark: boolean): Promise<string> {
  const id = `export-mmd-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
  try {
    const svg = await renderDiagram(id, source, dark, true);
    const doc = new DOMParser().parseFromString(svg, 'image/svg+xml');
    const root = doc.documentElement;
    root.setAttribute('xmlns', SVG_NS);
    // mermaid sizes with max-width only; give the file a real size.
    const view = (root.getAttribute('viewBox') ?? '').split(/[\s,]+/).map(Number);
    if (view.length === 4 && view[2] > 0 && view[3] > 0) {
      root.setAttribute('width', String(Math.ceil(view[2])));
      root.setAttribute('height', String(Math.ceil(view[3])));
    }
    root.removeAttribute('style');
    return new XMLSerializer().serializeToString(root);
  } finally {
    // mermaid leaves its measuring element behind when a render fails.
    document.getElementById(id)?.remove();
    document.getElementById(`d${id}`)?.remove();
  }
}

export interface ExportRequest {
  record: Pick<VisualRecord, 'kind' | 'title' | 'source' | 'params' | 'version'>;
  format: ExportFormat;
  /** Where the visual is drawn (pop-out stage or card), for kinds exported from the screen. */
  container: Element | null;
  dark: boolean;
}

/** The file contents of an export. */
async function render(request: ExportRequest): Promise<{ bytes?: Uint8Array; text?: string }> {
  const { record, format } = request;
  if (format === 'json') {
    let spec: unknown;
    try {
      spec = JSON.parse(record.source);
    } catch {
      throw new Error('The simulation spec is not valid JSON.');
    }
    const values = paramValuesOf(record.params);
    return { text: `${JSON.stringify({ spec, params: { values } }, null, 2)}\n` };
  }
  if (format === 'tex') return { text: `${record.source}\n` };
  if (format === 'md') return { text: `${record.source}\n` };
  let markup: string;
  if (record.kind === 'mermaid') {
    markup = await mermaidExportSvg(record.source, request.dark);
  } else {
    const svg = request.container ? mainSvg(request.container) : null;
    if (!svg) throw new Error('Open the visual first, then export it.');
    const surface = request.container ? window.getComputedStyle(request.container).backgroundColor : '';
    const background = format === 'png' && surface && surface !== 'rgba(0, 0, 0, 0)' ? surface : null;
    markup = standaloneSvg(svg, background);
  }
  if (format === 'svg') return { text: markup };
  return { bytes: await svgToPng(markup) };
}

/**
 * Asks where to save and writes the file. Resolves to the path, or null when
 * the user cancelled.
 */
export async function exportVisual(request: ExportRequest): Promise<string | null> {
  const contents = await render(request);
  const { record, format } = request;
  const version = record.version > 1 ? ` v${record.version}` : '';
  const path = await save({
    defaultPath: `${fileStem(record.title)}${version}.${format}`,
    filters: [{ name: FORMAT_LABEL[format], extensions: [format] }],
  });
  if (!path) return null;
  if (contents.bytes) await writeFile(path, contents.bytes);
  else await writeTextFile(path, contents.text ?? '');
  return path;
}

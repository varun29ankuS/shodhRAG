/**
 * Diagrams written by the agent as ```diagram fences: a typed graph, never
 * drawing code. The model names the parts and how they connect; Shodh checks
 * the graph (every edge joins declared nodes, every citation is a source of
 * the answer, every file path is a relative path in the code folder) and
 * lays it out itself (`diagramLayout.ts`). The pipeline follows GitDiagram
 * (MIT): the model explains structure, code renders it.
 *
 *   { "layout": "flow", "title": "Indexing",
 *     "nodes": [{"id": "parse", "label": "Parse PDF", "kind": "step", "cite": 2},
 *               {"id": "embed", "label": "Embed chunks", "emphasis": true}],
 *     "edges": [{"from": "parse", "to": "embed", "label": "chunks"}] }
 *
 * Layouts: flow (top to bottom), architecture (left to right, nodes may share
 * a `group`), sequence (nodes are participants, edges are messages in order),
 * layers (a stack, top first), timeline (nodes in order, each with `at`),
 * quadrant (`x`, `y` in 0..1 and `axes`), loop (edges carry `sign` + or -;
 * every cycle is labelled reinforcing or balancing).
 *
 * `emphasis` marks at most two nodes. `change` (added, removed, changed)
 * draws an architecture delta on one layout. `path` (Code mode) and `cite`
 * make a node open its file or source.
 *
 * Pure module, unit-tested with Node (`app/tests/visualDiagram.test.ts`).
 */

/** Largest diagram spec read, in characters. */
export const DIAGRAM_MAX_CHARS = 20_000;
export const MAX_NODES = 12;
export const MAX_EDGES = 16;
export const MAX_MESSAGES = 20;
export const MAX_PARTICIPANTS = 6;
export const MAX_EMPHASIS = 2;
export const MAX_LABEL_CHARS = 60;
export const MAX_DETAIL_CHARS = 200;
export const MAX_EDGE_LABEL_CHARS = 24;
export const MAX_PATH_CHARS = 300;

export const LAYOUTS = ['flow', 'architecture', 'sequence', 'layers', 'timeline', 'quadrant', 'loop'] as const;
export type DiagramLayoutKind = (typeof LAYOUTS)[number];

export const CHANGES = ['added', 'removed', 'changed'] as const;
export type Change = (typeof CHANGES)[number];

export interface DiagramNode {
  id: string;
  label: string;
  /** A short type tag shown under the label ("service", "store"). */
  kind: string | null;
  group: string | null;
  detail: string | null;
  /** Source number of the answer this node comes from. */
  cite: number | null;
  /** File or folder relative to the code folder. */
  path: string | null;
  emphasis: boolean;
  change: Change | null;
  /** Quadrant position, 0..1 from the left and from the bottom. */
  x: number | null;
  y: number | null;
  /** Timeline: when. */
  at: string | null;
}

export interface DiagramEdge {
  from: string;
  to: string;
  label: string | null;
  /** async, optional, return and data edges are dashed. */
  kind: string | null;
  sign: '+' | '-' | null;
  change: Change | null;
}

export interface DiagramSpec {
  layout: DiagramLayoutKind;
  title: string;
  nodes: DiagramNode[];
  edges: DiagramEdge[];
  /** Quadrant axes: [low, high] labels. */
  axes: { x: [string, string]; y: [string, string] } | null;
}

export interface DiagramContext {
  /**
   * The source numbers of the answer (`[n]` that open a passage); null when
   * the text has no sources to check against (citations then are refused).
   */
  citations: ReadonlySet<number> | null;
  /** File paths are allowed (a Code mode answer). */
  codeMode: boolean;
}

export type DiagramParse =
  | { ok: true; spec: DiagramSpec; /** Paths to check in the code folder (not those of removed nodes). */ paths: string[] }
  | { ok: false; errors: string[] };

const ID = /^[A-Za-z][A-Za-z0-9_.:-]{0,47}$/;
const DASHED_KINDS = new Set(['async', 'optional', 'return', 'data', 'event']);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function text(value: unknown, max: number): string | null {
  if (typeof value !== 'string') return null;
  const flat = value.replace(/\s+/g, ' ').trim();
  if (!flat) return null;
  const chars = Array.from(flat);
  return chars.length > max ? `${chars.slice(0, max - 1).join('')}…` : flat;
}

/** Whether `kind` is drawn dashed. */
export function isDashed(kind: string | null): boolean {
  return kind !== null && DASHED_KINDS.has(kind.toLowerCase());
}

/**
 * A path the model may name: relative, inside the folder (no `..`, drive
 * or leading slash), forward slashes. Null when it is not one.
 */
export function cleanPath(value: string): string | null {
  const path = value.trim().replace(/\\/g, '/').replace(/^\.\//, '');
  if (!path || path.length > MAX_PATH_CHARS) return null;
  if (path.startsWith('/') || /^[A-Za-z]:/.test(path) || path.includes(':')) return null;
  if (path.split('/').some(part => part === '..')) return null;
  if (/[\u0000-\u001f]/.test(path)) return null;
  return path.replace(/\/+$/, '') || null;
}

function nodeLimit(layout: DiagramLayoutKind): number {
  return layout === 'sequence' ? MAX_PARTICIPANTS : MAX_NODES;
}

function edgeLimit(layout: DiagramLayoutKind): number {
  return layout === 'sequence' ? MAX_MESSAGES : MAX_EDGES;
}

/** Parse and check a ```diagram block against the answer it is in. */
export function parseDiagram(source: string, context: DiagramContext): DiagramParse {
  if (source.length > DIAGRAM_MAX_CHARS) return { ok: false, errors: [`The diagram is longer than ${DIAGRAM_MAX_CHARS} characters.`] };
  let raw: unknown;
  try {
    raw = JSON.parse(source);
  } catch (error) {
    return { ok: false, errors: [`The diagram is not valid JSON (${error instanceof Error ? error.message : String(error)}).`] };
  }
  if (!isRecord(raw)) return { ok: false, errors: ['The diagram must be a JSON object.'] };
  const errors: string[] = [];
  const layout = typeof raw.layout === 'string' ? raw.layout.trim().toLowerCase() : '';
  if (!(LAYOUTS as readonly string[]).includes(layout)) {
    return { ok: false, errors: [`"layout" must be one of ${LAYOUTS.join(', ')}.`] };
  }
  const kind = layout as DiagramLayoutKind;
  const rawNodes = Array.isArray(raw.nodes) ? raw.nodes : null;
  const rawEdges = raw.edges === undefined || raw.edges === null ? [] : Array.isArray(raw.edges) ? raw.edges : null;
  if (!rawNodes || rawNodes.length === 0) return { ok: false, errors: ['"nodes" must be a non-empty list.'] };
  if (!rawEdges) return { ok: false, errors: ['"edges" must be a list.'] };
  if (rawNodes.length > nodeLimit(kind)) {
    errors.push(`A ${kind} diagram has at most ${nodeLimit(kind)} ${kind === 'sequence' ? 'participants' : 'nodes'}; split it into an overview and a detail.`);
  }
  if (rawEdges.length > edgeLimit(kind)) {
    errors.push(`A ${kind} diagram has at most ${edgeLimit(kind)} ${kind === 'sequence' ? 'messages' : 'edges'}.`);
  }

  const nodes: DiagramNode[] = [];
  const ids = new Set<string>();
  const paths: string[] = [];
  rawNodes.slice(0, nodeLimit(kind)).forEach((item, index) => {
    const where = `nodes[${index}]`;
    if (!isRecord(item)) {
      errors.push(`${where} must be an object.`);
      return;
    }
    const id = typeof item.id === 'string' ? item.id.trim() : '';
    if (!ID.test(id)) {
      errors.push(`${where}.id must start with a letter and use letters, digits, _ . : - (got ${JSON.stringify(item.id ?? null)}).`);
      return;
    }
    if (ids.has(id)) {
      errors.push(`Node id "${id}" is used twice.`);
      return;
    }
    ids.add(id);
    const label = text(item.label, MAX_LABEL_CHARS);
    if (!label) errors.push(`Node "${id}" has no label.`);
    let cite: number | null = null;
    if (item.cite !== undefined && item.cite !== null) {
      const n = typeof item.cite === 'string' ? Number(item.cite.replace(/^\[|\]$/g, '')) : item.cite;
      if (typeof n !== 'number' || !Number.isInteger(n) || n < 1) {
        errors.push(`Node "${id}": "cite" must be a source number such as 2.`);
      } else if (!context.citations || !context.citations.has(n)) {
        errors.push(`Node "${id}" cites [${n}], which is not a source of this answer.`);
      } else {
        cite = n;
      }
    }
    let change: Change | null = null;
    if (item.change !== undefined && item.change !== null) {
      if (typeof item.change === 'string' && (CHANGES as readonly string[]).includes(item.change)) change = item.change as Change;
      else errors.push(`Node "${id}": "change" must be added, removed or changed.`);
    }
    let path: string | null = null;
    if (item.path !== undefined && item.path !== null) {
      const cleaned = typeof item.path === 'string' ? cleanPath(item.path) : null;
      if (!context.codeMode) errors.push(`Node "${id}" names a file, which only Code mode diagrams can.`);
      else if (!cleaned) errors.push(`Node "${id}": "path" must be relative to the code folder (got ${JSON.stringify(item.path)}).`);
      else {
        path = cleaned;
        if (change !== 'removed' && !paths.includes(cleaned)) paths.push(cleaned);
      }
    }
    const coordinate = (key: 'x' | 'y'): number | null => {
      const v = item[key];
      if (kind !== 'quadrant') return null;
      if (typeof v !== 'number' || !Number.isFinite(v) || v < 0 || v > 1) {
        errors.push(`Node "${id}": "${key}" must be a number from 0 to 1.`);
        return null;
      }
      return v;
    };
    const at = text(item.at, 32);
    if (kind === 'timeline' && !at) errors.push(`Node "${id}" needs "at" (when it happens).`);
    nodes.push({
      id,
      label: label ?? id,
      kind: text(item.kind, 24),
      group: kind === 'architecture' ? text(item.group, 32) : null,
      detail: text(item.detail, MAX_DETAIL_CHARS),
      cite,
      path,
      emphasis: item.emphasis === true,
      change,
      x: coordinate('x'),
      y: coordinate('y'),
      at: kind === 'timeline' ? at : null,
    });
  });

  const emphasised = nodes.filter(n => n.emphasis).length;
  if (emphasised > MAX_EMPHASIS) errors.push(`At most ${MAX_EMPHASIS} nodes may have "emphasis" (${emphasised} do); keep the accent for what matters most.`);

  const edges: DiagramEdge[] = [];
  if (rawEdges.length > 0 && (kind === 'layers' || kind === 'timeline' || kind === 'quadrant')) {
    errors.push(`A ${kind} diagram has no edges.`);
  } else {
    rawEdges.slice(0, edgeLimit(kind)).forEach((item, index) => {
      const where = `edges[${index}]`;
      if (!isRecord(item)) {
        errors.push(`${where} must be an object.`);
        return;
      }
      const from = typeof item.from === 'string' ? item.from.trim() : '';
      const to = typeof item.to === 'string' ? item.to.trim() : '';
      if (!ids.has(from)) errors.push(`${where}.from "${from}" is not a node id.`);
      if (!ids.has(to)) errors.push(`${where}.to "${to}" is not a node id.`);
      let sign: '+' | '-' | null = null;
      if (item.sign !== undefined && item.sign !== null) {
        const s = String(item.sign).trim();
        if (s === '+' || s === 'positive' || s === 'same') sign = '+';
        else if (s === '-' || s === '−' || s === 'negative' || s === 'opposite') sign = '-';
        else errors.push(`${where}.sign must be "+" or "-".`);
      }
      if (kind === 'loop' && sign === null && !errors.some(e => e.startsWith(`${where}.sign`))) {
        errors.push(`${where} needs "sign": "+" (same direction) or "-" (opposite direction).`);
      }
      let change: Change | null = null;
      if (item.change !== undefined && item.change !== null) {
        if (typeof item.change === 'string' && (CHANGES as readonly string[]).includes(item.change)) change = item.change as Change;
        else errors.push(`${where}.change must be added, removed or changed.`);
      }
      if (from === to && kind !== 'loop' && kind !== 'sequence') errors.push(`${where} joins "${from}" to itself.`);
      edges.push({ from, to, label: text(item.label, MAX_EDGE_LABEL_CHARS), kind: text(item.kind, 16), sign, change });
    });
  }

  let axes: DiagramSpec['axes'] = null;
  if (kind === 'quadrant') {
    const pair = (value: unknown): [string, string] | null => {
      if (!Array.isArray(value) || value.length !== 2) return null;
      const a = text(value[0], 32);
      const b = text(value[1], 32);
      return a && b ? [a, b] : null;
    };
    const rawAxes = isRecord(raw.axes) ? raw.axes : null;
    const x = rawAxes ? pair(rawAxes.x) : null;
    const y = rawAxes ? pair(rawAxes.y) : null;
    if (!x || !y) errors.push('A quadrant needs "axes": {"x": ["low", "high"], "y": ["low", "high"]}.');
    else axes = { x, y };
  }
  if (kind === 'loop' && edges.length > 0 && errors.length === 0 && !hasCycle(nodes, edges)) {
    errors.push('A loop diagram needs at least one cycle; use flow for a process that ends.');
  }

  if (errors.length > 0) return { ok: false, errors };
  return {
    ok: true,
    spec: { layout: kind, title: text(raw.title, 80) ?? '', nodes, edges, axes },
    paths,
  };
}

function hasCycle(nodes: readonly DiagramNode[], edges: readonly DiagramEdge[]): boolean {
  const index = new Map(nodes.map((n, i) => [n.id, i]));
  const out: number[][] = nodes.map(() => []);
  for (const e of edges) out[index.get(e.from) ?? 0].push(index.get(e.to) ?? 0);
  const state = nodes.map(() => 0);
  const visit = (v: number): boolean => {
    state[v] = 1;
    for (const w of out[v]) {
      if (state[w] === 1) return true;
      if (state[w] === 0 && visit(w)) return true;
    }
    state[v] = 2;
    return false;
  };
  return nodes.some((_, v) => state[v] === 0 && visit(v));
}

/** One feedback loop of a loop diagram. */
export interface FeedbackLoop {
  /** Node ids around the loop, starting at the first declared one. */
  nodes: string[];
  /** Indices into `spec.edges`, in order around the loop. */
  edges: number[];
  /** R: reinforcing (an even number of - links); B: balancing (odd). */
  polarity: 'R' | 'B';
}

/** Most loops listed; larger graphs are past the diagram budget anyway. */
export const MAX_LOOPS = 24;

/**
 * Every elementary cycle (each listed once, starting at its first declared
 * node; parallel edges with different signs are different loops; a
 * self-link is a loop of one), labelled R or B, in a deterministic order.
 */
export function feedbackLoops(spec: Pick<DiagramSpec, 'nodes' | 'edges'>): FeedbackLoop[] {
  const index = new Map(spec.nodes.map((n, i) => [n.id, i]));
  const out: { to: number; edge: number }[][] = spec.nodes.map(() => []);
  spec.edges.forEach((e, i) => {
    const from = index.get(e.from);
    const to = index.get(e.to);
    if (from !== undefined && to !== undefined) out[from].push({ to, edge: i });
  });
  const loops: FeedbackLoop[] = [];
  for (let start = 0; start < spec.nodes.length && loops.length < MAX_LOOPS; start++) {
    const pathNodes: number[] = [start];
    const pathEdges: number[] = [];
    const onPath = new Set<number>([start]);
    const walk = (v: number) => {
      for (const { to, edge } of out[v]) {
        if (loops.length >= MAX_LOOPS) return;
        if (to === start) {
          const edges = [...pathEdges, edge];
          const negatives = edges.filter(i => spec.edges[i].sign === '-').length;
          loops.push({
            nodes: pathNodes.map(i => spec.nodes[i].id),
            edges,
            polarity: negatives % 2 === 0 ? 'R' : 'B',
          });
        } else if (to > start && !onPath.has(to)) {
          onPath.add(to);
          pathNodes.push(to);
          pathEdges.push(edge);
          walk(to);
          pathNodes.pop();
          pathEdges.pop();
          onPath.delete(to);
        }
      }
    };
    walk(start);
  }
  return loops;
}

/** A plain-text account of the diagram (for "Expand & ask" and screen readers). */
export function describeDiagram(spec: DiagramSpec): string {
  const byId = new Map(spec.nodes.map(n => [n.id, n]));
  const name = (id: string) => byId.get(id)?.label ?? id;
  const lines: string[] = [];
  lines.push(`${spec.title || 'Diagram'} (${spec.layout}).`);
  for (const n of spec.nodes) {
    const facts = [n.kind, n.group && `in ${n.group}`, n.at, n.path, n.cite !== null ? `[${n.cite}]` : null, n.change]
      .filter(Boolean)
      .join(', ');
    lines.push(`- ${n.label}${facts ? ` (${facts})` : ''}${n.detail ? `: ${n.detail}` : ''}`);
  }
  for (const e of spec.edges) {
    const sign = e.sign ? ` (${e.sign})` : '';
    lines.push(`- ${name(e.from)} → ${name(e.to)}${e.label ? `: ${e.label}` : ''}${sign}${e.change ? ` [${e.change}]` : ''}`);
  }
  if (spec.layout === 'loop') {
    feedbackLoops(spec).forEach((loop, i) => {
      lines.push(`Loop ${loop.polarity}${i + 1}: ${[...loop.nodes, loop.nodes[0]].map(name).join(' → ')}`);
    });
  }
  return lines.join('\n');
}

/** What one node is, with its links, for "Ask about this". */
export function describeNode(spec: DiagramSpec, id: string): string {
  const node = spec.nodes.find(n => n.id === id);
  if (!node) return '';
  const byId = new Map(spec.nodes.map(n => [n.id, n]));
  const name = (other: string) => byId.get(other)?.label ?? other;
  const parts = [node.label];
  if (node.kind) parts.push(`(${node.kind})`);
  const lines = [parts.join(' ')];
  if (node.detail) lines.push(node.detail);
  if (node.path) lines.push(`File: ${node.path}`);
  if (node.cite !== null) lines.push(`Source: [${node.cite}]`);
  const outgoing = spec.edges.filter(e => e.from === id).map(e => `→ ${name(e.to)}${e.label ? ` (${e.label})` : ''}`);
  const incoming = spec.edges.filter(e => e.to === id).map(e => `← ${name(e.from)}${e.label ? ` (${e.label})` : ''}`);
  if (outgoing.length) lines.push(`Leads to: ${outgoing.join('; ')}`);
  if (incoming.length) lines.push(`Comes from: ${incoming.join('; ')}`);
  return lines.join('\n');
}

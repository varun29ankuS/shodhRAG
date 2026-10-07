/**
 * Citation graph view logic: filtering (library only, year range, method),
 * capping what is drawn and clustering the rest, and the keyboard list that
 * presents the same data without the drawing. Pure module, unit-tested with
 * Node (`app/tests/graphModel.test.ts`).
 */

import type { BuildProgress, GraphViewData, ViewNode } from './graphTypes.ts';

/** Most nodes drawn; the rest are folded into one cluster node per library paper. */
export const MAX_DRAWN = 160;

export interface GraphFilters {
  libraryOnly: boolean;
  yearFrom: number | null;
  yearTo: number | null;
  /** Method id, or null for every method. */
  method: string | null;
}

export const DEFAULT_FILTERS: GraphFilters = { libraryOnly: false, yearFrom: null, yearTo: null, method: null };

/** A node to draw: a paper or a cluster of papers that were not drawn. */
export interface DrawnNode {
  id: string;
  label: string;
  kind: 'library' | 'external' | 'cluster';
  year: number | null;
  /** Library papers citing it (papers) or papers folded in (clusters). */
  weight: number;
  /** For a cluster: the library paper whose references it holds. */
  parent?: string;
}

export interface DrawnEdge {
  source: string;
  target: string;
}

export interface ShapedGraph {
  nodes: DrawnNode[];
  edges: DrawnEdge[];
  /** Papers matching the filters (drawn or clustered). */
  matching: number;
  /** Papers folded into clusters. */
  clustered: number;
}

/** Smallest and largest year among the nodes, or null when none has a year. */
export function yearBounds(nodes: ViewNode[]): { min: number; max: number } | null {
  const years = nodes.map(n => n.year).filter((y): y is number => typeof y === 'number');
  if (years.length === 0) return null;
  return { min: Math.min(...years), max: Math.max(...years) };
}

/** Whether a node passes the filters. A method filter keeps library papers using it and the works they cite. */
export function passes(node: ViewNode, filters: GraphFilters, methodPapers: Set<string> | null): boolean {
  if (filters.libraryOnly && !node.inLibrary) return false;
  if (filters.yearFrom !== null && (node.year === null || node.year < filters.yearFrom)) return false;
  if (filters.yearTo !== null && (node.year === null || node.year > filters.yearTo)) return false;
  if (methodPapers && !methodPapers.has(node.id)) return false;
  return true;
}

/** Ids kept by a method filter: library papers using the method and the works they cite. */
export function methodNeighbourhood(data: GraphViewData, method: string | null): Set<string> | null {
  if (!method) return null;
  const users = new Set(data.nodes.filter(n => n.methods.includes(method)).map(n => n.id));
  const keep = new Set(users);
  for (const [from, to] of data.edges) if (users.has(from)) keep.add(to);
  return keep;
}

/**
 * The nodes and edges to draw. Library papers are always drawn; external works
 * are drawn by how many library papers cite them, up to `cap` nodes in all.
 * External works left over are folded into one cluster per citing library
 * paper ("+12 references"), so no citation silently disappears.
 */
export function shapeGraph(data: GraphViewData, filters: GraphFilters, cap = MAX_DRAWN): ShapedGraph {
  const methodPapers = methodNeighbourhood(data, filters.method);
  const kept = data.nodes.filter(n => passes(n, filters, methodPapers));
  const library = kept.filter(n => n.inLibrary);
  const external = kept
    .filter(n => !n.inLibrary)
    .sort((a, b) => b.libraryCiters - a.libraryCiters || (b.citedByCount ?? 0) - (a.citedByCount ?? 0) || a.id.localeCompare(b.id));
  const room = Math.max(0, cap - library.length);
  const drawnExternal = external.slice(0, room);
  const drawnIds = new Set([...library, ...drawnExternal].map(n => n.id));
  const keptIds = new Set(kept.map(n => n.id));
  const nodes: DrawnNode[] = [
    ...library.map(n => ({ id: n.id, label: n.label, kind: 'library' as const, year: n.year, weight: n.libraryCiters })),
    ...drawnExternal.map(n => ({ id: n.id, label: n.label, kind: 'external' as const, year: n.year, weight: n.libraryCiters })),
  ];
  const edges: DrawnEdge[] = [];
  const folded = new Map<string, Set<string>>();
  for (const [from, to] of data.edges) {
    if (!keptIds.has(from) || !keptIds.has(to)) continue;
    if (drawnIds.has(from) && drawnIds.has(to)) {
      edges.push({ source: from, target: to });
    } else if (drawnIds.has(from)) {
      const set = folded.get(from) ?? new Set<string>();
      set.add(to);
      folded.set(from, set);
    }
  }
  const clusteredIds = new Set<string>();
  for (const [parent, members] of [...folded.entries()].sort((a, b) => a[0].localeCompare(b[0]))) {
    const id = `cluster:${parent}`;
    members.forEach(m => clusteredIds.add(m));
    nodes.push({ id, label: `+${members.size} ${members.size === 1 ? 'reference' : 'references'}`, kind: 'cluster', year: null, weight: members.size, parent });
    edges.push({ source: parent, target: id });
  }
  return { nodes, edges, matching: kept.length, clustered: clusteredIds.size };
}

/** One row of the keyboard list: a paper and what it cites and is cited by among the kept nodes. */
export interface ListRow {
  node: ViewNode;
  cites: number;
  citedBy: number;
}

/** The list alternative of the graph: the same filtered papers, library papers first. */
export function graphList(data: GraphViewData, filters: GraphFilters): ListRow[] {
  const methodPapers = methodNeighbourhood(data, filters.method);
  const kept = data.nodes.filter(n => passes(n, filters, methodPapers));
  const ids = new Set(kept.map(n => n.id));
  const cites = new Map<string, number>();
  const citedBy = new Map<string, number>();
  for (const [from, to] of data.edges) {
    if (!ids.has(from) || !ids.has(to)) continue;
    cites.set(from, (cites.get(from) ?? 0) + 1);
    citedBy.set(to, (citedBy.get(to) ?? 0) + 1);
  }
  return kept
    .map(node => ({ node, cites: cites.get(node.id) ?? 0, citedBy: citedBy.get(node.id) ?? 0 }))
    .sort((a, b) => Number(b.node.inLibrary) - Number(a.node.inLibrary) || b.citedBy - a.citedBy || a.node.label.localeCompare(b.node.label));
}

/** Radius of a drawn node: library papers larger, everything grows gently with weight. */
export function nodeRadius(node: DrawnNode): number {
  const base = node.kind === 'library' ? 9 : node.kind === 'cluster' ? 7 : 4;
  return base + Math.min(8, Math.sqrt(Math.max(0, node.weight)) * 1.5);
}

/** A one-line description of build progress. */
export function progressLabel(p: BuildProgress): string {
  switch (p.stage) {
    case 'scanning':
      return `Reading ${p.file} (${p.done + 1} of ${p.total})`;
    case 'resolving':
      return `Matching papers (${p.done} of ${p.total})`;
    case 'writing':
      return `Saving the graph (${p.done} of ${p.total})`;
  }
}

/** A short title for a paper, for lists and tooltips. */
export function shortLabel(label: string, max = 60): string {
  return label.length <= max ? label : `${label.slice(0, max - 1)}…`;
}

/**
 * The graph limited to some library papers (a workspace's): those papers, the works they
 * cite, and the citations between them. Library papers outside the limit are left out,
 * also as cited works. Methods stay (they are filters, not papers).
 */
export function restrictGraph(data: GraphViewData, keepLibrary: (node: ViewNode) => boolean): GraphViewData {
  const library = new Set(data.nodes.filter(n => n.inLibrary && keepLibrary(n)).map(n => n.id));
  const keep = new Set(library);
  const inLibrary = new Set(data.nodes.filter(n => n.inLibrary).map(n => n.id));
  for (const [from, to] of data.edges) {
    if (library.has(from) && !inLibrary.has(to)) keep.add(to);
  }
  return {
    ...data,
    nodes: data.nodes.filter(n => keep.has(n.id)),
    edges: data.edges.filter(([from, to]) => keep.has(from) && keep.has(to)),
  };
}

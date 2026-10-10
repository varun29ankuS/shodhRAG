/**
 * Pan and zoom of the citation graph drawing.
 *
 * Graph coordinates (where the force layout puts nodes) map to the drawing's
 * viewBox as `screen = graph * scale + (x, y)`. Every function is pure and
 * unit-tested with Node (`app/tests/graphViewport.test.ts`); the view only
 * turns pointer, wheel and key input into calls of these.
 */

import { clampScale, wheelZoomFactor } from '../focus/zoomMath.ts';
import type { Point, Size, View } from '../focus/zoomMath.ts';

export type { Point, Size, View };
export { wheelZoomFactor };

export const GRAPH_MIN_ZOOM = 0.2;
export const GRAPH_MAX_ZOOM = 6;
/** Factor of one + / − step. */
export const GRAPH_ZOOM_STEP = 1.3;
/** Space kept around the nodes when fitting them. */
export const GRAPH_FIT_PADDING = 32;
/** Part of the nodes' extent (in viewBox units) that always stays in view. */
export const GRAPH_KEEP_VISIBLE = 48;
/** Pointer travel (viewBox units) below which a press on a node is a click, not a drag. */
export const DRAG_THRESHOLD = 4;

export const IDENTITY: View = { scale: 1, x: 0, y: 0 };

export interface Bounds {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

export function graphScale(scale: number): number {
  return clampScale(scale, GRAPH_MIN_ZOOM, GRAPH_MAX_ZOOM);
}

/** The bounding box of circles at `points` with radius `radius(i)`; null for none. */
export function nodeBounds(points: readonly Point[], radius: (index: number) => number = () => 0): Bounds | null {
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (let i = 0; i < points.length; i++) {
    const p = points[i];
    if (!Number.isFinite(p.x) || !Number.isFinite(p.y)) continue;
    const r = Math.max(0, radius(i));
    minX = Math.min(minX, p.x - r);
    minY = Math.min(minY, p.y - r);
    maxX = Math.max(maxX, p.x + r);
    maxY = Math.max(maxY, p.y + r);
  }
  return Number.isFinite(minX) ? { minX, minY, maxX, maxY } : null;
}

/** The graph point drawn at `screen` (viewBox coordinates). */
export function toGraph(view: View, screen: Point): Point {
  return { x: (screen.x - view.x) / view.scale, y: (screen.y - view.y) / view.scale };
}

/** Where the graph point `graph` is drawn (viewBox coordinates). */
export function toScreen(view: View, graph: Point): Point {
  return { x: graph.x * view.scale + view.x, y: graph.y * view.scale + view.y };
}

/** The view that shows all of `bounds`, centred, never enlarged past 2×. */
export function fitGraph(bounds: Bounds | null, viewport: Size, padding = GRAPH_FIT_PADDING): View {
  if (!bounds) return IDENTITY;
  const width = Math.max(1, bounds.maxX - bounds.minX);
  const height = Math.max(1, bounds.maxY - bounds.minY);
  const room = { width: Math.max(1, viewport.width - padding * 2), height: Math.max(1, viewport.height - padding * 2) };
  const scale = graphScale(Math.min(room.width / width, room.height / height, 2));
  const cx = (bounds.minX + bounds.maxX) / 2;
  const cy = (bounds.minY + bounds.maxY) / 2;
  return { scale, x: viewport.width / 2 - cx * scale, y: viewport.height / 2 - cy * scale };
}

/**
 * Keep the drawing reachable: at least `keep` viewBox units of the nodes'
 * extent stay inside the viewport on each axis, so panning can never lose
 * the graph off-screen.
 */
export function clampGraphView(view: View, bounds: Bounds | null, viewport: Size, keep = GRAPH_KEEP_VISIBLE): View {
  const scale = graphScale(view.scale);
  if (!bounds) return { scale, x: view.x, y: view.y };
  const axis = (pos: number, min: number, max: number, room: number) => {
    const lo = min * scale + pos;
    const hi = max * scale + pos;
    const need = Math.min(keep, (hi - lo) / 2, room / 2);
    if (hi < need) return pos + (need - hi);
    if (lo > room - need) return pos - (lo - (room - need));
    return pos;
  };
  return {
    scale,
    x: axis(view.x, bounds.minX, bounds.maxX, viewport.width),
    y: axis(view.y, bounds.minY, bounds.maxY, viewport.height),
  };
}

/** Zoom to `nextScale` keeping the graph point under `at` (viewBox coordinates) in place. */
export function zoomGraphAt(view: View, nextScale: number, at: Point): View {
  const scale = graphScale(nextScale);
  const ratio = scale / view.scale;
  return { scale, x: at.x - (at.x - view.x) * ratio, y: at.y - (at.y - view.y) * ratio };
}

/** One + / − step around the viewport centre. */
export function zoomGraphStep(view: View, direction: 1 | -1, viewport: Size): View {
  const factor = direction > 0 ? GRAPH_ZOOM_STEP : 1 / GRAPH_ZOOM_STEP;
  return zoomGraphAt(view, view.scale * factor, { x: viewport.width / 2, y: viewport.height / 2 });
}

export function panGraph(view: View, dx: number, dy: number): View {
  return { scale: view.scale, x: view.x + dx, y: view.y + dy };
}

function distance(a: Point, b: Point): number {
  return Math.hypot(a.x - b.x, a.y - b.y);
}

function midpoint(a: Point, b: Point): Point {
  return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
}

/**
 * Two-finger pinch: the graph point under the fingers' starting midpoint
 * follows their current midpoint, scaled by how far they spread.
 */
export function pinchGraph(start: View, from: readonly [Point, Point], to: readonly [Point, Point]): View {
  const before = distance(from[0], from[1]);
  const after = distance(to[0], to[1]);
  const factor = before > 0 && after > 0 ? after / before : 1;
  const scale = graphScale(start.scale * factor);
  const anchor = toGraph(start, midpoint(from[0], from[1]));
  const now = midpoint(to[0], to[1]);
  return { scale, x: now.x - anchor.x * scale, y: now.y - anchor.y * scale };
}

/** Whether a press moved far enough (viewBox units) to be a drag. */
export function isDrag(from: Point, to: Point, threshold = DRAG_THRESHOLD): boolean {
  return distance(from, to) >= threshold;
}

/** The viewBox point under a client (CSS pixel) position of an element showing `viewBox`. */
export function clientToViewBox(
  client: Point,
  rect: { left: number; top: number; width: number; height: number },
  viewBox: Size,
): Point {
  const sx = rect.width > 0 ? viewBox.width / rect.width : 1;
  const sy = rect.height > 0 ? viewBox.height / rect.height : 1;
  return { x: (client.x - rect.left) * sx, y: (client.y - rect.top) * sy };
}

/**
 * Zoom and pan of the focus pop-out stage.
 *
 * A view places content of natural size `content` inside a `viewport`:
 * the content's top-left corner sits at (x, y) in viewport pixels and is
 * drawn at `scale`. Pure module, unit-tested with Node
 * (`app/tests/focusZoom.test.ts`).
 */

export interface Size {
  width: number;
  height: number;
}

export interface Point {
  x: number;
  y: number;
}

export interface View {
  scale: number;
  x: number;
  y: number;
}

export const MIN_ZOOM = 0.05;
export const MAX_ZOOM = 8;
/** Factor of one zoom step (+ / − keys and buttons). */
export const ZOOM_STEP = 1.25;
/** Space kept around content when fitting it. */
export const FIT_PADDING = 24;
/** Fit never enlarges small content beyond this. */
export const MAX_FIT_SCALE = 3;
/** Pixels an arrow key pans (Shift: `PAN_STEP_LARGE`). */
export const PAN_STEP = 48;
export const PAN_STEP_LARGE = 240;

export function clampScale(scale: number, min = MIN_ZOOM, max = MAX_ZOOM): number {
  if (!Number.isFinite(scale)) return 1;
  return Math.min(max, Math.max(min, scale));
}

function valid(size: Size): boolean {
  return Number.isFinite(size.width) && Number.isFinite(size.height) && size.width > 0 && size.height > 0;
}

/** The scale at which content fits the viewport (with padding). */
export function fitScale(content: Size, viewport: Size, padding = FIT_PADDING): number {
  if (!valid(content) || !valid(viewport)) return 1;
  const w = Math.max(1, viewport.width - padding * 2);
  const h = Math.max(1, viewport.height - padding * 2);
  return clampScale(Math.min(w / content.width, h / content.height, MAX_FIT_SCALE));
}

/**
 * Keep content reachable: on an axis where the scaled content is smaller
 * than the viewport it is centred; otherwise it may not leave a gap larger
 * than `padding` at either edge.
 */
export function clampView(view: View, content: Size, viewport: Size, padding = FIT_PADDING): View {
  if (!valid(content) || !valid(viewport)) return view;
  const axis = (pos: number, size: number, room: number) => {
    const scaled = size * view.scale;
    // Half a pixel of slack absorbs rounding in a fitted scale.
    if (scaled + padding * 2 <= room + 0.5) return (room - scaled) / 2;
    const min = room - scaled - padding;
    const max = padding;
    return Math.min(max, Math.max(min, pos));
  };
  return { scale: view.scale, x: axis(view.x, content.width, viewport.width), y: axis(view.y, content.height, viewport.height) };
}

/** Content at `scale`, centred. */
export function centeredView(content: Size, viewport: Size, scale: number): View {
  const s = clampScale(scale);
  return clampView({ scale: s, x: (viewport.width - content.width * s) / 2, y: (viewport.height - content.height * s) / 2 }, content, viewport);
}

/** Content fitted and centred. */
export function fitView(content: Size, viewport: Size): View {
  return centeredView(content, viewport, fitScale(content, viewport));
}

/**
 * Zoom to `nextScale` keeping the content point under `point` (viewport
 * coordinates) where it is: the cursor for wheel zoom, the centre for keys.
 */
export function zoomAt(view: View, nextScale: number, point: Point, content: Size, viewport: Size): View {
  const scale = clampScale(nextScale);
  if (scale === view.scale) return clampView(view, content, viewport);
  const ratio = scale / view.scale;
  const x = point.x - (point.x - view.x) * ratio;
  const y = point.y - (point.y - view.y) * ratio;
  return clampView({ scale, x, y }, content, viewport);
}

/** Zoom by a factor around the viewport centre. */
export function zoomBy(view: View, factor: number, content: Size, viewport: Size): View {
  return zoomAt(view, view.scale * factor, { x: viewport.width / 2, y: viewport.height / 2 }, content, viewport);
}

export function panBy(view: View, dx: number, dy: number, content: Size, viewport: Size): View {
  return clampView({ scale: view.scale, x: view.x + dx, y: view.y + dy }, content, viewport);
}

/** Whether the scaled content is larger than the viewport (panning is useful). */
export function canPan(view: View, content: Size, viewport: Size): boolean {
  return content.width * view.scale > viewport.width + 0.5 || content.height * view.scale > viewport.height + 0.5;
}

/**
 * Zoom factor of one wheel event. Pixel, line and page deltas are
 * normalised; a single event never zooms by more than 2× either way.
 * Trackpad pinch arrives as a wheel event with ctrlKey and small deltas.
 */
export function wheelZoomFactor(deltaY: number, deltaMode = 0): number {
  if (!Number.isFinite(deltaY) || deltaY === 0) return 1;
  const pixels = deltaMode === 1 ? deltaY * 16 : deltaMode === 2 ? deltaY * 400 : deltaY;
  const factor = Math.exp(-pixels * 0.0025);
  return Math.min(2, Math.max(0.5, factor));
}

/** Font size of text-like content (equations, tables) at a zoom level. */
export function scaledFontSize(base: number, scale: number): number {
  return Math.round(base * clampScale(scale) * 10) / 10;
}

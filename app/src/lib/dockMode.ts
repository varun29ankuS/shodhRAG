/**
 * Conversation dock layout: which form the dock takes on each view, how wide
 * a pinned side panel is, and how both are remembered.
 *
 * Pure module (no React, no DOM) so the rules are unit-tested directly with
 * Node (`app/tests/dockMode.test.ts`).
 *
 * Model
 * - Each view remembers whether the dock is pinned there as a side panel and
 *   the panel's width. Unpinned views start with the dock minimised to a pill.
 * - Expanding the pill (click, Ctrl+J) is transient: it opens the dock for the
 *   current visit only. Pinning makes the side panel that view's default.
 * - An open dock renders as a side panel the page reflows around. Only when
 *   the window is too narrow for a panel and a usable page does it fall back to
 *   a floating overlay, and only when expanded on purpose; that fallback is
 *   derived, never stored, so widening the window restores the panel.
 */
import { isViewTab, type ViewTab } from './viewTabs.ts';

/** What the dock renders as. */
export type DockLayout = 'pill' | 'panel' | 'overlay';

/** What a view asks for when nothing has been expanded or collapsed this visit. */
export type DockRequest = 'pill' | 'panel';

export interface DockViewPrefs {
  /** Pinned as a side panel on this view. */
  pinned: boolean;
  /** Preferred side-panel width in CSS pixels (clamped against the window at render). */
  width: number;
}

export type DockPrefs = Partial<Record<ViewTab, DockViewPrefs>>;

export const DOCK_STORAGE_KEY = 'shodh.conversationDock.v2';
const DOCK_PREFS_VERSION = 2;

export const DOCK_MIN_WIDTH = 320;
export const DOCK_DEFAULT_WIDTH = 380;
/** The panel never takes more than this share of the window. */
export const DOCK_MAX_FRACTION = 0.5;
/**
 * Width kept for everything left of the panel: the expanded sidebar (248px)
 * plus the narrowest page that still shows its toolbar without wrapping.
 */
export const DOCK_PAGE_RESERVE = 248 + 480;
/** Window width from which a side panel fits next to a usable page. */
export const DOCK_PANEL_MIN_WINDOW = DOCK_MIN_WIDTH + DOCK_PAGE_RESERVE;
/** Stored widths beyond this are treated as corrupt rather than clamped. */
const DOCK_STORED_WIDTH_LIMIT = 8192;

export const DOCK_KEY_STEP = 16;
export const DOCK_KEY_STEP_LARGE = 64;

/** The dock form a view uses before the person changes it. */
export function defaultDockRequest(view: ViewTab): DockRequest {
  // Every non-Ask view is a working surface (lists, tables, forms) that
  // should keep its full width until the person asks for the panel. Ask shows
  // the conversation itself and never mounts the dock.
  switch (view) {
    case 'ask':
    case 'workspaces':
    case 'library':
    case 'tasks':
    case 'activity':
    case 'settings':
      return 'pill';
  }
}

/** Default preferences for a view with nothing stored. */
export function defaultViewPrefs(view: ViewTab): DockViewPrefs {
  return { pinned: defaultDockRequest(view) === 'panel', width: DOCK_DEFAULT_WIDTH };
}

/** Largest panel width for this window: half the window, and never eating the page reserve. */
export function maxDockWidth(windowWidth: number): number {
  if (!Number.isFinite(windowWidth) || windowWidth <= 0) return DOCK_MIN_WIDTH;
  const byFraction = Math.floor(windowWidth * DOCK_MAX_FRACTION);
  const byReserve = Math.floor(windowWidth - DOCK_PAGE_RESERVE);
  return Math.max(DOCK_MIN_WIDTH, Math.min(byFraction, byReserve));
}

/** Clamp a panel width into [DOCK_MIN_WIDTH, maxDockWidth(window)]; non-finite input falls back to the default. */
export function clampDockWidth(width: number, windowWidth: number): number {
  const max = maxDockWidth(windowWidth);
  const value = Number.isFinite(width) ? Math.round(width) : DOCK_DEFAULT_WIDTH;
  return Math.min(max, Math.max(DOCK_MIN_WIDTH, value));
}

/** Whether a side panel fits next to a usable page in a window this wide. */
export function panelFits(windowWidth: number): boolean {
  return Number.isFinite(windowWidth) && windowWidth >= DOCK_PANEL_MIN_WINDOW;
}

/**
 * The form the dock takes. `open` is whether the dock is expanded; `explicit`
 * is whether that came from the person on this visit (pill, Ctrl+J) rather
 * than from the view's pinned preference.
 *
 * An open dock is a side panel when one fits. Otherwise it floats as an
 * overlay, but only when explicitly expanded: a pinned preference restored
 * on a narrow window stays a pill so nothing is covered unasked.
 */
export function resolveDockLayout(open: boolean, windowWidth: number, explicit: boolean): DockLayout {
  if (!open) return 'pill';
  if (panelFits(windowWidth)) return 'panel';
  return explicit ? 'overlay' : 'pill';
}

/**
 * Keyboard resizing of the right-hand panel. The handle sits on the panel's
 * left edge, so ArrowLeft widens it and ArrowRight narrows it. Returns null for
 * keys that do not resize.
 */
export function widthForKey(key: string, width: number, windowWidth: number, large: boolean): number | null {
  const step = large ? DOCK_KEY_STEP_LARGE : DOCK_KEY_STEP;
  const current = clampDockWidth(width, windowWidth);
  switch (key) {
    case 'ArrowLeft':
      return clampDockWidth(current + step, windowWidth);
    case 'ArrowRight':
      return clampDockWidth(current - step, windowWidth);
    case 'Home':
      return DOCK_MIN_WIDTH;
    case 'End':
      return maxDockWidth(windowWidth);
    default:
      return null;
  }
}

/** Width while dragging the left-edge handle from `startX` to `currentX`. */
export function widthForDrag(startWidth: number, startX: number, currentX: number, windowWidth: number): number {
  return clampDockWidth(startWidth + (startX - currentX), windowWidth);
}

function parseViewPrefs(value: unknown): DockViewPrefs | null {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  if (typeof record.pinned !== 'boolean') return null;
  const width = record.width;
  if (typeof width !== 'number' || !Number.isFinite(width) || width < DOCK_MIN_WIDTH || width > DOCK_STORED_WIDTH_LIMIT) {
    return null;
  }
  return { pinned: record.pinned, width: Math.round(width) };
}

/**
 * Parse the stored record. Anything malformed (bad JSON, wrong version,
 * unknown views, invalid entries) is dropped entry by entry so one bad value
 * never discards the rest.
 */
export function parseDockPrefs(raw: string | null): DockPrefs {
  if (raw === null || raw === '') return {};
  let data: unknown;
  try {
    data = JSON.parse(raw);
  } catch {
    return {};
  }
  if (typeof data !== 'object' || data === null || Array.isArray(data)) return {};
  const record = data as Record<string, unknown>;
  if (record.version !== DOCK_PREFS_VERSION) return {};
  const views = record.views;
  if (typeof views !== 'object' || views === null || Array.isArray(views)) return {};
  const prefs: DockPrefs = {};
  for (const [view, value] of Object.entries(views as Record<string, unknown>)) {
    if (!isViewTab(view) || view === 'ask') continue;
    const parsed = parseViewPrefs(value);
    if (parsed) prefs[view] = parsed;
  }
  return prefs;
}

export function serializeDockPrefs(prefs: DockPrefs): string {
  const views: Record<string, DockViewPrefs> = {};
  for (const [view, value] of Object.entries(prefs)) {
    if (!isViewTab(view) || view === 'ask' || !value) continue;
    const parsed = parseViewPrefs(value);
    if (parsed) views[view] = parsed;
  }
  return JSON.stringify({ version: DOCK_PREFS_VERSION, views });
}

export function viewPrefs(prefs: DockPrefs, view: ViewTab): DockViewPrefs {
  return prefs[view] ?? defaultViewPrefs(view);
}

/** A copy of `prefs` with `patch` applied to `view`. */
export function withViewPrefs(prefs: DockPrefs, view: ViewTab, patch: Partial<DockViewPrefs>): DockPrefs {
  return { ...prefs, [view]: { ...viewPrefs(prefs, view), ...patch } };
}

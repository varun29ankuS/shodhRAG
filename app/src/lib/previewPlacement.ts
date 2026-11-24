/**
 * When the embedded Artifacts preview and the floating chat over it may show. Both are native
 * child webviews (`preview_embed.rs`, `dock_layer.rs`) that paint above this page, so anything of
 * the page's own that must show over them (a dialog, a menu, a popover, the command palette, a
 * toast) hides both until it is gone; the in-page chat then shows instead. Pure, so the Node
 * tests load it.
 */

export interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * The page's own layers that must never be under the preview: modal dialogs, menus and list
 * boxes (the mode menu, selects), Radix popovers and tooltips, toasts and the command palette.
 * The in-page chat is none of these (its panel is a non-modal dialog): it hides while the layer
 * is up.
 */
export const OVERLAY_SELECTORS: readonly string[] = [
  '[role="dialog"][aria-modal="true"]',
  '[role="menu"]',
  '[role="listbox"]',
  '[data-radix-popper-content-wrapper]',
  '[data-sonner-toast]',
  '[cmdk-root]',
  // A pane of the page's own over its right side (the code review pane).
  '[data-shodh-overlay]',
];

function intersects(a: Box, b: Box): boolean {
  return a.width > 0 && a.height > 0 && b.width > 0 && b.height > 0
    && a.x < b.x + b.width && b.x < a.x + a.width
    && a.y < b.y + b.height && b.y < a.y + a.height;
}

/**
 * Whether something of the page's own is above the preview: any modal dialog wherever it is
 * (it takes the whole window's attention), or any overlay over the pane or over the chat's
 * corner box (`dockCorner`, where the floating chat's webview sits).
 */
export function coveredBy(pane: Box, overlays: readonly Box[], modalOpen: boolean, dockCorner: Box | null = null): boolean {
  if (modalOpen) return true;
  return overlays.some(overlay => intersects(overlay, pane) || (dockCorner !== null && intersects(overlay, dockCorner)));
}

/** An overlay of the page's own, with the name the pane says it by. */
export interface NamedBox {
  name: string;
  box: Box;
}

/**
 * What hides the preview, by name, or null when nothing does: a modal dialog wherever it is
 * (`modal`, its name), else the first overlay over the pane or over the chat's corner box. It
 * decides exactly as {@link coveredBy}, and says what the user would have to close.
 */
export function coveringOverlay(pane: Box, overlays: readonly NamedBox[], modal: string | null, dockCorner: Box | null = null): string | null {
  if (modal !== null) return modal;
  const found = overlays.find(({ box }) => intersects(box, pane) || (dockCorner !== null && intersects(box, dockCorner)));
  return found ? found.name : null;
}

/** The longest name the status line shows for what hides the page. */
const MAX_OVERLAY_NAME = 40;

/** What an overlay element is, as {@link overlayName} reads it (no DOM, so tests need none). */
export interface OverlayLike {
  /** Its `aria-label`, if any. */
  label: string | null;
  role: string | null;
  /** A toast (`data-sonner-toast`). */
  toast: boolean;
  /** The command palette (`cmdk-root`). */
  palette: boolean;
}

/** The name the pane uses for an overlay: its own label, else what kind of thing it is. */
export function overlayName(overlay: OverlayLike): string {
  const label = overlay.label?.trim() ?? '';
  if (label) return label.length > MAX_OVERLAY_NAME ? `${label.slice(0, MAX_OVERLAY_NAME - 1)}…` : label;
  if (overlay.toast) return 'a notification';
  if (overlay.palette) return 'the command palette';
  switch (overlay.role) {
    case 'menu':
      return 'a menu';
    case 'listbox':
      return 'a list';
    case 'dialog':
    case 'alertdialog':
      return 'a dialog';
    default:
      return 'a panel';
  }
}

/** The pane's status line while the page is hidden under something of the app's own. */
export function hiddenWhileText(name: string): string {
  return `Hidden while ${name} is open`;
}

/**
 * Where the floating chat's webview sits: the open panel's box at the bottom right of a
 * `viewport`, `edge` from its edges, left of a pane docked at the right (`rightInset`, CSS
 * pixels), and never larger than the room left. Rust places the chat the same way
 * (`dock_layer::dock_bounds`, with the inset the pane reports).
 */
export function dockCornerBox(
  viewport: { width: number; height: number },
  panel: { width: number; height: number },
  edge: number,
  rightInset = 0,
): Box {
  const inset = Number.isFinite(rightInset) && rightInset > 0 ? rightInset : 0;
  const right = viewport.width - inset;
  const width = Math.max(0, Math.min(panel.width, right - 2 * edge));
  const height = Math.max(0, Math.min(panel.height, viewport.height - 2 * edge));
  return { x: Math.max(0, right - edge - width), y: Math.max(0, viewport.height - edge - height), width, height };
}

/** The preview and the floating chat show only while every one of these holds. */
export interface LayerState {
  /** Artifacts is the view on screen. */
  onArtifacts: boolean;
  /** Its preview pane is the pane on screen, for this creation. */
  paneShown: boolean;
  /** The user moved the page to a window of its own. */
  poppedOut: boolean;
  /** Something of the page's own is above the pane ({@link coveredBy}). */
  covered: boolean;
  /** The window is visible (not minimised, not hidden to the tray, the page not hidden). */
  windowVisible: boolean;
}

export function layerVisible(state: LayerState): boolean {
  return state.onArtifacts && state.paneShown && !state.poppedOut && !state.covered && state.windowVisible;
}

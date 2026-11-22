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

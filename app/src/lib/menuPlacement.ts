/** The trigger's box, in viewport pixels (a DOMRect fits). */
export interface AnchorBox {
  top: number;
  bottom: number;
  left: number;
}

/** Where a fixed-position menu goes, and how tall it may be before it scrolls. */
export interface MenuPlacement {
  top: number;
  left: number;
  maxHeight: number;
}

/** Space kept between the menu and the window's edges. */
export const MENU_EDGE = 8;
/** Space between the trigger and the menu. */
export const MENU_GAP = 6;

/**
 * Places a menu of natural size `height` × `width` next to `anchor` in a
 * `viewportWidth` × `viewportHeight` window. It opens above the trigger when it
 * fits there (composers sit low in the window), else below when it fits there,
 * else on the side with more room, capped to that room so it scrolls instead of
 * running off the window. It never starts left of the trigger unless the window's
 * right edge pushes it.
 */
export function placeMenu(
  anchor: AnchorBox,
  height: number,
  width: number,
  viewportWidth: number,
  viewportHeight: number,
): MenuPlacement {
  const above = Math.max(0, anchor.top - MENU_GAP - MENU_EDGE);
  const below = Math.max(0, viewportHeight - anchor.bottom - MENU_GAP - MENU_EDGE);
  const left = Math.max(MENU_EDGE, Math.min(anchor.left, viewportWidth - width - MENU_EDGE));
  const openAbove = height <= above || (height > below && above >= below);
  if (openAbove) {
    const shown = Math.min(height, above);
    return { top: anchor.top - MENU_GAP - shown, left, maxHeight: above };
  }
  return { top: anchor.bottom + MENU_GAP, left, maxHeight: below };
}

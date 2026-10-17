/**
 * Where the webview's own right-click menu is kept. Elsewhere it is turned off: it
 * offers Reload, which drops unsent drafts and running answers, and Back/Forward,
 * which mean nothing in the app. Text fields keep it for Cut, Copy, Paste and the
 * spelling suggestions. No imports: unit-tested under Node.
 */

/** Fields that take typed text. */
export const EDITABLE_SELECTOR = [
  'textarea',
  '[contenteditable]:not([contenteditable="false"])',
  'input:not([type="button"]):not([type="submit"]):not([type="reset"]):not([type="checkbox"]):not([type="radio"]):not([type="range"]):not([type="color"]):not([type="file"]):not([type="image"])',
].join(', ');

/** Whether a right-click on `target` shows the webview's menu. */
export function keepsNativeContextMenu(target: { closest(selector: string): unknown } | null): boolean {
  return target !== null && target.closest(EDITABLE_SELECTOR) !== null;
}

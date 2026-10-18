/**
 * When the webview's own right-click menu is kept. Without a text selection it is
 * the page menu (WebView2: Back, Forward, Reload, Save as, Print; WKWebView: Reload),
 * and Reload drops unsent drafts and running answers, so it is turned off. With text
 * selected the webview shows its selection menu instead (Copy, Search, Look Up, Share
 * and the like, no Reload), which is kept so selected answer or PDF text can be
 * copied with the mouse; text fields keep theirs for Cut, Copy, Paste and spelling.
 * No imports: unit-tested under Node.
 */

/** Fields that take typed text. */
export const EDITABLE_SELECTOR = [
  'textarea',
  '[contenteditable]:not([contenteditable="false"])',
  'input:not([type="button"]):not([type="submit"]):not([type="reset"]):not([type="checkbox"]):not([type="radio"]):not([type="range"]):not([type="color"]):not([type="file"]):not([type="image"])',
].join(', ');

interface SelectionLike {
  readonly isCollapsed: boolean;
  toString(): string;
}

/** Whether `selection` holds text (a caret or whitespace only does not). */
export function selectionHasText(selection: SelectionLike | null): boolean {
  return selection !== null && !selection.isCollapsed && selection.toString().trim().length > 0;
}

/**
 * Whether a right-click on `target` shows the webview's menu: while text is selected
 * (`textSelected`, from `selectionHasText`), or in a text field.
 */
export function keepsNativeContextMenu(target: { closest(selector: string): unknown } | null, textSelected: boolean): boolean {
  if (textSelected) return true;
  return target !== null && target.closest(EDITABLE_SELECTOR) !== null;
}

/**
 * Keyboard map of the focus pop-out stage. Esc is handled by the dialog
 * (stop a running side answer, then close).
 *
 * Pure module, unit-tested with Node (`app/tests/focusKeys.test.ts`).
 */

export type FocusCommand =
  | 'zoomIn'
  | 'zoomOut'
  | 'fit'
  | 'actualSize'
  | 'panLeft'
  | 'panRight'
  | 'panUp'
  | 'panDown'
  | 'toggleMaximize';

export interface FocusKeyInput {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  /** Focus is in a text field, select or contenteditable element. */
  editable: boolean;
  /** The content is larger than the stage, so arrows pan it. */
  pannable: boolean;
}

/**
 * - Ctrl/⌘ with = + − 0: zoom in, out, fit (instead of zooming the window).
 * - Without modifiers, outside text fields: + = / − _ zoom, 0 fit, 1 actual
 *   size, M maximise, arrows pan when the content is larger than the stage.
 */
export function focusCommand(input: FocusKeyInput): FocusCommand | null {
  const { key, altKey, editable, pannable } = input;
  const mod = input.ctrlKey || input.metaKey;
  if (altKey) return null;
  if (mod) {
    if (key === '=' || key === '+') return 'zoomIn';
    if (key === '-' || key === '_') return 'zoomOut';
    if (key === '0') return 'fit';
    return null;
  }
  if (editable) return null;
  switch (key) {
    case '+':
    case '=':
      return 'zoomIn';
    case '-':
    case '_':
      return 'zoomOut';
    case '0':
      return 'fit';
    case '1':
      return 'actualSize';
    case 'm':
    case 'M':
      return 'toggleMaximize';
    case 'ArrowLeft':
      return pannable ? 'panLeft' : null;
    case 'ArrowRight':
      return pannable ? 'panRight' : null;
    case 'ArrowUp':
      return pannable ? 'panUp' : null;
    case 'ArrowDown':
      return pannable ? 'panDown' : null;
    default:
      return null;
  }
}

/** Pan offset of an arrow command: the content moves opposite to the view. */
export function panDelta(command: FocusCommand, step: number): { dx: number; dy: number } | null {
  switch (command) {
    case 'panLeft':
      return { dx: step, dy: 0 };
    case 'panRight':
      return { dx: -step, dy: 0 };
    case 'panUp':
      return { dx: 0, dy: step };
    case 'panDown':
      return { dx: 0, dy: -step };
    default:
      return null;
  }
}

/**
 * Keyboard map of the focus pop-out stage. Esc is handled by the dialog
 * (stop a running side answer, then close).
 *
 * Pure module (its one import has its extension), unit-tested with Node
 * (`app/tests/focusKeys.test.ts`). The zoom chords come from the shortcut registry.
 */

import { IS_MAC_PLATFORM, matchesShortcut } from '../../lib/shortcuts.ts';

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
  /** A Mac (⌘ chords); defaults to this computer. */
  mac?: boolean;
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
    // The registry's zoom chords (⌘ alone on a Mac, Ctrl alone elsewhere).
    const chord = { key, code: '', ctrlKey: input.ctrlKey, metaKey: input.metaKey, shiftKey: input.shiftKey, altKey };
    const mac = input.mac ?? IS_MAC_PLATFORM;
    if (matchesShortcut(chord, 'textLarger', mac)) return 'zoomIn';
    if (matchesShortcut(chord, 'textSmaller', mac)) return 'zoomOut';
    if (matchesShortcut(chord, 'textReset', mac)) return 'fit';
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

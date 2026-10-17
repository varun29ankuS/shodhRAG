/**
 * Esc that belongs to a control inside a panel rather than to the panel: a find field
 * (`role="search"`) or an element that cancels itself on Esc (`data-esc-local`: an
 * editor, the PDF viewer while it is in snippet mode). The panel's own Esc handler
 * leaves such a key to it. No imports: unit-tested under Node.
 */

export const ESC_LOCAL_SELECTOR = '[role="search"], [data-esc-local]';

interface ClosestTarget {
  closest(selector: string): unknown;
}

interface Container<T> {
  contains(node: T | null): boolean;
}

/** Whether Esc on `target` is handled by a control inside `container`. */
export function escapeStaysLocal<T extends ClosestTarget>(target: T | null, container: Container<T> | null): boolean {
  if (!target || !container || !container.contains(target)) return false;
  return target.closest(ESC_LOCAL_SELECTOR) !== null;
}

/**
 * Whether Esc on `target` belongs to another dialog than `container`: the target is
 * outside it and inside some `[role="dialog"]` (the feedback dialog, the Instructions
 * popover, the palette, over the source preview). That dialog closes, not this one.
 */
export function insideOtherDialog<T extends ClosestTarget>(target: T | null, container: Container<T> | null): boolean {
  if (!target) return false;
  if (container && container.contains(target)) return false;
  return target.closest('[role="dialog"]') !== null;
}

/**
 * Whether a key in the PDF viewer leaves snippet mode: Esc while snipping (or drawing
 * a rectangle), also when a focus pop-out already marked it handled (the viewer is
 * `data-esc-local` while snipping), except in the find bar, which closes itself.
 */
export function leavesSnippetMode(input: { key: string; snipping: boolean; inFindBar: boolean }): boolean {
  return input.key === 'Escape' && input.snipping && !input.inFindBar;
}

export type InstructionsEscapeAction = 'ignore' | 'cancel-edit' | 'close';

export interface InstructionsEscapeInput {
  key: string;
  /** Something before marked the key handled. */
  handled: boolean;
  /** An instruction is being edited. */
  editing: boolean;
  /** The key is in another dialog on top (see `insideOtherDialog`). */
  inOtherDialog: boolean;
}

/**
 * What Esc does while the Instructions popover is open, wherever focus is: it cancels an
 * edit first, else closes the popover. Keys another dialog or handler owns are left alone.
 */
export function instructionsEscape(input: InstructionsEscapeInput): InstructionsEscapeAction {
  if (input.key !== 'Escape' || input.handled || input.inOtherDialog) return 'ignore';
  return input.editing ? 'cancel-edit' : 'close';
}

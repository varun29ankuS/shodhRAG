/**
 * Conversation dock layout off the Ask view.
 *
 * Pure module (no React, no DOM) so the rules are unit-tested directly with
 * Node (`app/tests/dockMode.test.ts`).
 *
 * Model
 * - The dock is a pill in the bottom-right corner, or, expanded, a chat that
 *   floats over the page in that corner. It never takes a column: the page
 *   keeps its full width on every view and at every window size.
 * - Each visit to a view starts minimised. Expanding (pill, Ctrl+J, or the
 *   assistant opening a view for the user to watch, as a Creator build does)
 *   lasts for that visit only. Pressing the page leaves it open; Esc in it, or
 *   Minimise, collapses it.
 */

/** What the dock renders as. */
export type DockLayout = 'pill' | 'overlay';

/**
 * Records of earlier dock versions (a global open state, then per-view side-panel
 * pins and widths). Nothing reads them now; they are removed on load.
 */
export const RETIRED_DOCK_STORAGE_KEYS: readonly string[] = ['shodh.conversationDock', 'shodh.conversationDock.v2'];

/** The form the dock takes: floating when open, a pill otherwise. */
export function dockLayout(open: boolean): DockLayout {
  return open ? 'overlay' : 'pill';
}

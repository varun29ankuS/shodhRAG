/** Top-level views reachable from the sidebar and command palette. */
export const VIEW_TABS = ['chat', 'documents', 'calendar', 'graph'] as const;

export type ViewTab = typeof VIEW_TABS[number];

export function isViewTab(value: unknown): value is ViewTab {
  return typeof value === 'string' && (VIEW_TABS as readonly string[]).includes(value);
}

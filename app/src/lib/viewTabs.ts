/** Top-level views reachable from the sidebar and command palette. */
export const VIEW_TABS = ['ask', 'library', 'calendar', 'settings'] as const;

export type ViewTab = typeof VIEW_TABS[number];

/** Visible label for each view (sidebar, title bar, command palette). */
export const VIEW_TAB_LABELS: Record<ViewTab, string> = {
  ask: 'Ask',
  library: 'Library',
  calendar: 'Calendar',
  settings: 'Settings',
};

/**
 * Other ids for a view: names used before the Ask/Library rename, and the
 * agent's view ids `tasks` (the calendar) and `activity` (the audit log,
 * under Settings here).
 */
const LEGACY_TAB_ALIASES: Readonly<Record<string, ViewTab>> = {
  chat: 'ask',
  documents: 'library',
  tasks: 'calendar',
  activity: 'settings',
  audit: 'settings',
};

export function isViewTab(value: unknown): value is ViewTab {
  return typeof value === 'string' && (VIEW_TABS as readonly string[]).includes(value);
}

/**
 * Resolve a tab id (current or legacy) to a navigable view.
 * Returns null for ids that are not navigable, including hidden views such as 'graph'.
 */
export function normalizeViewTab(value: unknown): ViewTab | null {
  if (isViewTab(value)) return value;
  if (typeof value === 'string' && Object.prototype.hasOwnProperty.call(LEGACY_TAB_ALIASES, value)) {
    return LEGACY_TAB_ALIASES[value];
  }
  return null;
}

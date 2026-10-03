/**
 * Top-level views reachable from the sidebar and command palette.
 *
 * Single source of view ids for the shell, the palette and agent navigation.
 * Pure module (no runtime imports) so it is unit-tested directly with Node
 * (`app/tests/viewTabs.test.ts`).
 */
export const VIEW_TABS = ['ask', 'library', 'tasks', 'activity', 'settings'] as const;

export type ViewTab = typeof VIEW_TABS[number];

/** Visible label for each view (sidebar, title bar, command palette). */
export const VIEW_TAB_LABELS: Record<ViewTab, string> = {
  ask: 'Ask',
  library: 'Library',
  tasks: 'Tasks',
  activity: 'Activity',
  settings: 'Settings',
};

/** One-line description of each view, used by the command palette. */
export const VIEW_TAB_DESCRIPTIONS: Record<ViewTab, string> = {
  ask: 'Ask questions about your sources',
  library: 'Folders and files Shodh can search',
  tasks: 'Tasks and events, as a list or a calendar',
  activity: 'Usage and the tamper-evident audit log',
  settings: 'Models, search and stored data',
};

/** Extra words the command palette matches for each view. */
export const VIEW_TAB_KEYWORDS: Record<ViewTab, string> = {
  ask: 'ask chat messages conversation question',
  library: 'library documents files sources folders index',
  tasks: 'tasks todo calendar events schedule agenda',
  activity: 'activity audit usage log history tools retrieved',
  settings: 'settings preferences models llm provider search data',
};

/**
 * Ids emitted by older callers and by the agent's `open_view` tool, whose
 * view enum (Rust, `harness/tools/navigate.rs`) still says 'calendar'.
 */
const VIEW_TAB_ALIASES: Readonly<Record<string, ViewTab>> = {
  chat: 'ask',
  documents: 'library',
  calendar: 'tasks',
  audit: 'activity',
};

export function isViewTab(value: unknown): value is ViewTab {
  return typeof value === 'string' && (VIEW_TABS as readonly string[]).includes(value);
}

/**
 * Resolve a tab id (current or alias) to a navigable view.
 * Returns null for ids that are not navigable, including hidden views such as 'graph'.
 */
export function normalizeViewTab(value: unknown): ViewTab | null {
  if (isViewTab(value)) return value;
  if (typeof value === 'string' && Object.prototype.hasOwnProperty.call(VIEW_TAB_ALIASES, value)) {
    return VIEW_TAB_ALIASES[value];
  }
  return null;
}

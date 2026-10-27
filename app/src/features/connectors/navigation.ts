/**
 * Where the Connectors view is: the gallery, or one connector's page at a tab. An Inbox
 * item of a connector opens its page (`link: { view: "connectors", target: { connector,
 * tab } }`, see `app/src-tauri/src/connectors/slack_notices.rs`), and the view remembers
 * the last page it showed for this session only (never saved).
 *
 * Pure apart from the module-level slots and one window event, so `node --test` loads it
 * (`app/tests/connectors.test.ts`).
 */

import type { ConnectorId } from './registry.ts';
import type { SlackTabId } from './slackModel.ts';
import type { TelegramTabId } from './telegramModel.ts';

/** The tabs of the Slack page, in order. */
export const SLACK_TABS: readonly SlackTabId[] = ['overview', 'answering', 'indexing', 'memories', 'settings'];

export const SLACK_TAB_LABELS: Record<SlackTabId, string> = {
  overview: 'Overview',
  answering: 'Answering',
  indexing: 'Indexing',
  memories: 'Memories',
  settings: 'Settings',
};

/** The tabs of the Telegram page, in order. */
export const TELEGRAM_TABS: readonly TelegramTabId[] = ['overview', 'people', 'settings'];

export const TELEGRAM_TAB_LABELS: Record<TelegramTabId, string> = {
  overview: 'Overview',
  people: 'People & groups',
  settings: 'Settings',
};

/** Any connector's tab. */
export type ConnectorTabId = SlackTabId | TelegramTabId;

/** A connector's page, at a tab (its first when `null`). */
export interface ConnectorPage {
  connector: ConnectorId;
  tab: ConnectorTabId | null;
}

const CONNECTOR_IDS: readonly ConnectorId[] = ['slack', 'telegram'];

/** The tabs of `connector`'s page. */
export function tabsOf(connector: ConnectorId): readonly ConnectorTabId[] {
  return connector === 'telegram' ? TELEGRAM_TABS : SLACK_TABS;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** The page an Inbox link's target names, or `null` for anything malformed or unknown. */
export function parseConnectorTarget(value: unknown): ConnectorPage | null {
  if (!isRecord(value)) return null;
  const connector = value.connector;
  if (typeof connector !== 'string' || !(CONNECTOR_IDS as readonly string[]).includes(connector)) return null;
  const id = connector as ConnectorId;
  const tabs = tabsOf(id) as readonly string[];
  const tab = typeof value.tab === 'string' && tabs.includes(value.tab) ? (value.tab as ConnectorTabId) : null;
  return { connector: id, tab };
}

/** The tab after (`+1`) or before (`-1`) `tab` among `tabs`, wrapping; Home and End go to the ends. */
export function moveTab<T extends string>(tab: T, key: string, tabs: readonly T[] = SLACK_TABS as readonly string[] as readonly T[]): T | null {
  const index = tabs.indexOf(tab);
  switch (key) {
    case 'ArrowRight':
      return tabs[(index + 1) % tabs.length];
    case 'ArrowLeft':
      return tabs[(index - 1 + tabs.length) % tabs.length];
    case 'Home':
      return tabs[0];
    case 'End':
      return tabs[tabs.length - 1];
    default:
      return null;
  }
}

/** Dispatched when a page is requested ([`requestConnectorPage`]). */
export const OPEN_CONNECTOR_PAGE = 'shodh:open-connector-page';

let requested: ConnectorPage | null = null;
let last: ConnectorPage | null = null;

/** Ask the Connectors view to show `page` (it takes it when it mounts, or at once). */
export function requestConnectorPage(page: ConnectorPage): void {
  requested = page;
  if (typeof window !== 'undefined') window.dispatchEvent(new CustomEvent(OPEN_CONNECTOR_PAGE));
}

/** The requested page, once. */
export function takeRequestedConnectorPage(): ConnectorPage | null {
  const page = requested;
  requested = null;
  return page;
}

/** The page the view showed last in this session (`null`: the gallery). */
export function lastConnectorPage(): ConnectorPage | null {
  return last;
}

export function rememberConnectorPage(page: ConnectorPage | null): void {
  last = page;
}

/**
 * Whether Escape pressed on `target` goes back to the gallery: not while typing, and not
 * inside a menu or a confirmation, which close themselves first.
 */
export function escapeGoesBack(target: { tagName?: string; isContentEditable?: boolean; closest?: (selector: string) => unknown } | null): boolean {
  if (!target) return true;
  const tag = (target.tagName ?? '').toUpperCase();
  if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || target.isContentEditable) return false;
  if (target.closest && target.closest('[role="menu"], [role="alertdialog"], [role="dialog"]')) return false;
  return true;
}

/**
 * Notices about the app's own data files (`store_notices.rs`): a file that
 * was damaged and read from its backup, settings that had to be reset, a
 * calendar that cannot be read. Pure, so it is unit-tested with Node
 * (`app/tests/storeNotices.test.ts`).
 */

/** Event the backend sends with one notice. */
export const STORE_NOTICE_EVENT = 'store-notice';

export type StoreNoticeKind = 'restored' | 'reset' | 'unreadable';

export interface StoreNotice {
  /** Stable id: one problem is shown once. */
  id: string;
  store: string;
  kind: StoreNoticeKind;
  title: string;
  message: string;
  /** The file the notice is about. */
  path: string;
}

const KINDS: readonly StoreNoticeKind[] = ['restored', 'reset', 'unreadable'];

/** A notice from the backend, or null when the payload is malformed. */
export function parseStoreNotice(value: unknown): StoreNotice | null {
  if (typeof value !== 'object' || value === null) return null;
  const v = value as Record<string, unknown>;
  const text = (key: string) => (typeof v[key] === 'string' ? (v[key] as string) : null);
  const id = text('id');
  const store = text('store');
  const title = text('title');
  const message = text('message');
  const path = text('path');
  const kind = KINDS.find(k => k === v.kind);
  if (!id || !store || !title || message === null || path === null || !kind) return null;
  return { id, store, kind, title, message, path };
}

/** The notices in `list` (a backend listing) not shown yet; marks them shown in `shown`. */
export function unseenNotices(list: unknown, shown: Set<string>): StoreNotice[] {
  if (!Array.isArray(list)) return [];
  const out: StoreNotice[] = [];
  for (const item of list) {
    const notice = parseStoreNotice(item);
    if (!notice || shown.has(notice.id)) continue;
    shown.add(notice.id);
    out.push(notice);
  }
  return out;
}

/** The toast's text: the backend's message, then where the file is. */
export function noticeDescription(notice: StoreNotice): string {
  const message = notice.message.trim();
  return notice.path.length > 0 && !message.includes(notice.path)
    ? `${message} File: ${notice.path}`
    : message;
}

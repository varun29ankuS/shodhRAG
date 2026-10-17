import { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { noticeDescription, STORE_NOTICE_EVENT, unseenNotices } from '../lib/storeNotices';
import type { StoreNotice } from '../lib/storeNotices';

function show(notice: StoreNotice): void {
  const options = {
    id: notice.id,
    description: noticeDescription(notice),
    // Stays until closed: the user decides when they have read it.
    duration: Infinity,
  };
  if (notice.kind === 'restored') toast.warning(notice.title, options);
  else toast.error(notice.title, options);
}

/**
 * Shows each notice about a damaged data file once: those the backend
 * reports from now on (`store-notice`), and those it reported before the
 * window listened (`store_notices`, read once listening has started, so
 * none falls between the two).
 */
export function StoreNoticeToasts() {
  useEffect(() => {
    const shown = new Set<string>();
    let disposed = false;
    let unlisten: (() => void) | null = null;
    const showNew = (list: unknown) => {
      if (disposed) return;
      for (const notice of unseenNotices(list, shown)) show(notice);
    };
    listen<unknown>(STORE_NOTICE_EVENT, event => showNew([event.payload]))
      .then(fn => {
        if (disposed) {
          fn();
          return;
        }
        unlisten = fn;
        return invoke<unknown>('store_notices').then(showNew);
      })
      .catch(err => console.error('Notices about damaged data files are not shown:', err));
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, []);
  return null;
}

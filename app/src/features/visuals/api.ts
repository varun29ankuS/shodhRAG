/**
 * Gallery commands (`visual_commands.rs`). Errors arrive as
 * `{ code, message }`; `toVisualError` normalises anything thrown.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import type { CaptureBatch, CaptureBlock } from './capture';
import type { VisualKind } from './extract';
import type { VisualDetail, VisualOrigin, VisualPage } from './model';

export type VisualErrorCode = 'not_found' | 'deleted' | 'invalid' | 'unavailable' | 'storage' | 'unknown';

export interface VisualFailure {
  code: VisualErrorCode;
  message: string;
}

const CODES: readonly VisualErrorCode[] = ['not_found', 'deleted', 'invalid', 'unavailable', 'storage'];

export function toVisualError(error: unknown): VisualFailure {
  if (typeof error === 'object' && error !== null) {
    const e = error as { code?: unknown; message?: unknown };
    if (typeof e.message === 'string') {
      const code = typeof e.code === 'string' && (CODES as readonly string[]).includes(e.code) ? (e.code as VisualErrorCode) : 'unknown';
      return { code, message: e.message };
    }
  }
  if (typeof error === 'string') return { code: 'unknown', message: error };
  return { code: 'unknown', message: error instanceof Error ? error.message : 'The gallery could not be reached.' };
}

export interface CaptureReport {
  created: string[];
  existing: string[];
  skipped: { index: number; reason: string }[];
}

export interface VisualListQuery {
  conversationId?: string | null;
  /** Only visuals of these conversations; an empty list matches nothing. */
  conversationIds?: string[] | null;
  kind?: VisualKind | null;
  text?: string | null;
  pinnedOnly?: boolean;
  limit?: number;
  offset?: number;
}

export const visualsApi = {
  capture: (origin: VisualOrigin, blocks: CaptureBlock[]) => invoke<CaptureReport>('visuals_capture', { origin, blocks }),
  backfillStatus: () => invoke<boolean>('visuals_backfill_status'),
  backfill: (batches: CaptureBatch[], finished: boolean) =>
    invoke<{ created: number; failed: number }>('visuals_backfill', { batches, finished }),
  list: (query: VisualListQuery) => invoke<VisualPage>('visuals_list', { query }),
  count: (conversationId: string) => invoke<number>('visuals_count', { conversationId }),
  get: (id: string, options: { version?: number | null; latest?: boolean } = {}) =>
    invoke<VisualDetail>('visuals_get', { id, version: options.version ?? null, latest: options.latest ?? false }),
  rename: (id: string, title: string) => invoke<VisualDetail>('visuals_rename', { id, title }),
  setPinned: (id: string, pinned: boolean) => invoke<VisualDetail>('visuals_set_pinned', { id, pinned }),
  setNote: (id: string, note: string) => invoke<VisualDetail>('visuals_set_note', { id, note }),
  addVersion: (baseId: string, source: string, params: Record<string, unknown>, instruction: string) =>
    invoke<VisualDetail>('visuals_add_version', { baseId, source, params, instruction }),
  remove: (id: string) => invoke<string>('visuals_delete', { id }),
  restore: (id: string) => invoke<string>('visuals_restore', { id }),
};

/** `visuals-changed`: visuals of a conversation (null: of several) changed. */
export function onVisualsChanged(handler: (conversationId: string | null) => void): Promise<UnlistenFn> {
  return listen<{ conversationId?: unknown }>('visuals-changed', event => {
    const id = event.payload?.conversationId;
    handler(typeof id === 'string' ? id : null);
  });
}

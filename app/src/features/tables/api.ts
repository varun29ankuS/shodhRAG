/**
 * Evidence table commands (`app/src-tauri/src/table_commands.rs`). Errors arrive as
 * `{ code, message }`; `tableError` keeps the code as sent, because the Tables tab
 * words `model_refused`, `model_unavailable`, `fill_running`, `hand_fill_running` and
 * `diagnostics_off` each its own way. Shapes are in `./types`.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type {
  Cell,
  Column,
  ColumnSpec,
  EstimateView,
  EvidenceTable,
  FillCancel,
  FillStart,
  FillStatus,
  HandFill,
  PaperCapture,
  TableFailure,
  TableRow,
  TablesSettings,
  TableView,
} from './types';
import { TABLE_CHANGED_EVENT } from './types';

export const tablesApi = {
  list: (workspaceId: string) => invoke<EvidenceTable[]>('tables_list', { workspaceId }),
  create: (workspaceId: string, name: string) => invoke<EvidenceTable>('table_create', { workspaceId, name }),
  rename: (tableId: string, name: string) => invoke<EvidenceTable>('table_rename', { tableId, name }),
  remove: (tableId: string) => invoke<boolean>('table_delete', { tableId }),
  get: (tableId: string) => invoke<TableView>('table_get', { tableId }),

  addColumn: (tableId: string, spec: ColumnSpec) => invoke<Column>('column_add', { tableId, spec }),
  updateColumn: (tableId: string, columnId: string, spec: ColumnSpec) => invoke<Column>('column_update', { tableId, columnId, spec }),
  deleteColumn: (tableId: string, columnId: string) => invoke<boolean>('column_delete', { tableId, columnId }),
  moveColumn: (tableId: string, columnId: string, position: number) => invoke<Column[]>('column_move', { tableId, columnId, position }),

  addRows: (tableId: string, paths: string[]) => invoke<TableRow[]>('rows_add', { tableId, paths }),
  removeRow: (tableId: string, rowId: string) => invoke<boolean>('row_remove', { tableId, rowId }),

  editCell: (tableId: string, rowId: string, columnId: string, value: string) =>
    invoke<Cell>('cell_edit', { tableId, rowId, columnId, value }),
  clearCell: (tableId: string, rowId: string, columnId: string) => invoke<boolean>('cell_clear', { tableId, rowId, columnId }),
  refillCell: (tableId: string, rowId: string, columnId: string) => invoke<HandFill>('cell_refill', { tableId, rowId, columnId }),

  estimate: (tableId: string) => invoke<EstimateView>('fill_estimate', { tableId }),
  start: (tableId: string, plannedCalls: number) => invoke<FillStart>('fill_start', { tableId, plannedCalls }),
  cancel: (tableId: string) => invoke<FillCancel>('fill_cancel', { tableId }),
  status: (tableId: string) => invoke<FillStatus>('fill_status', { tableId }),

  fillPaper: (tableId: string, rowId: string) => invoke<PaperCapture>('table_fill_paper', { tableId, rowId }),
  exportCaptures: (tableId: string) => invoke<unknown[]>('table_captures_export', { tableId }),

  settings: () => invoke<TablesSettings>('tables_settings_get'),
  /** The cap is sent every time; `diagnostics` only when it changes. */
  setSettings: (dailyCallCap: number, diagnostics?: boolean) =>
    invoke<TablesSettings>('tables_settings_set', { dailyCallCap, diagnostics: diagnostics ?? null }),
};

/** A command error with its code as sent (`unknown` for anything without one). */
export function tableError(error: unknown): TableFailure {
  if (typeof error === 'object' && error !== null) {
    const e = error as { code?: unknown; message?: unknown };
    if (typeof e.message === 'string') return { code: typeof e.code === 'string' ? e.code : 'unknown', message: e.message };
  }
  if (typeof error === 'string') return { code: 'unknown', message: error };
  return { code: 'unknown', message: error instanceof Error ? error.message : 'Evidence tables could not be reached.' };
}

/** Listen for `table_changed` (`tableId` of the table that changed). Returns the unsubscribe function. */
export function onTableChanged(handler: (tableId: string | null) => void): () => void {
  let disposed = false;
  let unlisten: (() => void) | null = null;
  listen<{ tableId?: unknown }>(TABLE_CHANGED_EVENT, event => {
    const id = event.payload?.tableId;
    handler(typeof id === 'string' ? id : null);
  })
    .then(fn => {
      if (disposed) fn();
      else unlisten = fn;
    })
    .catch(err => console.error(`Failed to listen for ${TABLE_CHANGED_EVENT}:`, err));
  return () => {
    disposed = true;
    if (unlisten) unlisten();
  };
}

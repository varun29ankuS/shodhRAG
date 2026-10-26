/**
 * The app's diagram ledger (`mermaidRetry.ts`), kept in localStorage: the
 * attempts and versions of every diagram, shared by the answer, the
 * gallery's thumbnails and exports, and the print window (which reads it
 * when it opens).
 */

import { useSyncExternalStore } from 'react';
import { chosenSource, createRepairLedger, sourceKey } from './mermaidRetry';
import type { DiagramRecord, RepairLedger } from './mermaidRetry';

let ledger: RepairLedger | null = null;

/** The app-wide ledger, created on first use. */
export function diagramLedger(): RepairLedger {
  if (!ledger) {
    let storage: Storage | null = null;
    try {
      storage = typeof window !== 'undefined' ? window.localStorage : null;
    } catch {
      storage = null;
    }
    ledger = createRepairLedger(storage);
  }
  return ledger;
}

/** The record of the diagram keyed `key`, re-rendering when it changes. */
export function useDiagramRecord(key: string): DiagramRecord {
  const store = diagramLedger();
  return useSyncExternalStore(store.subscribe, () => store.get(key), () => store.get(key));
}

/** The version of the diagram written as `written` that the reader chose (what is shown, printed and exported). */
export function chosenDiagramSource(written: string): string {
  const source = written.trim();
  return chosenSource(source, diagramLedger().get(sourceKey(source)));
}

/** As `chosenDiagramSource`, re-rendering when the choice changes. */
export function useChosenDiagramSource(written: string): string {
  const source = written.trim();
  const record = useDiagramRecord(sourceKey(source));
  return chosenSource(source, record);
}

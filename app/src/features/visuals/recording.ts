/**
 * Recording answers' visuals in the gallery: after an answer completes, and
 * once for conversations saved before the gallery existed.
 */

import { invoke } from '@tauri-apps/api/core';
import type { Conversation } from '../../hooks/useConversations';
import { toVisualError, visualsApi } from './api';
import { backfillBatches, captureBatch, chunks } from './capture';
import type { VisualOrigin } from './model';

/**
 * Record the visual blocks of a completed answer. Never throws and never
 * blocks the caller: a failure is logged (the answer itself is saved as
 * usual). Recording an answer again adds nothing (captures are idempotent).
 */
export function recordAnswerVisuals(origin: VisualOrigin, answer: string): void {
  const batch = captureBatch(origin, answer);
  if (!batch) return;
  visualsApi.capture(batch.origin, batch.blocks).catch(error => {
    console.warn('Recording the answer’s visuals failed:', toVisualError(error).message);
  });
}

let backfill: Promise<void> | null = null;

/**
 * Record the visuals of every saved conversation, once. Reads the saved
 * conversations (not the ones in memory, which may still be loading).
 * Concurrent callers share one run; a failed run is retried on the next call.
 */
export function backfillOnce(): Promise<void> {
  if (backfill) return backfill;
  const run = (async () => {
    if (await visualsApi.backfillStatus()) return;
    const saved = await invoke<Conversation[]>('load_conversations');
    const parts = chunks(backfillBatches(saved));
    if (parts.length === 0) {
      await visualsApi.backfill([], true);
      return;
    }
    for (let i = 0; i < parts.length; i++) {
      await visualsApi.backfill(parts[i], i === parts.length - 1);
    }
  })();
  backfill = run.catch(error => {
    backfill = null;
    throw error;
  });
  return backfill;
}

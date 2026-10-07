/**
 * Gallery actions shared by the cards and the focus pop-out.
 */

import { toast } from 'sonner';
import { notify } from '../../lib/notify';
import { UNDO_WINDOW_MS } from '../../lib/undoQueue';
import { toVisualError, visualsApi } from './api';
import { exportVisual, FORMAT_LABEL } from './exportVisual';
import type { ExportFormat } from './exportVisual';
import type { VisualRecord } from './model';
import { requestReveal } from './reveal';

/**
 * Delete the visual (every version) at once; the notice offers Undo for the app's
 * undo window. A deleted visual is only marked deleted, so Undo restores it exactly.
 * True when deleted.
 */
export async function deleteWithUndo(record: Pick<VisualRecord, 'id' | 'title' | 'kind'>): Promise<boolean> {
  try {
    const root = await visualsApi.remove(record.id);
    toast.success(`Deleted “${record.title}”`, {
      duration: UNDO_WINDOW_MS,
      action: {
        label: 'Undo',
        onClick: () => {
          visualsApi.restore(root).catch(error =>
            notify.error('The visual was not restored', { description: toVisualError(error).message }));
        },
      },
    });
    return true;
  } catch (error) {
    notify.error('The visual was not deleted', { description: toVisualError(error).message });
    return false;
  }
}

/** Export to a file the user picks; reports the outcome. */
export async function runExport(
  record: Pick<VisualRecord, 'kind' | 'title' | 'source' | 'params' | 'version'>,
  format: ExportFormat,
  container: Element | null,
  dark: boolean,
): Promise<void> {
  try {
    const path = await exportVisual({ record, format, container, dark });
    if (path) notify.success(`Exported ${FORMAT_LABEL[format]}`, { description: path });
  } catch (error) {
    notify.error('The visual was not exported', { description: error instanceof Error ? error.message : String(error) });
  }
}

/**
 * Show the answer a visual came from: its conversation in Ask, scrolled to
 * the answer. `switchConversation` is the chat session's.
 */
export function goToMessage(
  record: Pick<VisualRecord, 'conversationId' | 'messageId'>,
  switchConversation: (id: string) => void,
): boolean {
  if (!record.messageId) return false;
  switchConversation(record.conversationId);
  window.dispatchEvent(new CustomEvent('switchTab', { detail: 'ask' }));
  requestReveal(record.conversationId, record.messageId);
  return true;
}

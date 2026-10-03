/**
 * Gallery actions shared by the cards and the focus pop-out.
 */

import { ask } from '@tauri-apps/plugin-dialog';
import { toast } from 'sonner';
import { notify } from '../../lib/notify';
import { toVisualError, visualsApi } from './api';
import { KIND_NOUN } from './extract';
import { exportVisual, FORMAT_LABEL } from './exportVisual';
import type { ExportFormat } from './exportVisual';
import type { VisualRecord } from './model';
import { requestReveal } from './reveal';

/** Ask, then delete the visual (every version), with Undo in the notice. True when deleted. */
export async function confirmDelete(record: Pick<VisualRecord, 'id' | 'title' | 'kind'>): Promise<boolean> {
  const noun = KIND_NOUN[record.kind].toLowerCase();
  const confirmed = await ask(
    `Delete the ${noun} “${record.title}” and all of its versions from the gallery? The answer it came from keeps it.`,
    { title: `Delete ${noun}`, kind: 'warning', okLabel: 'Delete', cancelLabel: 'Keep' },
  );
  if (!confirmed) return false;
  try {
    const root = await visualsApi.remove(record.id);
    toast.success(`Deleted “${record.title}”`, {
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

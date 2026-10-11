/**
 * Export → PDF: the user picks where to save, the app prints the document
 * with its own WebView (`export_pdf` in pdf_export.rs). Where the app cannot
 * write PDFs itself (macOS, Linux) the system print dialog opens instead.
 */

import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import { toast } from 'sonner';
import { notify } from '../../lib/notify';
import { printFileName } from './printModel';
import { writesPdfFiles } from './printPlatform';
import type { PrintDocument } from './printModel';

interface PdfExportOutcome {
  path: string | null;
  bytes: number | null;
  written: boolean;
}

let running = false;

/** Export `document` as PDF; reports the outcome as a notification. */
export async function exportPdf(document: PrintDocument): Promise<void> {
  if (running) {
    notify.info('A PDF is already being prepared', { description: 'Try again when it is saved.' });
    return;
  }
  running = true;
  try {
    let path: string | null = null;
    if (writesPdfFiles()) {
      path = await save({
        title: 'Export as PDF',
        defaultPath: printFileName(document.title),
        filters: [{ name: 'PDF document', extensions: ['pdf'] }],
      });
      if (!path) return;
    }
    const pending = toast.loading('Preparing the PDF…');
    try {
      const outcome = await invoke<PdfExportOutcome>('export_pdf', { document, path });
      toast.dismiss(pending);
      if (outcome.written && outcome.path) {
        notify.success('Exported PDF', { description: outcome.path });
      } else {
        notify.info('Print dialog opened', { description: 'Choose “Save as PDF” as the printer to save the file.' });
      }
    } catch (error) {
      toast.dismiss(pending);
      throw error;
    }
  } catch (error) {
    notify.error('The PDF was not exported', { description: error instanceof Error ? error.message : String(error) });
  } finally {
    running = false;
  }
}

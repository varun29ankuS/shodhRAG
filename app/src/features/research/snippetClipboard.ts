/**
 * Copying snippets: the image as PNG and the text. The image goes through
 * the web clipboard (`ClipboardItem`, supported by the app's WebView); when
 * that is unavailable or refused it falls back to the clipboard plugin.
 */

import { writeImage } from '@tauri-apps/plugin-clipboard-manager';
import { base64ToBytes } from './api';

export async function copyPngImage(png: string): Promise<void> {
  const bytes = base64ToBytes(png);
  const canUseWeb = typeof ClipboardItem !== 'undefined' && typeof navigator !== 'undefined' && Boolean(navigator.clipboard?.write);
  if (canUseWeb) {
    try {
      const blob = new Blob([bytes as BlobPart], { type: 'image/png' });
      await navigator.clipboard.write([new ClipboardItem({ 'image/png': blob })]);
      return;
    } catch {
      // Fall through to the plugin.
    }
  }
  await writeImage(bytes);
}

export async function copyText(text: string): Promise<void> {
  await navigator.clipboard.writeText(text);
}

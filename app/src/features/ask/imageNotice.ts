/**
 * Images added to a chat (pasted into the Ask composer, dropped on the window, or
 * picked with the image button) and the text read from them. They are shown as
 * notices (`role: 'system'`): never an answer of the assistant's, so they are not
 * replayed to the model as one. Type imports only: unit-tested under Node.
 */
import type { ChatMessage } from './types';

/** What a paste into the composer holds, decided while the paste event runs. */
export type PasteDecision =
  /** Text (also text with a bitmap beside it, as Excel, Word and browsers copy): a normal paste. */
  | { kind: 'text' }
  /** An image file on the clipboard (`clipboardData.files[index]`). */
  | { kind: 'file'; index: number }
  /** Nothing the webview exposes: the system clipboard may still hold a bitmap. */
  | { kind: 'clipboard' };

/**
 * How to handle a paste with `text` and files of `fileTypes` (MIME types). Mirrors
 * `read_clipboard_image`: a clipboard that holds text is a text paste.
 */
export function pasteDecision(text: string, fileTypes: readonly string[]): PasteDecision {
  if (text.trim().length > 0) return { kind: 'text' };
  const index = fileTypes.findIndex(type => type.startsWith('image/'));
  if (index >= 0) return { kind: 'file', index };
  return fileTypes.length === 0 ? { kind: 'clipboard' } : { kind: 'text' };
}

/** `image_upload_commands.rs::ImageProcessResult` (camelCase). */
export interface ImageText {
  text: string;
  wordCount: number;
  confidence: number;
  /** A data URI of the image, for display ('' when none). */
  imageData: string;
}

function num(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0;
}

/** The OCR result of `process_image_from_*`; missing fields read as empty. */
export function readImageText(raw: unknown): ImageText {
  const value = typeof raw === 'object' && raw !== null ? (raw as Record<string, unknown>) : {};
  const text = typeof value.extractedText === 'string' ? value.extractedText : '';
  return {
    text,
    wordCount: num(value.wordCount),
    confidence: num(value.confidence),
    imageData: typeof value.imageData === 'string' ? value.imageData : '',
  };
}

export type ImageOrigin = 'pasted' | 'dropped' | 'added';

const HEADING: Record<ImageOrigin, string> = {
  pasted: 'Image pasted',
  dropped: 'Image dropped',
  added: 'Image added',
};

/** The notice of an image and the text read from it. */
export function imageNotice(id: string, timestamp: string, origin: ImageOrigin, result: ImageText): ChatMessage {
  const found = result.text.trim().length > 0 && result.wordCount > 0;
  const content = found
    ? `**${HEADING[origin]}.** Text read from it (${result.wordCount} ${result.wordCount === 1 ? 'word' : 'words'}, ${Math.round(result.confidence * 100)}% confidence):\n\n${result.text}`
    : `**${HEADING[origin]}.** No text was found in it.`;
  return {
    id,
    role: 'system',
    content,
    timestamp,
    ...(result.imageData ? { image: result.imageData } : {}),
  };
}

/** A notice that adding files or an image failed. */
export function failureNotice(id: string, timestamp: string, what: string, error: string): ChatMessage {
  return { id, role: 'system', content: `**${what}.** ${error}`, timestamp };
}

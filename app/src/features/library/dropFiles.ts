/**
 * What happens to each path dropped on the window: images have their text read
 * (OCR) into the chat; Word and PowerPoint files saved in the old binary formats
 * (.doc, .ppt), which the indexer cannot read, are named with what to do; every
 * other path (documents, folders, any file) is added to the Library, where a type
 * the indexer does not support fails with its reason. The drop overlay promises
 * "PDF, Word, Excel, PowerPoint, text, Markdown or images". No imports:
 * unit-tested under Node.
 */

/** Images whose text is read when dropped (`process_image_from_file`). */
export const DROP_IMAGE_EXTENSIONS: readonly string[] = ['png', 'jpg', 'jpeg', 'gif', 'bmp', 'webp'];

/**
 * Documents the indexer reads (a subset of `shodh_rag::indexing::SUPPORTED_FILE_EXTENSIONS`):
 * the formats a chat upload takes.
 */
export const DROP_DOCUMENT_EXTENSIONS: readonly string[] = [
  'pdf', 'docx', 'pptx', 'xlsx', 'xls', 'csv', 'txt', 'md', 'html', 'htm', 'json',
];

/** Old binary Office formats and the format to save them as. */
export const LEGACY_OFFICE_FORMATS: Readonly<Record<string, string>> = { doc: 'docx', ppt: 'pptx' };

/** The last part of a Windows or POSIX path. */
function baseName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** The lowercase extension of a path or file name, without the dot ('' when none). */
export function extensionOf(path: string): string {
  const name = baseName(path);
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
}

export function isDroppedImage(path: string): boolean {
  return DROP_IMAGE_EXTENSIONS.includes(extensionOf(path));
}

export function isIndexableDocument(path: string): boolean {
  return DROP_DOCUMENT_EXTENSIONS.includes(extensionOf(path));
}

/** The format to save `path` as when it is an old binary Office file, else null. */
export function legacyOfficeTarget(path: string): string | null {
  return LEGACY_OFFICE_FORMATS[extensionOf(path)] ?? null;
}

export interface DroppedPaths {
  images: string[];
  /** Old .doc/.ppt files: not indexed, the person is told to save them in the newer format. */
  legacy: string[];
  /** Documents, folders and other files, in drop order: added to the Library. */
  library: string[];
}

export function sortDroppedPaths(paths: readonly string[]): DroppedPaths {
  const sorted: DroppedPaths = { images: [], legacy: [], library: [] };
  for (const path of paths) {
    if (isDroppedImage(path)) sorted.images.push(path);
    else if (legacyOfficeTarget(path)) sorted.legacy.push(path);
    else sorted.library.push(path);
  }
  return sorted;
}

/** What to tell the person about dropped .doc/.ppt files. */
export function legacyOfficeNotice(paths: readonly string[]): string {
  const names = paths.map(baseName);
  const targets = [...new Set(paths.map(p => `.${legacyOfficeTarget(p)}`))].join(' or ');
  const list = names.join(', ');
  return `${list} ${names.length === 1 ? 'is' : 'are'} in an older Office format Shodh cannot read. Save ${names.length === 1 ? 'it' : 'them'} as ${targets} and drop ${names.length === 1 ? 'it' : 'them'} again.`;
}

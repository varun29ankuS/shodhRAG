/**
 * "Ask about this file" (the Library, a paper's page, a figure): the composer gets a
 * question that names the document by its title only, and the file itself goes with the
 * next question as a separate reference, the answer's search limit (`agent_send`'s
 * `scope.sourceFiles`). The file's local path never enters the question's text, so it is
 * never shown in the conversation, its title, an export or a print, and never reaches a
 * connector the conversation is continued in.
 *
 * The reference waits per conversation (like the draft) as a removable chip, and the user
 * message keeps it (metadata `aboutFiles`) so Retry asks about the same file again. Kept
 * for the session only. No imports: unit-tested under Node.
 */

/** A file a question is about. */
export interface FileRef {
  /** The indexed path: what limits the answer's search. Never shown. */
  path: string;
  /** What the user sees: the file's or the paper's title. */
  name: string;
}

/**
 * A request to start the Ask composer with `text` (e.g. "Ask about this file"), and the
 * file its question is about. `seq` tells repeated requests apart.
 */
export interface AskDraftRequest {
  text: string;
  seq: number;
  file?: FileRef;
}

/** Metadata key of the files a user message was limited to. */
export const ABOUT_FILES_METADATA_KEY = 'aboutFiles';

/** Most files one answer may be limited to (`MAX_SCOPE_ITEMS` in agent_session_commands.rs). */
const MAX_ABOUT_FILES = 50;

/** The draft the composer starts with: the document's title, never its path. */
export function fileQuestionDraft(name: string): string {
  return `About ${displayName(name)}: `;
}

/** A title fit for the question: one line, no surrounding space. */
function displayName(name: string): string {
  return name.replace(/\s+/g, ' ').trim();
}

/** The last segment of a path (either separator), for a reference stored without its name. */
export function fileNameOfPath(path: string): string {
  const parts = path.split(/[\\/]/).filter(p => p.length > 0);
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

/** Paths a question is limited to: trimmed, non-empty, unique, within the backend's limit. */
export function cleanFiles(files: readonly unknown[] | null | undefined): string[] {
  if (!files) return [];
  const out: string[] = [];
  for (const file of files) {
    if (typeof file !== 'string') continue;
    const path = file.trim();
    if (path.length === 0 || out.includes(path)) continue;
    out.push(path);
    if (out.length === MAX_ABOUT_FILES) break;
  }
  return out;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** The files a stored user message was limited to. */
export function readAboutFiles(metadata: unknown): string[] {
  if (!isRecord(metadata)) return [];
  const value = metadata[ABOUT_FILES_METADATA_KEY];
  return Array.isArray(value) ? cleanFiles(value) : [];
}

/** Metadata with the files set (removed when there are none), keeping every other key. */
export function metadataWithAboutFiles(metadata: unknown, files: readonly string[]): Record<string, unknown> | undefined {
  const base: Record<string, unknown> = isRecord(metadata) ? { ...metadata } : {};
  delete base[ABOUT_FILES_METADATA_KEY];
  const clean = cleanFiles(files);
  if (clean.length > 0) base[ABOUT_FILES_METADATA_KEY] = clean;
  return Object.keys(base).length > 0 ? base : undefined;
}

/**
 * Send options limited to `files` when there are any (a question about a file, or its
 * retry); unchanged otherwise. Generic so the caller's options type is kept.
 */
export function limitedToFiles<T extends { sourceFiles?: string[] }>(options: T, files: readonly string[] | null | undefined): T {
  const clean = cleanFiles(files);
  return clean.length > 0 ? { ...options, sourceFiles: clean } : options;
}

type Listener = () => void;

const NO_CONVERSATION = '';

/** The file each conversation's next question is about. */
export class FileRefStore {
  private readonly refs = new Map<string, FileRef>();
  private readonly listeners = new Set<Listener>();

  /** The file the next question of `conversationId` is about, if any (the same object until it changes). */
  get(conversationId: string | null): FileRef | null {
    return this.refs.get(conversationId ?? NO_CONVERSATION) ?? null;
  }

  /** Make the next question of `conversationId` about `ref`; null forgets it. A blank path is no file. */
  set(conversationId: string | null, ref: FileRef | null): void {
    const key = conversationId ?? NO_CONVERSATION;
    const path = ref?.path.trim() ?? '';
    const next = ref && path ? { path, name: displayName(ref.name) || fileNameOfPath(path) } : null;
    const current = this.refs.get(key) ?? null;
    if (current === null && next === null) return;
    if (current && next && current.path === next.path && current.name === next.name) return;
    if (next) this.refs.set(key, next);
    else this.refs.delete(key);
    for (const listener of [...this.listeners]) listener();
  }

  /**
   * Forget the file a question was sent with (read with `get` before sending); a file
   * chosen while it was being sent stays for the next one.
   */
  consume(conversationId: string | null, sent: FileRef | null): void {
    if (sent && this.get(conversationId) === sent) this.set(conversationId, null);
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }
}

/** The files the Ask composer's next questions are about. */
export const composerFiles = new FileRefStore();

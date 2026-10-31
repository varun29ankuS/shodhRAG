/**
 * "Ask about this file" (the Library, a paper's page, a figure): the composer gets a
 * question that names the document by its title only, and the file itself goes with the
 * next question as a separate reference, the answer's search limit (`agent_send`'s
 * `scope.sourceFiles`). The file's local path never enters the question's text, so it is
 * never shown in the conversation, its title, an export or a print, and never reaches a
 * connector the conversation is continued in.
 *
 * The reference waits per conversation (like the draft) as a removable chip, and the user
 * message keeps it (metadata `aboutFiles`) so Retry asks about the same file again. Once
 * a question about it is sent, the file stays the chat's search limit: follow-ups are about
 * it too, and the conversation keeps it (`aboutFile`) until the user removes the chip.
 * No imports: unit-tested under Node.
 */

/** A file a question is about. */
export interface FileRef {
  /** The indexed path: what limits the answer's search. Never shown. */
  path: string;
  /** What the user sees: the file's or the paper's title. */
  name: string;
  /**
   * The text the composer was given with this file ("About <title>: "), so asking about
   * another file replaces it instead of leaving the old question in front of the new file.
   */
  prefix?: string;
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

/** "Ask about this paper": the paper's title in the text, its PDF as the reference. */
export function paperQuestion(title: string, filePath: string | null | undefined): { text: string; file?: FileRef } {
  const name = displayName(title);
  const text = `About "${name}": `;
  const path = filePath?.trim();
  return path ? { text, file: { path, name } } : { text };
}

/** "Ask about it" on a figure: the figure and the paper's title in the text, the PDF as the reference. */
export function figureQuestion(figure: string, paperTitle: string, filePath: string): { text: string; file?: FileRef } {
  const name = displayName(paperTitle);
  const text = `Explain ${displayName(figure)} of "${name}": `;
  const path = filePath.trim();
  return path ? { text, file: { path, name } } : { text };
}

/**
 * The draft after asking about another file: when it still starts with the text the
 * previous file was asked about with, that text is replaced, so the question and the chip
 * name the same file. Null when the draft does not (the caller appends instead).
 */
export function replaceFilePrefix(draft: string, previous: FileRef | null, text: string): string | null {
  const prefix = previous?.prefix;
  if (!prefix || !draft.startsWith(prefix)) return null;
  return text + draft.slice(prefix.length);
}

/** A title fit for the question: one line, no surrounding space. */
function displayName(name: string): string {
  return name.replace(/\s+/g, ' ').trim();
}

/** The file a conversation keeps as its search limit, as saved with it. */
export interface ConversationFile {
  path: string;
  name: string;
}

/** A conversation's saved file limit, if it holds a usable one. */
export function readConversationFile(value: unknown): FileRef | null {
  if (!isRecord(value) || typeof value.path !== 'string') return null;
  const path = value.path.trim();
  if (!path) return null;
  const name = typeof value.name === 'string' && value.name.trim() ? displayName(value.name) : fileNameOfPath(path);
  return { path, name };
}

/**
 * What a chat keeps after a question about `sent` went out: the same file, without the
 * composer text it came with (that text was sent).
 */
export function keptFile(sent: FileRef): ConversationFile {
  return { path: sent.path.trim(), name: displayName(sent.name) || fileNameOfPath(sent.path) };
}

/** Whether a conversation's saved limit already is `file`. */
export function sameConversationFile(saved: unknown, file: ConversationFile | null): boolean {
  const current = readConversationFile(saved);
  if (!current || !file) return current === null && file === null;
  return current.path === file.path && current.name === file.name;
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
 * retry): only those files are searched, so the Library's included sources are dropped
 * (the backend searches the union of sources and files). Unchanged without files.
 * Generic so the caller's options type is kept.
 */
export function limitedToFiles<T extends { sourceIds?: string[]; sourceFiles?: string[] }>(
  options: T,
  files: readonly string[] | null | undefined,
): T {
  const clean = cleanFiles(files);
  return clean.length > 0 ? { ...options, sourceIds: [], sourceFiles: clean } : options;
}

/**
 * The files of `paths` the index no longer holds (removed, or renamed since the question
 * was asked about them), checked with `isIndexed`. A check that fails counts the file as
 * present: the answer's search then says plainly what it could not find.
 */
export async function missingFiles(
  paths: readonly string[],
  isIndexed: (path: string) => Promise<boolean>,
): Promise<string[]> {
  const clean = cleanFiles(paths);
  const indexed = await Promise.all(clean.map(path => isIndexed(path).catch(() => true)));
  return clean.filter((_, i) => !indexed[i]);
}

/** What the user is told when a file a question is about is gone; names only, never paths. */
export function missingFileNotice(paths: readonly string[], retry: boolean): { title: string; description: string } {
  const names = cleanFiles(paths).map(fileNameOfPath);
  const one = names.length === 1;
  const title = one ? 'This file is no longer in your library' : 'These files are no longer in your library';
  const which = names.join(', ');
  const was = one ? 'was' : 'were';
  const it = one ? 'it' : 'them';
  return {
    title,
    description: retry
      ? `${which} ${was} removed or renamed, so this question cannot be asked about ${it} again. Ask a new question instead.`
      : `${which} ${was} removed or renamed. Ask without ${it}, or add ${it} to the Library again.`,
  };
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
    const next: FileRef | null = ref && path
      ? { path, name: displayName(ref.name) || fileNameOfPath(path), ...(ref.prefix ? { prefix: ref.prefix } : {}) }
      : null;
    const current = this.refs.get(key) ?? null;
    if (current === null && next === null) return;
    if (current && next && current.path === next.path && current.name === next.name && current.prefix === next.prefix) return;
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

  /**
   * Give a file chosen while no conversation was open (at startup, before the
   * conversations load) to `conversationId`, unless it already has one.
   */
  adoptUnassigned(conversationId: string): void {
    const waiting = this.refs.get(NO_CONVERSATION);
    if (!waiting || conversationId === NO_CONVERSATION) return;
    this.refs.delete(NO_CONVERSATION);
    if (!this.refs.has(conversationId)) this.refs.set(conversationId, waiting);
    for (const listener of [...this.listeners]) listener();
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

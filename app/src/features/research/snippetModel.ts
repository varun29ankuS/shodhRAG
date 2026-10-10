/**
 * Snippets as the UI uses them: the focus target of a snippet, the context
 * block "Add to chat" puts in the composer, labels, validation of records,
 * and the drag payload. Pure module, unit-tested with Node
 * (`app/tests/snippetModel.test.ts`).
 */

import type { FocusTarget } from '../focus/focusTypes.ts';
import { isSnippetRect } from './snippetGeometry.ts';
import type { Snippet, SnippetKind } from './types.ts';
import { SNIPPET_KINDS } from './types.ts';

/** Characters of a snippet's text kept in its focus target. */
export const MAX_SNIPPET_TARGET_TEXT = 8_000;
/** Characters of a snippet's text put into the composer. */
export const MAX_SNIPPET_CHAT_TEXT = 3_000;
/** MIME type of a dragged snippet (the composer accepts it). */
export const SNIPPET_DRAG_TYPE = 'application/x-shodh-snippet';

const LABEL_CHARS = 60;

function short(text: string, max = LABEL_CHARS): string {
  const flat = text.replace(/\s+/g, ' ').trim();
  const chars = Array.from(flat);
  return chars.length > max ? `${chars.slice(0, max - 1).join('')}…` : flat;
}

function cap(text: string, max: number): string {
  const chars = Array.from(text);
  return chars.length > max ? chars.slice(0, max).join('') : text;
}

export const KIND_LABEL: Record<SnippetKind, string> = {
  figure: 'Figure',
  table: 'Table',
  equation: 'Equation',
  passage: 'Passage',
};

export function isSnippetKind(value: unknown): value is SnippetKind {
  return typeof value === 'string' && (SNIPPET_KINDS as readonly string[]).includes(value);
}

/** The base name of a path (either separator). */
export function fileNameOf(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** What a snippet is called: its title, else its kind and place. */
export function snippetLabel(s: Pick<Snippet, 'title' | 'kind' | 'fileName' | 'page'>): string {
  const title = s.title.trim();
  if (title) return short(title);
  return `${KIND_LABEL[s.kind] ?? 'Snippet'} · ${short(s.fileName, 40)} p.${s.page}`;
}

/** "paper.pdf, page 4". */
export function snippetSource(s: Pick<Snippet, 'fileName' | 'filePath' | 'page'>): string {
  return `${s.fileName || fileNameOf(s.filePath)}, page ${s.page}`;
}

/** The focus target of a snippet: its id and text, never its image. */
export function snippetTarget(s: Snippet): FocusTarget {
  return {
    kind: 'snippet',
    label: snippetLabel(s),
    snippetId: s.id,
    filePath: s.filePath,
    fileName: s.fileName || fileNameOf(s.filePath),
    page: s.page,
    rect: { ...s.rect },
    text: cap(s.text, MAX_SNIPPET_TARGET_TEXT),
  };
}

/**
 * The context block "Add to chat" inserts into the composer: where the
 * snippet is from (with its id, so the assistant can look it up with
 * list_snippets) and its text as a quote.
 */
export function snippetChatContext(s: Pick<Snippet, 'id' | 'title' | 'kind' | 'fileName' | 'filePath' | 'page' | 'text' | 'latex'>): string {
  const title = s.title.trim();
  const head = `Snippet${title ? ` “${short(title, 80)}”` : ''} (${KIND_LABEL[s.kind]?.toLowerCase() ?? 'snippet'}) from ${snippetSource(s)} [id ${s.id}]:`;
  const body = s.latex?.trim() ? `$$\n${s.latex.trim()}\n$$` : cap(s.text.trim(), MAX_SNIPPET_CHAT_TEXT);
  const quoted = body
    ? body.split('\n').map(line => `> ${line}`).join('\n')
    : '> (no text in this region)';
  const omitted = !s.latex?.trim() && Array.from(s.text.trim()).length > MAX_SNIPPET_CHAT_TEXT ? '\n> …' : '';
  return `${head}\n${quoted}${omitted}\n\n`;
}

/** Appends a context block to a draft, separated by a blank line. */
export function appendToDraft(draft: string, block: string): string {
  if (!draft.trim()) return block;
  return `${draft.replace(/\s+$/, '')}\n\n${block}`;
}

function str(value: unknown): value is string {
  return typeof value === 'string';
}

/** A snippet record from the backend or a drag payload, or null when malformed. */
export function readSnippet(value: unknown): Snippet | null {
  if (typeof value !== 'object' || value === null) return null;
  const v = value as Record<string, unknown>;
  if (!str(v.id) || !v.id || !str(v.filePath) || !v.filePath) return null;
  if (typeof v.page !== 'number' || !Number.isInteger(v.page) || v.page < 1) return null;
  if (!isSnippetRect(v.rect)) return null;
  return {
    id: v.id,
    statementId: str(v.statementId) ? v.statementId : v.id,
    filePath: v.filePath,
    fileName: str(v.fileName) && v.fileName ? v.fileName : fileNameOf(v.filePath),
    page: v.page,
    rect: { x: v.rect.x, y: v.rect.y, width: v.rect.width, height: v.rect.height },
    text: str(v.text) ? v.text : '',
    title: str(v.title) ? v.title : '',
    note: str(v.note) ? v.note : '',
    tags: Array.isArray(v.tags) ? v.tags.filter(str) : [],
    kind: isSnippetKind(v.kind) ? v.kind : 'passage',
    hasImage: v.hasImage === true,
    latex: str(v.latex) && v.latex.trim() ? v.latex : null,
    latexModel: str(v.latexModel) && v.latexModel.trim() ? v.latexModel : null,
    scope: str(v.scope) ? v.scope : 'global',
    createdAt: str(v.createdAt) ? v.createdAt : '',
    updatedAt: str(v.updatedAt) ? v.updatedAt : '',
  };
}

/** Tags typed as "a, b c" → distinct, trimmed, at most 20 of 40 characters. */
export function parseTags(input: string): string[] {
  const out: string[] = [];
  for (const raw of input.split(/[,;\n]/)) {
    const tag = raw.trim().replace(/\s+/g, ' ').slice(0, 40);
    if (tag && !out.some(t => t.toLowerCase() === tag.toLowerCase())) out.push(tag);
    if (out.length === 20) break;
  }
  return out;
}

/** Newest first, then by id (stable). */
export function sortSnippets(items: readonly Snippet[]): Snippet[] {
  return [...items].sort((a, b) => (a.updatedAt === b.updatedAt ? (a.id < b.id ? -1 : a.id > b.id ? 1 : 0) : a.updatedAt < b.updatedAt ? 1 : -1));
}

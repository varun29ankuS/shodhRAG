/**
 * Pure logic of workspaces: grouping chats by workspace for the sidebar, the composer's
 * scope chip, source summaries, template order, the answer scope sent with a question,
 * and the line diff of two instruction versions.
 *
 * Type-only imports, so Node's strip-types test runner loads it directly
 * (`app/tests/workspaces.test.ts`).
 */

import type { AnswerScope } from '../agent/useAgentSession';
import type { SourceCounts, SourceKind, SourceState, Workspace, WorkspaceTab, WorkspaceTemplate } from './types';

export const WORKSPACE_TABS: readonly WorkspaceTab[] = ['overview', 'sources', 'chats', 'memory', 'visuals', 'results'];

export const WORKSPACE_TAB_LABELS: Record<WorkspaceTab, string> = {
  overview: 'Overview',
  sources: 'Sources',
  chats: 'Chats',
  memory: 'Memory',
  visuals: 'Visuals',
  results: 'Results & graph',
};

/** Icons a workspace may use (mirrors `shodh_rag::workspaces::ICONS`). */
export const WORKSPACE_ICONS = [
  'folder',
  'book-open',
  'flask',
  'file-text',
  'pen-line',
  'briefcase',
  'scale',
  'graduation-cap',
  'landmark',
  'lightbulb',
  'microscope',
  'clipboard-check',
] as const;

/** Colours a workspace may use (mirrors `shodh_rag::workspaces::COLORS`): theme tokens. */
export const WORKSPACE_COLORS = ['neutral', 'accent', 'info', 'success', 'warning', 'violet'] as const;

/** Text colour class of a workspace colour (existing theme tokens only). */
export function colorClass(color: string): string {
  switch (color) {
    case 'accent': return 'text-shodh-accent-text';
    case 'info': return 'text-shodh-info';
    case 'success': return 'text-shodh-success';
    case 'warning': return 'text-shodh-warning';
    case 'violet': return 'text-shodh-violet';
    default: return 'text-shodh-text-muted';
  }
}

/** Longest instructions, in characters (mirrors `MAX_INSTRUCTIONS_CHARS`). */
export const MAX_INSTRUCTIONS_CHARS = 6000;

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** "2 folders · 5 files · 1 snippet", or "No sources yet". */
export function sourceSummary(counts: SourceCounts): string {
  const parts: string[] = [];
  if (counts.folders > 0) parts.push(plural(counts.folders, 'folder', 'folders'));
  if (counts.files > 0) parts.push(plural(counts.files, 'file', 'files'));
  if (counts.snippets > 0) parts.push(plural(counts.snippets, 'snippet', 'snippets'));
  if (counts.papers > 0) parts.push(plural(counts.papers, 'paper', 'papers'));
  return parts.length > 0 ? parts.join(' · ') : 'No sources yet';
}

export function totalSources(counts: SourceCounts): number {
  return counts.folders + counts.files + counts.snippets + counts.papers;
}

export const SOURCE_KIND_LABELS: Record<SourceKind, string> = {
  folder: 'Folders',
  file: 'Files',
  snippet: 'Snippets',
  paper: 'Papers',
};

/** What a source's index state means for answers, in words. */
export function sourceStateLabel(state: SourceState): string {
  switch (state) {
    case 'indexed': return 'Searched';
    case 'not_indexed': return 'Not indexed: answers cannot search it';
    case 'missing': return 'No longer in the Library';
    case 'reference': return 'Reference only: not in your library, never searched';
    default: return 'Could not be checked';
  }
}

/** The composer's scope chip: what this question will search. */
export interface ScopeChip {
  label: string;
  title: string;
}

/**
 * The chip for a question: the workspace's sources (the default in a workspace), the
 * whole library ("search all my library" turned on for this question), or every source
 * outside a workspace.
 */
export function scopeChip(
  workspace: Pick<Workspace, 'name' | 'sourceCounts'> | null,
  searchAll: boolean,
  librarySources: number,
): ScopeChip {
  if (!workspace) {
    return librarySources === 0
      ? { label: 'No sources yet', title: 'Add a folder in Library' }
      : { label: `All sources · ${librarySources}`, title: 'Answers search everything you have indexed. Manage sources in Library.' };
  }
  if (searchAll) {
    return {
      label: `All my library · ${workspace.name}`,
      title: `This question searches your whole library. The instructions and memories of “${workspace.name}” still apply.`,
    };
  }
  const total = totalSources(workspace.sourceCounts);
  return {
    label: `${workspace.name} · ${total === 0 ? 'no sources' : plural(total, 'source', 'sources')}`,
    title: total === 0
      ? `“${workspace.name}” has no sources yet: answers find nothing until you add some, or turn on “Search all my library”.`
      : `Answers search only the sources of “${workspace.name}” (${sourceSummary(workspace.sourceCounts)}).`,
  };
}

/**
 * The answer scope sent with a question. In a workspace the workspace decides what is
 * searched (resolved by the backend from its id), so the Library's included-sources
 * selection does not apply; files the user asked about stay as a narrower limit.
 */
export function answerScope(
  scope: AnswerScope | null,
  workspaceId: string | null,
  searchAll: boolean,
): AnswerScope | null {
  const workspace = workspaceId?.trim() || null;
  if (!workspace) return scope;
  return {
    sourceIds: [],
    sourceFiles: scope?.sourceFiles ?? [],
    ...(scope?.pages && scope.pages.length > 0 ? { pages: scope.pages } : {}),
    workspaceId: workspace,
    ...(searchAll ? { searchAll: true } : {}),
  };
}

/** The fields grouping needs; `Conversation` satisfies it. */
export interface GroupableChat {
  id: string;
  updatedAt: string;
  pinned: boolean;
  workspaceId?: string | null;
}

export type GroupableWorkspace = Pick<Workspace, 'id' | 'name' | 'color' | 'icon' | 'pinned' | 'archived'>;

export interface ChatGroup<T extends GroupableChat> {
  /** Null for "No workspace". */
  workspace: GroupableWorkspace | null;
  items: T[];
  /** Newest `updatedAt` among the items (ms). */
  latest: number;
}

function timeOf(iso: string): number {
  const t = new Date(iso).getTime();
  return Number.isNaN(t) ? Number.NEGATIVE_INFINITY : t;
}

/**
 * Chats grouped by workspace for the sidebar: pinned workspaces first, then the one with
 * the most recent chat; "No workspace" last. Chats of an archived workspace are hidden;
 * chats of a workspace that no longer exists are in "No workspace". Within a group,
 * pinned chats first, then newest first. Workspaces without chats are left out.
 */
export function groupChatsByWorkspace<T extends GroupableChat>(
  chats: readonly T[],
  workspaces: readonly GroupableWorkspace[],
): ChatGroup<T>[] {
  const byId = new Map(workspaces.map(w => [w.id, w]));
  const groups = new Map<string | null, ChatGroup<T>>();
  for (const chat of chats) {
    const id = chat.workspaceId?.trim() || null;
    const workspace = id ? byId.get(id) ?? null : null;
    if (workspace?.archived) continue;
    const key = workspace ? workspace.id : null;
    let group = groups.get(key);
    if (!group) {
      group = { workspace, items: [], latest: Number.NEGATIVE_INFINITY };
      groups.set(key, group);
    }
    group.items.push(chat);
    group.latest = Math.max(group.latest, timeOf(chat.updatedAt));
  }
  for (const group of groups.values()) {
    group.items.sort((a, b) => Number(b.pinned) - Number(a.pinned) || timeOf(b.updatedAt) - timeOf(a.updatedAt));
  }
  const named = [...groups.values()].filter(g => g.workspace !== null);
  named.sort((a, b) =>
    Number(b.workspace?.pinned ?? false) - Number(a.workspace?.pinned ?? false)
    || b.latest - a.latest
    || (a.workspace?.name ?? '').localeCompare(b.workspace?.name ?? ''));
  const loose = groups.get(null);
  return loose ? [...named, loose] : named;
}

/** Templates for the "new workspace" choice: the named ones in order, blank last. */
export function orderTemplates(templates: readonly WorkspaceTemplate[]): WorkspaceTemplate[] {
  return [...templates.filter(t => t.id !== 'blank'), ...templates.filter(t => t.id === 'blank')];
}

/** Workspaces as the list shows them: pinned, then most recently active or changed. */
export function sortWorkspaces<T extends Workspace & { lastChatAt?: string | null }>(items: readonly T[]): T[] {
  const activity = (w: T) => Math.max(timeOf(w.lastActiveAt ?? ''), timeOf(w.lastChatAt ?? ''), timeOf(w.updatedAt));
  return [...items].sort((a, b) => Number(b.pinned) - Number(a.pinned) || activity(b) - activity(a) || a.name.localeCompare(b.name));
}

export type DiffOp = 'same' | 'added' | 'removed';

export interface DiffLine {
  op: DiffOp;
  text: string;
}

/**
 * Line diff of two instruction versions (longest common subsequence), removals before
 * additions where a block changed. Mirrors `shodh_rag::workspaces::line_diff`.
 */
export function lineDiff(before: string, after: string): DiffLine[] {
  const a = before === '' ? [] : before.split('\n');
  const b = after === '' ? [] : after.split('\n');
  const lcs: number[][] = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0));
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      out.push({ op: 'same', text: a[i] });
      i++;
      j++;
    } else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
      out.push({ op: 'removed', text: a[i] });
      i++;
    } else {
      out.push({ op: 'added', text: b[j] });
      j++;
    }
  }
  for (; i < a.length; i++) out.push({ op: 'removed', text: a[i] });
  for (; j < b.length; j++) out.push({ op: 'added', text: b[j] });
  return out;
}

/** A diff line list from an approval's details, or null when it is not one. */
export function parseDiff(value: unknown): DiffLine[] | null {
  if (!Array.isArray(value)) return null;
  const lines: DiffLine[] = [];
  for (const item of value) {
    if (typeof item !== 'object' || item === null) return null;
    const { op, text } = item as { op?: unknown; text?: unknown };
    if ((op !== 'same' && op !== 'added' && op !== 'removed') || typeof text !== 'string') return null;
    lines.push({ op, text });
  }
  return lines;
}

/** The workspace a new chat starts in: the open workspace page, else the active chat's. */
export function workspaceForNewChat(openWorkspaceId: string | null, activeChatWorkspaceId: string | null | undefined): string | null {
  return openWorkspaceId?.trim() || activeChatWorkspaceId?.trim() || null;
}

/** Whether `path` is `folder` or inside it, across separators and (on Windows) case. */
export function pathWithin(path: string, folder: string, caseInsensitive: boolean): boolean {
  const norm = (p: string) => {
    let out = p.trim().replace(/\\/g, '/');
    while (out.length > 1 && out.endsWith('/') && !out.endsWith(':/')) out = out.slice(0, -1);
    return caseInsensitive ? out.toLowerCase() : out;
  };
  const p = norm(path);
  const f = norm(folder);
  if (!f) return false;
  return p === f || p.startsWith(f.endsWith('/') ? f : `${f}/`);
}

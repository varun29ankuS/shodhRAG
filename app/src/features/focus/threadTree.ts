/**
 * Side threads as a tree: drill-down threads hang under the thread whose
 * answer they were opened from (`parentThreadId`). Stored lists may be
 * incomplete (the device store keeps only the newest threads) or corrupt,
 * so a thread whose parent is missing, or whose parent chain loops, is
 * treated as a root rather than lost.
 *
 * Pure module, unit-tested with Node (`app/tests/focusTree.test.ts`).
 */

import type { FocusTarget, FocusThread } from './focusTypes.ts';
import { repliesLabel, sameTarget } from './threadStore.ts';

/** Nested levels allowed below the thread opened from the conversation. */
export const MAX_DEPTH = 6;

export interface ThreadNode {
  thread: FocusThread;
  children: ThreadNode[];
}

/** Effective parent of each thread: null for roots, orphans and threads in a loop. */
export function effectiveParents(threads: readonly FocusThread[]): Map<string, string | null> {
  const byId = new Map(threads.map(t => [t.id, t]));
  const result = new Map<string, string | null>();
  for (const thread of threads) {
    const parent = thread.parentThreadId;
    if (!parent || !byId.has(parent)) {
      result.set(thread.id, null);
      continue;
    }
    // Walk up; reaching this thread again means it sits in a loop, and loop
    // members become roots. A thread whose chain only reaches a loop it is
    // not part of keeps its parent (it hangs under that loop member).
    const seen = new Set<string>();
    let cursor: string | undefined = parent;
    let loops = false;
    while (cursor) {
      if (cursor === thread.id) {
        loops = true;
        break;
      }
      if (seen.has(cursor)) break;
      seen.add(cursor);
      cursor = byId.get(cursor)?.parentThreadId;
    }
    result.set(thread.id, loops ? null : parent);
  }
  return result;
}

/** Roots (oldest first), each with its nested threads (oldest first). */
export function buildThreadTree(threads: readonly FocusThread[]): ThreadNode[] {
  const parents = effectiveParents(threads);
  const nodes = new Map<string, ThreadNode>();
  for (const thread of threads) if (!nodes.has(thread.id)) nodes.set(thread.id, { thread, children: [] });
  const roots: ThreadNode[] = [];
  for (const [id, node] of nodes) {
    const parent = parents.get(id) ?? null;
    const parentNode = parent ? nodes.get(parent) : undefined;
    if (parentNode) parentNode.children.push(node);
    else roots.push(node);
  }
  return roots;
}

/** Threads opened from the conversation itself (or whose parent is gone). */
export function rootThreads(threads: readonly FocusThread[]): FocusThread[] {
  const parents = effectiveParents(threads);
  return threads.filter(t => parents.get(t.id) === null);
}

/**
 * Roots worth a chip in the conversation: those with turns of their own or
 * anywhere beneath them (a root left empty because the reader drilled down
 * before asking must still lead to its nested discussions).
 */
export function activeRootThreads(threads: readonly FocusThread[]): FocusThread[] {
  const busy = (node: ThreadNode): boolean => node.thread.turns.length > 0 || node.children.some(busy);
  return buildThreadTree(threads).filter(busy).map(n => n.thread);
}

/** The chain from a root to `threadId` (inclusive); empty when unknown. */
export function threadPath(threads: readonly FocusThread[], threadId: string): FocusThread[] {
  const byId = new Map(threads.map(t => [t.id, t]));
  const parents = effectiveParents(threads);
  const path: FocusThread[] = [];
  const seen = new Set<string>();
  let cursor: string | null = threadId;
  while (cursor && !seen.has(cursor)) {
    const thread = byId.get(cursor);
    if (!thread) break;
    seen.add(cursor);
    path.unshift(thread);
    cursor = parents.get(cursor) ?? null;
  }
  return path;
}

/** How many threads hang (at any depth) under `threadId`. */
export function descendantCount(threads: readonly FocusThread[], threadId: string): number {
  const find = (nodes: readonly ThreadNode[]): ThreadNode | null => {
    for (const node of nodes) {
      if (node.thread.id === threadId) return node;
      const inner = find(node.children);
      if (inner) return inner;
    }
    return null;
  };
  const count = (node: ThreadNode): number => node.children.reduce((n, c) => n + 1 + count(c), 0);
  const node = find(buildThreadTree(threads));
  return node ? count(node) : 0;
}

/** The existing thread about `target` opened from the same place, if any. */
export function findChildThread(
  threads: readonly FocusThread[],
  target: FocusTarget,
  parentThreadId: string | null,
): FocusThread | null {
  const parents = effectiveParents(threads);
  return threads.find(t => (parents.get(t.id) ?? null) === parentThreadId && sameTarget(t.anchor.target, target)) ?? null;
}

/** An outer level of a nested thread: its object and the exchange the next level came from. */
export interface ThreadAncestor {
  target: FocusTarget;
  question?: string;
  answer?: string;
}

/**
 * The outer levels of a thread opened from `parentThreadId` (at its answer
 * `parentTurnId`), outermost first: for each, the object, the answer the
 * next level was opened from (the latest answer when unknown) and the
 * question before it.
 */
export function ancestorsFor(
  threads: readonly FocusThread[],
  parentThreadId: string | null,
  parentTurnId: string | null,
): ThreadAncestor[] {
  if (!parentThreadId) return [];
  const path = threadPath(threads, parentThreadId).slice(-MAX_DEPTH);
  return path.map((thread, i) => {
    const turnId = i === path.length - 1 ? parentTurnId : path[i + 1].parentTurnId ?? null;
    let at = turnId ? thread.turns.findIndex(t => t.id === turnId && t.role === 'assistant') : -1;
    if (at < 0) {
      for (let k = thread.turns.length - 1; k >= 0; k--) {
        if (thread.turns[k].role === 'assistant') {
          at = k;
          break;
        }
      }
    }
    const info: ThreadAncestor = { target: thread.anchor.target };
    if (at >= 0) {
      info.answer = thread.turns[at].content;
      for (let k = at - 1; k >= 0; k--) {
        if (thread.turns[k].role === 'user') {
          info.question = thread.turns[k].content;
          break;
        }
      }
    }
    return info;
  });
}

/** One visible row of the exploration map (a `role="tree"`). */
export interface FlatNode {
  id: string;
  label: string;
  /** 1-based, as `aria-level`. */
  level: number;
  parentId: string | null;
  hasChildren: boolean;
  expanded: boolean;
  /** 1-based position among siblings, and the sibling count. */
  posInSet: number;
  setSize: number;
}

/** Label of a node in the map: the object, and how much was said about it. */
export function nodeLabel(thread: FocusThread): string {
  return thread.turns.length > 0 ? repliesLabel(thread) : thread.anchor.target.label;
}

/** Visible rows in reading order; children of collapsed nodes are left out. */
export function flattenTree(roots: readonly ThreadNode[], collapsed: ReadonlySet<string> = new Set()): FlatNode[] {
  const out: FlatNode[] = [];
  const walk = (nodes: readonly ThreadNode[], level: number, parentId: string | null) => {
    nodes.forEach((node, i) => {
      const hasChildren = node.children.length > 0;
      const expanded = hasChildren && !collapsed.has(node.thread.id);
      out.push({
        id: node.thread.id,
        label: nodeLabel(node.thread),
        level,
        parentId,
        hasChildren,
        expanded,
        posInSet: i + 1,
        setSize: nodes.length,
      });
      if (expanded) walk(node.children, level + 1, node.thread.id);
    });
  };
  walk(roots, 1, null);
  return out;
}

export type TreeKeyResult =
  | { type: 'focus'; id: string }
  | { type: 'expand'; id: string }
  | { type: 'collapse'; id: string }
  | { type: 'activate'; id: string }
  | null;

/**
 * WAI-ARIA tree keys: ↑/↓ move, → expands (or enters the first child),
 * ← collapses (or moves to the parent), Home/End jump, Enter/Space open.
 */
export function treeKey(rows: readonly FlatNode[], currentId: string, key: string): TreeKeyResult {
  const index = rows.findIndex(r => r.id === currentId);
  if (index < 0) return rows.length > 0 ? { type: 'focus', id: rows[0].id } : null;
  const row = rows[index];
  switch (key) {
    case 'ArrowDown':
      return index + 1 < rows.length ? { type: 'focus', id: rows[index + 1].id } : null;
    case 'ArrowUp':
      return index > 0 ? { type: 'focus', id: rows[index - 1].id } : null;
    case 'Home':
      return { type: 'focus', id: rows[0].id };
    case 'End':
      return { type: 'focus', id: rows[rows.length - 1].id };
    case 'ArrowRight':
      if (!row.hasChildren) return null;
      if (!row.expanded) return { type: 'expand', id: row.id };
      return index + 1 < rows.length ? { type: 'focus', id: rows[index + 1].id } : null;
    case 'ArrowLeft':
      if (row.hasChildren && row.expanded) return { type: 'collapse', id: row.id };
      return row.parentId ? { type: 'focus', id: row.parentId } : null;
    case 'Enter':
    case ' ':
      return { type: 'activate', id: row.id };
    default:
      return null;
  }
}

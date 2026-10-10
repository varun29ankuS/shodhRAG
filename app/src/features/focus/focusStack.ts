/**
 * The levels of the focus pop-out. Opening an object inside a side answer
 * pushes a level; one overlay shows the current level, breadcrumbs show the
 * way back. Going back keeps the levels ahead, so "forward" returns to them
 * until a new object is opened from an earlier level.
 *
 * Pure module, unit-tested with Node (`app/tests/focusStack.test.ts`).
 */

import { MAX_DEPTH } from './threadTree.ts';

export interface StackState<L> {
  levels: readonly L[];
  /** Level shown; 0 is the object opened from the conversation. */
  index: number;
}

export type StackAction<L> =
  | { type: 'push'; level: L }
  | { type: 'back' }
  | { type: 'forward' }
  | { type: 'jump'; index: number }
  /** Replace every level (a jump from the exploration map). */
  | { type: 'reset'; levels: readonly L[]; index?: number };

export function initialStack<L>(root: L): StackState<L> {
  return { levels: [root], index: 0 };
}

/** Whether a level can be opened from the current one. */
export function canPush<L>(state: StackState<L>): boolean {
  return state.index < MAX_DEPTH;
}

export function stackReducer<L>(state: StackState<L>, action: StackAction<L>): StackState<L> {
  switch (action.type) {
    case 'push': {
      if (!canPush(state)) return state;
      return { levels: [...state.levels.slice(0, state.index + 1), action.level], index: state.index + 1 };
    }
    case 'back':
      return state.index > 0 ? { ...state, index: state.index - 1 } : state;
    case 'forward':
      return state.index < state.levels.length - 1 ? { ...state, index: state.index + 1 } : state;
    case 'jump': {
      const index = Math.trunc(action.index);
      if (!Number.isFinite(index) || index < 0 || index >= state.levels.length || index === state.index) return state;
      return { ...state, index };
    }
    case 'reset': {
      const levels = action.levels.slice(0, MAX_DEPTH + 1);
      if (levels.length === 0) return state;
      const index = Math.min(levels.length - 1, Math.max(0, action.index ?? levels.length - 1));
      return { levels, index };
    }
  }
}

export interface StackKeyInput {
  key: string;
  altKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  /** Focus is in a text field, select or contenteditable element. */
  editable: boolean;
}

/**
 * Alt+← back, Alt+→ forward (also while typing: Alt+arrows do not edit
 * text); Backspace goes back only outside text fields.
 */
export function stackKey(input: StackKeyInput): 'back' | 'forward' | null {
  const mod = input.ctrlKey || input.metaKey;
  if (input.altKey && !mod && !input.shiftKey) {
    if (input.key === 'ArrowLeft') return 'back';
    if (input.key === 'ArrowRight') return 'forward';
    return null;
  }
  if (input.key === 'Backspace' && !input.altKey && !mod && !input.shiftKey && !input.editable) return 'back';
  return null;
}

export type Crumb = { type: 'level'; index: number; label: string; current: boolean } | { type: 'gap'; hidden: number[] };

/**
 * Breadcrumbs for the levels up to and including the current one. Long
 * trails keep the first level and the last `tail` levels, with a gap that
 * lists the hidden ones.
 */
export function crumbs(labels: readonly string[], index: number, max = 5): Crumb[] {
  const shown = labels.slice(0, index + 1);
  const all: Crumb[] = shown.map((label, i) => ({ type: 'level', index: i, label, current: i === index }));
  if (shown.length <= max || max < 3) return all;
  const tail = max - 2;
  const hidden = Array.from({ length: shown.length - 1 - tail }, (_, k) => k + 1);
  return [all[0], { type: 'gap', hidden }, ...all.slice(shown.length - tail)];
}

/**
 * The code review pane's interface, apart from the provider that runs it
 * (`CodeReviewContext.tsx`): its value's type, the React context, the hook that reads it
 * and the pure helpers code views use. A code block in an answer offers "Review this"
 * through `useCodeReview`, so it renders under any provider of this value: the main
 * window's, which opens the pane, and the floating chat's (`features/dock`), which sends
 * the snippet to main to open it there.
 */
import type React from 'react';
import { createContext, useContext } from 'react';
import type { LineSelection } from './diffModel';
import { linesAtOffsets } from './reviewModel';
import type { CheckRun, ReviewSubject, StoredReview } from './reviewModel';
import type { StoredDesignCheck } from './designModel';

export type { ReviewSubject } from './reviewModel';

/** Something on screen that can be reviewed with the shortcut: its root and what it shows. */
export interface ReviewSource {
  root: HTMLElement;
  /** The subject, with the selected text (if any) turned into a line selection. */
  subject: (selectedText: string) => ReviewSubject;
}

export interface CodeReviewValue {
  /** Open the pane on `subject` (its selection highlighted). */
  open: (subject: ReviewSubject) => void;
  close: () => void;
  subject: ReviewSubject | null;
  review: StoredReview | null;
  setReview: (review: StoredReview | null) => void;
  /** A design check whose findings the pane shows (with or without code beside them). */
  design: StoredDesignCheck | null;
  /** Show a design check's findings; the subject (a file it cites) stays if given. */
  openDesign: (check: StoredDesignCheck, subject?: ReviewSubject | null) => void;
  setSelection: (selection: LineSelection | null) => void;
  /** Checks run in the conversation, by `reviewId:findingIndex`. */
  runs: Record<string, CheckRun>;
  /** A check was sent to the conversation: its command is matched when it starts. */
  awaitCheck: (key: string, command: string) => void;
  /** Make `source` reviewable with the shortcut; returns the cleanup. */
  register: (source: ReviewSource) => () => void;
  /** Bumped by the shortcut while the pane is open: the pane runs the review. */
  runRequest: number;
  /** The pane's own element, inside which the shortcut runs the review. */
  paneRef: React.RefObject<HTMLElement | null>;
}

export const CodeReviewContext = createContext<CodeReviewValue | null>(null);

/** The code pane, or null outside its provider (the print view). */
export function useCodeReview(): CodeReviewValue | null {
  return useContext(CodeReviewContext);
}

/** Characters of `container`'s text before `node` at `offset`. */
function offsetIn(container: Node, node: Node, offset: number): number | null {
  if (!container.contains(node)) return null;
  const range = document.createRange();
  range.setStart(container, 0);
  range.setEnd(node, offset);
  return range.toString().length;
}

/**
 * The lines of `code` the text selection inside `container` (the code's own element)
 * covers, found by where it starts and ends, so a selected line that also appears
 * elsewhere is the one selected. Null without a selection in it.
 */
export function snippetSelection(container: HTMLElement | null, code: string, path: string): LineSelection | null {
  const selection = typeof window === 'undefined' ? null : window.getSelection();
  if (!container || !selection || selection.rangeCount === 0 || selection.isCollapsed) return null;
  const range = selection.getRangeAt(0);
  const start = offsetIn(container, range.startContainer, range.startOffset);
  const end = offsetIn(container, range.endContainer, range.endOffset);
  if (start === null || end === null) return null;
  const lines = linesAtOffsets(code, start, end);
  return lines ? { path, side: 'new', from: lines.from, to: lines.to } : null;
}

/** Whether `element` takes typed text (a field, a text area, an editable element). */
export function isTextEntry(element: Element | null): boolean {
  if (!element) return false;
  const tag = element.tagName;
  return tag === 'TEXTAREA' || tag === 'INPUT' || (element as HTMLElement).isContentEditable;
}


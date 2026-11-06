import React, { useEffect, useRef } from 'react';
import { ScanEye } from 'lucide-react';
import { cn } from '../../lib/utils';
import { snippetSelection, useCodeReview } from './CodeReviewContext';
import type { ReviewSubject } from './CodeReviewContext';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

/**
 * "Review this": opens the code pane on what this code view shows (its selected lines
 * highlighted), where a check is picked and the fresh-eyes reviewer runs. The code view
 * it sits in (the nearest `role="group"`, or `root`) answers to the shortcut too, while
 * the keyboard focus or a text selection is inside it. Nothing outside the pane's
 * provider (the print view) shows it.
 */
export function ReviewThisButton({
  subject,
  className,
  root,
}: {
  subject: (selectedText: string) => ReviewSubject;
  className?: string;
  /** The element the shortcut answers in (the button's code view when absent). */
  root?: React.RefObject<HTMLElement | null>;
}) {
  const review = useCodeReview();
  const button = useRef<HTMLButtonElement>(null);
  const subjectRef = useRef(subject);
  subjectRef.current = subject;
  const register = review?.register;
  useEffect(() => {
    const element = root?.current ?? button.current?.closest<HTMLElement>('[role="group"]') ?? null;
    if (!register || !element) return;
    return register({ root: element, subject: text => subjectRef.current(text) });
  }, [register, root]);
  if (!review) return null;
  return (
    <button
      ref={button}
      type="button"
      onClick={() => review.open(subject(''))}
      title="Have a fresh-eyes reviewer check this code. It sees only this code, never the conversation."
      className={cn(
        'inline-flex items-center gap-1.5 h-7 px-2 rounded-md text-caption font-medium text-shodh-text-secondary hover:bg-shodh-raised-2 hover:text-shodh-text transition-colors duration-micro',
        FOCUS_RING,
        className,
      )}
    >
      <ScanEye className="w-3.5 h-3.5" aria-hidden="true" />
      Review this
    </button>
  );
}

/**
 * A code block of an answer, reviewable: with the shortcut on text selected in it, or
 * with the button it hands to `children` for its header. The block's own lines are the
 * subject (a snippet, not a file of the project); a selection in its code narrows the
 * review to the lines selected, found by where the selection starts and ends.
 */
export function ReviewableSnippet({
  code,
  language,
  children,
}: {
  code: string;
  language: string;
  children: (reviewButton: React.ReactNode, codeRef: React.RefObject<HTMLDivElement | null>) => React.ReactNode;
}) {
  const root = useRef<HTMLDivElement>(null);
  const codeBox = useRef<HTMLDivElement>(null);
  const subject = (): ReviewSubject => ({
    kind: 'file',
    title: `Code from the answer (${language})`,
    path: 'snippet',
    text: code,
    language,
    firstLine: 1,
    workspaceId: null,
    selection: snippetSelection(codeBox.current, code, 'snippet'),
  });
  return (
    <div ref={root}>
      {children(<ReviewThisButton subject={subject} root={root} className="h-6" />, codeBox)}
    </div>
  );
}

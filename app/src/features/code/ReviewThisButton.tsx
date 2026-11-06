import React, { useEffect } from 'react';
import { ScanEye } from 'lucide-react';
import { cn } from '../../lib/utils';
import { selectionLines, useCodeReview } from './CodeReviewContext';
import type { ReviewSubject } from './CodeReviewContext';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

/**
 * "Review this": opens the code pane on what this code view shows (its selected lines
 * highlighted), where a check is picked and the fresh-eyes reviewer runs. Nothing
 * outside the pane's provider (the print view) shows it.
 */
export function ReviewThisButton({ subject, className }: { subject: () => ReviewSubject; className?: string }) {
  const review = useCodeReview();
  if (!review) return null;
  return (
    <button
      type="button"
      onClick={() => review.open(subject())}
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
 * subject (a snippet, not a file of the project).
 */
export function ReviewableSnippet({
  code,
  language,
  children,
}: {
  code: string;
  language: string;
  children: (reviewButton: React.ReactNode) => React.ReactNode;
}) {
  const root = React.useRef<HTMLDivElement>(null);
  const subject = useCallbackSubject(code, language);
  useReviewSource(root, subject);
  return (
    <div ref={root}>
      {children(
        <ReviewThisButton
          subject={() => {
            // Text selected in this block narrows the review to its lines.
            const selected = window.getSelection();
            const inside = selected?.anchorNode && root.current?.contains(selected.anchorNode);
            return subject(inside ? selected.toString() : '');
          }}
          className="h-6"
        />,
      )}
    </div>
  );
}

function useCallbackSubject(code: string, language: string): (selectedText: string) => ReviewSubject {
  return React.useCallback(
    (selectedText: string) => ({
      kind: 'file',
      title: `Code from the answer (${language})`,
      path: 'snippet',
      text: code,
      language,
      firstLine: 1,
      workspaceId: null,
      selection: selectionLines(code, selectedText, 'snippet'),
    }),
    [code, language],
  );
}

/**
 * Makes the element `root` points at reviewable with the shortcut: text selected in it
 * opens the pane on `subject(selectedText)`.
 */
export function useReviewSource(root: React.RefObject<HTMLElement | null>, subject: (selectedText: string) => ReviewSubject) {
  const review = useCodeReview();
  const subjectRef = React.useRef(subject);
  subjectRef.current = subject;
  const register = review?.register;
  useEffect(() => {
    const element = root.current;
    if (!register || !element) return;
    return register({ root: element, subject: text => subjectRef.current(text) });
  }, [register, root]);
}

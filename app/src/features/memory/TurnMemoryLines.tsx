import React, { useState } from 'react';
import { Bookmark, BookmarkCheck } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { errorText, undoSuggestion } from './api';
import { MAX_LINES, lineText, summaryText } from './turnMemories';
import type { TurnMemoryLine } from './turnMemories';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

const LINK = cn(
  'shrink-0 underline underline-offset-2 text-shodh-accent-text hover:text-shodh-accent-hover rounded-sm disabled:opacity-50 disabled:cursor-not-allowed',
  FOCUS_RING,
);

interface TurnMemoryLinesProps {
  lines: readonly TurnMemoryLine[];
  /** Opens the suggestions (Settings → Memory) for "Review". */
  onReview: () => void;
  /** Under the user's message (right-aligned), else under the answer. */
  align: 'end' | 'start';
}

/**
 * What was saved from a turn, in the chat: "Remembered: “…” · Undo" under a message that
 * asked Shodh to remember something, "Learned: “…” · Undo" under an answer memories were
 * learned from ("Learned 3 things · Show" when there are more than two), and "Shodh
 * suggests remembering “…” · Review" for what waits for approval. Undo is the Undo of
 * Settings → Memory → "Recently learned".
 */
export function TurnMemoryLines({ lines, onReview, align }: TurnMemoryLinesProps) {
  const [busy, setBusy] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(false);
  if (lines.length === 0) return null;

  const undo = async (line: TurnMemoryLine) => {
    setBusy(line.id);
    try {
      const after = await undoSuggestion(line.id);
      // Undone: the user's own note is erased as it goes, which is the undo, not "nothing to do".
      if (after?.status !== 'undone' && after?.forgotten) {
        notify.info('Nothing to undo', { description: 'You forgot this memory, so its words are already erased.' });
      } else {
        notify.success('Undone', { description: 'Shodh no longer remembers it.' });
      }
    } catch (err) {
      notify.error('It was not undone', { description: errorText(err) });
    } finally {
      setBusy(null);
    }
  };

  const folded = !expanded && lines.length > MAX_LINES;
  return (
    <div
      aria-live="polite"
      className={cn('flex flex-col gap-0.5 text-[12.5px] text-shodh-text-muted', align === 'end' ? 'items-end' : 'items-start')}
    >
      {folded ? (
        <p className="m-0 inline-flex items-center gap-1.5 max-w-full">
          <BookmarkCheck aria-hidden="true" className="w-3.5 h-3.5 shrink-0 text-shodh-accent-text" />
          <span>{summaryText(lines)}</span>
          <span aria-hidden="true">·</span>
          <button type="button" className={LINK} onClick={() => setExpanded(true)}>
            Show
          </button>
        </p>
      ) : (
        lines.map(line => {
          const text = lineText(line);
          const Icon = line.kind === 'suggested' ? Bookmark : BookmarkCheck;
          return (
            <p key={line.id} className="m-0 inline-flex items-center gap-1.5 max-w-full min-w-0">
              <Icon aria-hidden="true" className="w-3.5 h-3.5 shrink-0 text-shodh-accent-text" />
              <span className="min-w-0 truncate" title={text.quote ?? undefined}>
                {text.lead}
                {text.quote !== null && (
                  <>
                    {line.kind === 'suggested' ? ' ' : ': '}
                    <span className="text-shodh-text-secondary">“{text.quote}”</span>
                  </>
                )}
              </span>
              {text.action !== null && <span aria-hidden="true">·</span>}
              {text.action === 'undo' && (
                <button
                  type="button"
                  className={LINK}
                  disabled={busy !== null}
                  onClick={() => void undo(line)}
                  aria-label={`Undo: forget “${text.quote ?? ''}”`}
                >
                  Undo
                </button>
              )}
              {text.action === 'review' && (
                <button type="button" className={LINK} onClick={onReview} aria-label="Review suggested memories in Settings, Memory">
                  Review
                </button>
              )}
            </p>
          );
        })
      )}
    </div>
  );
}

/**
 * Turning on (or rewording) a paper alert, inline in Library → What's new.
 *
 * - Topics suggested from the library show as chips with the exact words they send;
 *   ticking chips fills the words box, which stays editable, and the user can type a
 *   topic of their own. With no suggestions there are no chips, only the box.
 * - The consent line always shows the box's words, exactly as they will be sent, and
 *   nothing is sent before "Turn on".
 * - While nothing may leave the computer, the form says why instead of offering to send.
 */

import React, { useEffect, useId, useState } from 'react';
import { Check } from 'lucide-react';
import { cn } from '../../lib/utils';
import { whatsNewApi } from './api';
import { MAX_WORDS_CHARS, charCount, cleanWords, consentLine, toggleChip } from './consent';
import type { TopicSuggestion } from './types';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

const BUTTON = cn(
  'h-8 px-3 inline-flex items-center gap-1.5 rounded-lg text-small whitespace-nowrap shrink-0 transition-colors duration-micro disabled:opacity-50',
  FOCUS_RING,
);

const MAX_WORDS = MAX_WORDS_CHARS;

export function PaperAlertForm({
  initialWords = '',
  submitLabel,
  blocked,
  onSubmit,
  onCancel,
}: {
  initialWords?: string;
  submitLabel: string;
  /** Why nothing may be sent now, in one line; the form then offers no Turn on. */
  blocked: string | null;
  onSubmit: (words: string) => Promise<void>;
  onCancel: () => void;
}) {
  const inputId = useId();
  const [words, setWords] = useState(initialWords);
  const [ticked, setTicked] = useState<string[]>([]);
  const [suggestions, setSuggestions] = useState<TopicSuggestion[] | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    whatsNewApi
      .suggestions()
      .then(list => live && setSuggestions(list))
      .catch(() => live && setSuggestions([]));
    return () => {
      live = false;
    };
  }, []);

  const toggle = (chip: string) => {
    const on = !ticked.includes(chip);
    setTicked(on ? [...ticked, chip] : ticked.filter(t => t !== chip));
    // What the user typed stays; the chip's words are added or taken out.
    setWords(current => toggleChip(current, chip, on));
  };

  const clean = cleanWords(words);
  const tooLong = charCount(clean) > MAX_WORDS;

  const submit = async () => {
    if (saving || !clean || tooLong || blocked) return;
    setSaving(true);
    setError(null);
    try {
      await onSubmit(clean);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div role="group" aria-label="Paper alert" className="flex flex-col gap-3 rounded-xl border border-shodh-border bg-shodh-surface p-4">
      <label htmlFor={inputId} className="text-small font-medium text-shodh-text">Papers about</label>
      {suggestions && suggestions.length > 0 && (
        <div className="flex flex-col gap-1.5">
          <p className="text-caption text-shodh-text-muted">Suggested from your library</p>
          <div className="flex gap-1.5 overflow-x-auto scrollbar-thin pb-1" role="list">
            {suggestions.map(s => {
              const on = ticked.includes(s.words);
              return (
                <button
                  key={s.words}
                  type="button"
                  role="listitem"
                  aria-pressed={on}
                  className={cn(
                    BUTTON,
                    'h-7 px-2.5 border',
                    on ? 'border-shodh-accent bg-shodh-accent-soft text-shodh-text' : 'border-shodh-border text-shodh-text-secondary hover:bg-shodh-raised',
                  )}
                  onClick={() => toggle(s.words)}
                >
                  {on && <Check className="h-3.5 w-3.5" aria-hidden="true" />}
                  {s.words}
                </button>
              );
            })}
          </div>
        </div>
      )}
      <input
        id={inputId}
        value={words}
        onChange={e => setWords(e.target.value)}
        onKeyDown={e => {
          if (e.key === 'Enter' && !saving) void submit();
          if (e.key === 'Escape') onCancel();
        }}
        placeholder="Type a topic, e.g. linear attention"
        maxLength={MAX_WORDS + 50}
        className={cn('h-9 rounded-lg border border-shodh-border bg-shodh-ground px-3 text-small text-shodh-text', FOCUS_RING)}
      />
      {tooLong && <p className="text-caption text-shodh-warning">Keep the words under {MAX_WORDS} characters; they are sent as they are.</p>}
      {blocked ? (
        <p role="status" className="text-small text-shodh-text-secondary">{blocked}</p>
      ) : (
        <p className="text-small text-shodh-text-secondary">
          {clean ? consentLine(clean) : 'Type a topic to see exactly what is sent.'}
        </p>
      )}
      {!blocked && <p className="text-caption text-shodh-text-muted">The first check runs now. New papers show here and in your Inbox.</p>}
      {error && <p role="alert" className="text-small text-shodh-warning">{error}</p>}
      <div className="flex flex-nowrap justify-end gap-2">
        <button type="button" className={cn(BUTTON, 'text-shodh-text-secondary hover:bg-shodh-raised')} onClick={onCancel}>
          Cancel
        </button>
        {!blocked && (
          <button
            type="button"
            className={cn(BUTTON, 'bg-shodh-accent text-shodh-on-accent font-semibold hover:bg-shodh-accent-hover')}
            disabled={!clean || tooLong || saving}
            onClick={() => void submit()}
          >
            {submitLabel}
          </button>
        )}
      </div>
    </div>
  );
}

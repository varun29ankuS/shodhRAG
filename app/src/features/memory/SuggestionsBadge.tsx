import React, { useEffect, useState } from 'react';
import { Sparkles } from 'lucide-react';
import { cn } from '../../lib/utils';
import { learnStatus, onSuggestionsChanged } from './api';
import { badgeText } from './suggestions';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

/**
 * A small badge near the composer when memories learned from the conversation wait for
 * the user's decision; it opens Settings → Memory. Renders nothing otherwise.
 */
export function SuggestionsBadge({ onOpen }: { onOpen: () => void }) {
  const [pending, setPending] = useState(0);

  useEffect(() => {
    let cancelled = false;
    learnStatus()
      .then(status => {
        if (!cancelled && status) setPending(status.pending);
      })
      .catch(() => {
        // Memory is unavailable (for example before the search models are installed):
        // there is nothing to show.
      });
    const unsubscribe = onSuggestionsChanged(n => {
      if (!cancelled) setPending(n);
    });
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  const text = badgeText(pending);
  if (!text) return null;
  return (
    <button
      type="button"
      onClick={onOpen}
      className={cn(
        'shrink-0 h-[22px] px-2 inline-flex items-center gap-1 rounded-full border border-shodh-border bg-shodh-surface',
        'text-[11.5px] text-shodh-text-secondary hover:text-shodh-text hover:bg-shodh-raised transition-colors duration-micro',
        FOCUS_RING,
      )}
      aria-label={`${text}: review them in Settings, Memory`}
    >
      <Sparkles aria-hidden="true" className="w-3 h-3" />
      {text}
    </button>
  );
}

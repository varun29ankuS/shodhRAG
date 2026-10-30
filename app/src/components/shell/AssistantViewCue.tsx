import React, { useEffect, useRef, useState } from 'react';
import { Sparkles } from 'lucide-react';
import { VIEW_TAB_LABELS } from '../../lib/viewTabs';
import type { ViewTab } from '../../lib/viewTabs';
import { VIEW_CUE_MS, cueAnnouncement, initialCueTracker, nextCue } from '../../lib/viewCue';

interface Cue {
  id: number;
  view: ViewTab;
}

/**
 * Which view the assistant just opened, for a moment after it did (see
 * lib/viewCue). `seq` and `requested` come from the chat session's latest
 * navigation; `tab` is the view open now.
 */
export function useAssistantViewCue(seq: number, requested: ViewTab | null, tab: ViewTab): Cue | null {
  const tracker = useRef(initialCueTracker({ seq, view: requested, tab }));
  const [cue, setCue] = useState<Cue | null>(null);

  useEffect(() => {
    const result = nextCue(tracker.current, { seq, view: requested, tab });
    tracker.current = result.tracker;
    if (result.cue) {
      setCue({ id: seq, view: result.cue });
    } else {
      // The user moved on: the cue no longer describes the view.
      setCue(current => (current && current.view !== tab ? null : current));
    }
  }, [seq, requested, tab]);

  useEffect(() => {
    if (!cue) return undefined;
    const timer = window.setTimeout(() => setCue(null), VIEW_CUE_MS);
    return () => window.clearTimeout(timer);
  }, [cue]);

  return cue;
}

/**
 * "Opened by Shodh" next to the view's title after the assistant changed the
 * view. Never takes focus; the polite live region stays mounted so screen
 * readers hear "Shodh opened Library" when its text changes.
 */
export function AssistantViewCue({ cue }: { cue: Cue | null }) {
  return (
    <span role="status" aria-live="polite" aria-atomic="true" className="inline-flex items-center">
      {cue && (
        <span key={cue.id} className="shell-cue ml-2 inline-flex items-center gap-1 text-[11px] font-medium text-shodh-text-muted">
          <Sparkles className="w-3 h-3 text-shodh-accent-text" aria-hidden="true" />
          <span aria-hidden="true">opened by Shodh</span>
          <span className="sr-only">{cueAnnouncement(VIEW_TAB_LABELS[cue.view])}</span>
        </span>
      )}
    </span>
  );
}

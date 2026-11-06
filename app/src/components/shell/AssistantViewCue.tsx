import React, { useEffect, useRef, useState } from 'react';
import { Sparkles } from 'lucide-react';
import type { ViewTab } from '../../lib/viewTabs';
import { VIEW_CUE_MS, initialCueTracker, nextCue, visibleCue } from '../../lib/viewCue';

interface Cue {
  id: number;
  view: ViewTab;
}

/**
 * Which view the assistant just opened (see lib/viewCue), while it is the
 * view on screen. `seq` and `requested` come from the chat session's latest
 * navigation; `tab` is the view asked for now and `shown` the one rendered
 * (a view whose code is still loading is shown a moment after it is asked
 * for). The cue's time starts when its view is on screen, so it is seen for
 * its full length either way.
 */
export function useAssistantViewCue(seq: number, requested: ViewTab | null, tab: ViewTab, shown: ViewTab): Cue | null {
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

  const visible = visibleCue(cue, shown);
  const visibleId = visible?.id ?? null;
  useEffect(() => {
    if (visibleId === null) return undefined;
    const timer = window.setTimeout(() => setCue(current => (current?.id === visibleId ? null : current)), VIEW_CUE_MS);
    return () => window.clearTimeout(timer);
  }, [visibleId]);

  return visible;
}

/**
 * "Opened by Shodh" next to the view's title after the assistant changed the
 * view. Never takes focus. Screen readers hear "Shodh opened Library: …" from
 * the shell's one live region (lib/announcer, said when the move is followed),
 * so this cue is visual only and two regions never talk over each other.
 */
export function AssistantViewCue({ cue }: { cue: Cue | null }) {
  return (
    <span className="inline-flex items-center" aria-hidden="true">
      {cue && (
        <span key={cue.id} className="shell-cue ml-2 inline-flex items-center gap-1 text-caption font-medium text-shodh-text-muted">
          <Sparkles className="w-3 h-3 text-shodh-accent-text" />
          <span>opened by Shodh</span>
        </span>
      )}
    </span>
  );
}

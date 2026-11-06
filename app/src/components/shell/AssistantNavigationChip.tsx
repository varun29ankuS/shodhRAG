import React from 'react';
import { ArrowRight, X } from 'lucide-react';
import { VIEW_TAB_LABELS } from '../../lib/viewTabs';
import { heldNavigationText } from '../../lib/navigationGate';
import type { Counted } from '../../lib/navigationGate';
import type { HeldNavigation } from '../../features/ask/ChatSessionContext';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background';

/**
 * "Shodh has “Renew lease” to show you in Tasks · Go there": shown instead of
 * switching the view while the user was typing (lib/navigationGate). It never takes
 * the focus, so the user keeps typing; "Go there" follows the move, the cross drops
 * it. It is announced once through the shell's live region when it appears (the
 * session does that), so it is not a live region itself.
 */
export function AssistantNavigationChip({
  held,
  onGo,
  onDismiss,
}: {
  held: Counted<HeldNavigation> | null;
  onGo: () => void;
  onDismiss: () => void;
}) {
  if (!held) return null;
  const where = held.nav.view === null ? null : VIEW_TAB_LABELS[held.nav.view];
  return (
    <span className="ml-2 inline-flex items-center gap-1 rounded-full border border-shodh-border bg-shodh-surface pl-2.5 pr-1 py-0.5 text-caption text-shodh-text-secondary">
      <span>{heldNavigationText(where, held.nav.label, held.count)}</span>
      <button
        type="button"
        onClick={onGo}
        className={`inline-flex items-center gap-1 h-6 rounded-full px-2 font-medium text-shodh-accent-text hover:bg-shodh-raised ${FOCUS_RING}`}
      >
        <ArrowRight className="w-3 h-3" aria-hidden="true" />
        Go there
      </button>
      <button
        type="button"
        onClick={onDismiss}
        aria-label="Dismiss"
        title="Dismiss"
        className={`inline-grid place-items-center w-6 h-6 rounded-full text-shodh-text-faint hover:text-shodh-text hover:bg-shodh-raised ${FOCUS_RING}`}
      >
        <X className="w-3.5 h-3.5" aria-hidden="true" />
      </button>
    </span>
  );
}

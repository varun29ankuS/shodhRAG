import { useCallback, useEffect, useRef, useState } from 'react';
import { Loader2, Palette } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { creationsApi } from '../artifacts/api';
import { designApi, onDesignCheckUpdated } from './api';
import { useCodeReview } from './CodeReviewContext';
import { badgeDetail, badgeText } from './designModel';
import type { DesignCheckRequest } from './api';
import type { StoredDesignCheck } from './designModel';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

interface DesignCheckChipProps {
  conversationId: string;
  /** The Code conversation's project; null in a Creator conversation. */
  workspaceId: string | null;
  /** A Creator conversation: its creation is checked. */
  creates: boolean;
  /** Read again when an answer ends. */
  answerRunning: boolean;
}

/**
 * The Code status line's design badge: "Design check: 3 issues" (or "No issues") for
 * the UI files this conversation's agent wrote, checked by Shodh after each round of
 * writes, with no model. Click to see the findings in the code pane. Before the first
 * check it offers one ("Design check").
 */
export function DesignCheckChip({ conversationId, workspaceId, creates, answerRunning }: DesignCheckChipProps) {
  const pane = useCodeReview();
  const [check, setCheck] = useState<StoredDesignCheck | null>(null);
  const [busy, setBusy] = useState(false);
  const generation = useRef(0);

  const load = useCallback(() => {
    const mine = ++generation.current;
    designApi
      .latest(conversationId, null)
      .then(found => {
        if (mine === generation.current) setCheck(found);
      })
      .catch(err => console.error('Design check unavailable:', String(err)));
  }, [conversationId]);

  useEffect(() => {
    setCheck(null);
    load();
  }, [load]);

  useEffect(() => {
    if (!answerRunning) load();
  }, [answerRunning, load]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    onDesignCheckUpdated(update => {
      if (update.conversationId === conversationId) load();
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(err => console.error('Failed to listen for design-check-updated:', err));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [conversationId, load]);

  const runFirst = async () => {
    setBusy(true);
    try {
      let request: DesignCheckRequest;
      if (creates) {
        const creation = await creationsApi.forConversation(conversationId);
        if (!creation) {
          notify.info('Nothing to check yet', { description: 'The design check runs once this conversation has built something.' });
          return;
        }
        request = { creationId: creation.id, conversationId };
      } else if (workspaceId) {
        request = { workspaceId, conversationId, changes: true };
      } else {
        return;
      }
      const found = await designApi.run(request);
      generation.current += 1;
      setCheck(found);
      pane?.openDesign(found);
    } catch (err) {
      notify.error('The design check did not run', { description: String(err) });
    } finally {
      setBusy(false);
    }
  };

  if (!creates && !workspaceId) return null;
  const errors = check?.report.counts.error ?? 0;
  const errorWords = errors > 0 ? `, ${errors} ${errors === 1 ? 'error' : 'errors'}` : '';
  return (
    <button
      type="button"
      onClick={() => (check ? pane?.openDesign(check) : void runFirst())}
      disabled={busy}
      title={check
        ? `${badgeDetail(check)} Click to see the findings.`
        : creates
          ? 'Check this conversation’s creation with Shodh’s design rules. No model is used.'
          : 'Check the UI files this conversation’s agent changed (before it has written any, the project’s changes since its branch started) with Shodh’s design rules. No model is used.'}
      aria-label={check ? `${badgeText(check)}${errorWords}. Show the findings` : 'Run a design check'}
      className={cn(
        'inline-flex items-center gap-1 h-[26px] px-2 rounded-full text-caption font-medium border shrink-0 whitespace-nowrap tabular-nums transition-colors duration-micro',
        errors > 0
          ? 'border-shodh-warning/60 text-shodh-text hover:bg-shodh-raised'
          : 'border-shodh-border text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised',
        FOCUS_RING,
      )}
    >
      {busy ? <Loader2 className="w-3 h-3 shrink-0 animate-spin" aria-hidden="true" /> : <Palette className="w-3 h-3 shrink-0" aria-hidden="true" />}
      <span>{check ? badgeText(check) : 'Design check'}</span>
    </button>
  );
}

export default DesignCheckChip;

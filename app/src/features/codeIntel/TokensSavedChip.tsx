import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { Gauge } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { SwitchRow } from '../../components/PrivacySettings';
import { useAgentSession } from '../agent/useAgentSession';
import type { AgentEventEnvelope } from '../agent/events';
import { codeIntelApi, errorText } from './api';
import type { ConversationSavings } from './api';
import { chipLabel, sessionDetail, sessionSentence, showsChip } from './format';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

interface TokensSavedChipProps {
  conversationId: string;
  /** Re-read when an answer ends. */
  answerRunning: boolean;
  /** Open Activity, where the running total is. */
  onOpenActivity: () => void;
}

/**
 * Code mode's status line: the tokens this conversation saved because Shodh
 * shortens what the model reads of command output ("≈41,200 tokens saved").
 * One number per conversation, never per answer. Click for what it means and
 * the conversation's switch. Shown from the start, so the switch is there before
 * the first command ("Output shortening on").
 */
export function TokensSavedChip({ conversationId, answerRunning, onOpenActivity }: TokensSavedChipProps) {
  const [view, setView] = useState<ConversationSavings | null>(null);
  const [open, setOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const popoverId = useId();
  const headingId = useId();
  const generation = useRef(0);

  const load = useCallback(() => {
    const mine = ++generation.current;
    codeIntelApi
      .savings(conversationId)
      .then(next => {
        if (mine === generation.current) setView(next);
      })
      .catch(err => console.error('Tokens saved unavailable:', errorText(err)));
  }, [conversationId]);

  useEffect(() => {
    setView(null);
    setOpen(false);
    load();
  }, [load]);

  useEffect(() => {
    if (!answerRunning) load();
  }, [answerRunning, load]);

  // Each finished command may have added to the count (it is saved before the event).
  const onAgentEvent = useCallback(
    (envelope: AgentEventEnvelope) => {
      if (envelope.event.type === 'command_finished') load();
    },
    [load],
  );
  useAgentSession(onAgentEvent);

  const close = useCallback((refocus: boolean) => {
    setOpen(false);
    if (refocus) buttonRef.current?.focus();
  }, []);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        close(true);
      }
    };
    const onPointer = (e: PointerEvent) => {
      const target = e.target as Node;
      if (!popoverRef.current?.contains(target) && !buttonRef.current?.contains(target)) close(false);
    };
    window.addEventListener('keydown', onKey, true);
    window.addEventListener('pointerdown', onPointer);
    return () => {
      window.removeEventListener('keydown', onKey, true);
      window.removeEventListener('pointerdown', onPointer);
    };
  }, [open, close]);

  const setShortening = (enabled: boolean) => {
    setSaving(true);
    codeIntelApi
      .setShortening(conversationId, enabled)
      .then(next => {
        generation.current += 1;
        setView(next);
      })
      .catch(err => notify.error('Could not change output shortening', { description: errorText(err) }))
      .finally(() => setSaving(false));
  };

  if (!showsChip(view) || view === null) return null;

  return (
    <div className="relative shrink-0">
      <button
        ref={buttonRef}
        type="button"
        onClick={() => setOpen(o => !o)}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? popoverId : undefined}
        title={view.compressionOn ? `${sessionSentence(view)} (estimated)` : 'Command output reaches the model in full'}
        className={cn(
          'inline-flex items-center gap-1 h-[26px] px-2 rounded-full text-caption font-medium border border-shodh-border',
          'text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised transition-colors duration-micro tabular-nums',
          FOCUS_RING,
        )}
      >
        <Gauge className="w-3 h-3 shrink-0" aria-hidden="true" />
        <span className="whitespace-nowrap">{chipLabel(view)}</span>
      </button>
      {open && (
        <div
          ref={popoverRef}
          id={popoverId}
          role="dialog"
          aria-labelledby={headingId}
          className="absolute bottom-full left-0 mb-2 z-40 w-[340px] max-w-[calc(100vw-32px)] rounded-[14px] border border-shodh-border bg-shodh-raised p-3.5 shadow-[0_12px_40px_rgba(0,0,0,0.35)] flex flex-col gap-3"
        >
          <h2 id={headingId} className="m-0 text-small font-semibold text-shodh-text tabular-nums">
            {sessionSentence(view)}
          </h2>
          <p className="m-0 text-small text-shodh-text-secondary tabular-nums">{sessionDetail(view)}</p>
          <p className="m-0 text-small text-shodh-text-muted">
            Shodh removes passing-test lines, and after a successful run build progress, from what the model reads of a
            command&apos;s output. Errors, warnings and failing tests are kept, and the model can read the full output.
            You always see it.
          </p>
          <p className="m-0 text-caption text-shodh-text-faint">{view.method}</p>
          <SwitchRow
            label="Shorten command output"
            description="For the model, in this conversation. Applies from the next command."
            checked={view.compressionOn}
            disabled={saving}
            onChange={setShortening}
          />
          <button
            type="button"
            onClick={() => {
              close(false);
              onOpenActivity();
            }}
            className={cn('self-start text-small font-medium text-shodh-accent-text hover:underline rounded', FOCUS_RING)}
          >
            See the total in Activity
          </button>
        </div>
      )}
    </div>
  );
}

export default TokensSavedChip;

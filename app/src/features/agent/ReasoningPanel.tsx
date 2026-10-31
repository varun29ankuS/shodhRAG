import React, { useCallback, useId, useMemo, useSyncExternalStore } from 'react';
import { ChevronRight } from 'lucide-react';
import { cn } from '../../lib/utils';
import { reasoningSections } from './reducer';
import type { TranscriptState } from './reducer';
import { reasoningOpen } from './reasoningOpen';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

/**
 * The reasoning text the provider streamed with an answer, on demand: a
 * small "Reasoning" button at the right of the answer that expands a panel.
 * Shown only when the provider sent reasoning; never opened by itself. The
 * text is the model's own working, not checked against the sources, and
 * never part of the answer, its citations, its check or its exports.
 */
export function ReasoningPanel({ transcript, compact = false }: { transcript: TranscriptState; compact?: boolean }) {
  const sections = useMemo(() => reasoningSections(transcript), [transcript]);
  const runId = transcript.runId;
  const open = useSyncExternalStore(
    reasoningOpen.subscribe,
    () => reasoningOpen.isOpen(runId),
    () => false,
  );
  const toggle = useCallback(() => reasoningOpen.setOpen(runId, !reasoningOpen.isOpen(runId)), [runId]);
  const panelId = useId();
  if (sections.length === 0) return null;
  const truncated = transcript.reasoning?.truncated === true;

  return (
    <div className="flex flex-col gap-2" data-reasoning="">
      <div className="flex justify-end">
        <button
          type="button"
          onClick={toggle}
          aria-expanded={open}
          aria-controls={open ? panelId : undefined}
          className={cn(
            'inline-flex items-center gap-1 h-6 px-2 rounded-md text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised transition-colors duration-micro',
            compact ? 'text-caption' : 'text-caption',
            FOCUS_RING,
          )}
        >
          <ChevronRight
            className={cn('w-3 h-3 transition-transform duration-micro motion-reduce:transition-none', open && 'rotate-90')}
            aria-hidden="true"
          />
          Reasoning
        </button>
      </div>
      {open && (
        <section
          id={panelId}
          aria-label="The model's reasoning"
          className={cn(
            'max-h-80 overflow-y-auto rounded-lg border border-shodh-border-subtle bg-shodh-raised px-3 py-2.5 flex flex-col gap-2.5 text-shodh-text-secondary',
            compact ? 'text-caption' : 'text-small',
          )}
        >
          <p className="m-0 text-caption text-shodh-text-faint">
            The model&apos;s own reasoning as its provider sent it. It is not checked against your sources.
          </p>
          {sections.map((section, index) => (
            <div key={index} className="flex flex-col gap-1">
              <p className="m-0 text-caption font-medium text-shodh-text-muted">
                {section.afterStep ? `After: ${section.afterStep}` : 'Before any tool call'}
              </p>
              <p className="m-0 whitespace-pre-wrap break-words leading-relaxed">{section.text.trim()}</p>
            </div>
          ))}
          {truncated && (
            <p className="m-0 text-caption text-shodh-text-faint">Longer reasoning was cut here (about 32 KB is kept per answer).</p>
          )}
        </section>
      )}
    </div>
  );
}

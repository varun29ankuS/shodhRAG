import React, { useMemo } from 'react';
import { ChevronRight, CornerDownRight, Settings2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { MessageContentRenderer } from '../ask/MessageContentRenderer';
import type { SearchHit } from '../ask/types';
import { RuntimeCard } from './RuntimeCard';
import { Spinner, StepLine } from './StepLine';
import { currentStep, isLive } from './reducer';
import type { TranscriptState } from './reducer';
import { workFold } from './workSummary';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

interface TranscriptProps {
  transcript: TranscriptState;
  /** Citation targets built from the run's passages. */
  hits: readonly SearchHit[];
  activeCitation?: number | null;
  onOpenCitation: (hit: SearchHit, trigger: HTMLElement) => void;
  onDecide?: (stepId: string, approved: boolean) => void;
  onRuntimeInstalled: () => void;
  onOpenSettings: () => void;
  artifacts?: any[];
  onOpenArtifact?: (artifactId: string) => void;
  /** Dense variant for the conversation dock. */
  compact?: boolean;
}

/** A live "what is happening now" row with a spinner. */
function ActivityRow({ text, compact }: { text: string; compact: boolean }) {
  return (
    <div className={cn('ask-rise flex items-center gap-2 text-shodh-text-muted', compact ? 'text-[12.5px]' : 'text-[13.5px]')}>
      <Spinner />
      <span className="ask-breathe-soft">{text}</span>
    </div>
  );
}

/**
 * One agent turn as a live transcript: answer text (with citation pills),
 * tool steps with their results, steering messages, approvals, and the
 * run's outcome. Every row comes from a real event.
 */
export function Transcript({
  transcript,
  hits,
  activeCitation = null,
  onOpenCitation,
  onDecide,
  onRuntimeInstalled,
  onOpenSettings,
  artifacts,
  onOpenArtifact,
  compact = false,
}: TranscriptProps) {
  const live = isLive(transcript);
  const step = currentStep(transcript);
  const lastBlock = transcript.blocks[transcript.blocks.length - 1];
  const textBlocks = useMemo(() => transcript.blocks.filter(b => b.kind === 'text').length, [transcript.blocks]);
  let textSeen = 0;
  // A finished answer folds its working into one line so the answer starts at the top.
  const fold = useMemo(() => workFold(transcript), [transcript]);

  // While running with nothing in flight and no text streaming, the model is
  // working on its next move; say so instead of showing a frozen screen.
  let activity: string | null = null;
  if (transcript.status === 'starting') activity = 'Starting agent…';
  else if (transcript.status === 'running' && !step && (!lastBlock || lastBlock.kind !== 'text')) {
    activity = transcript.thinking.trim() ? 'Thinking…' : transcript.blocks.length === 0 ? 'Reading your question…' : 'Working…';
  }

  return (
    <div className={cn('flex flex-col min-w-0', compact ? 'gap-2.5' : 'gap-3.5')}>
      {fold && (
        <details className="group/work">
          <summary
            className={cn(
              'list-none [&::-webkit-details-marker]:hidden w-fit inline-flex items-center gap-1.5 rounded-md cursor-pointer select-none text-shodh-text-muted hover:text-shodh-text transition-colors duration-micro',
              compact ? 'text-[12px]' : 'text-[12.5px]',
              FOCUS_RING,
            )}
          >
            <ChevronRight className="w-3.5 h-3.5 transition-transform duration-micro group-open/work:rotate-90 motion-reduce:transition-none" aria-hidden="true" />
            {fold.label}
          </summary>
          <div className={cn('mt-2 pl-3 border-l border-shodh-border-subtle flex flex-col', compact ? 'gap-2' : 'gap-3')}>
            {transcript.blocks.slice(0, fold.answerIndex).map(block => {
              if (block.kind === 'step') {
                const s = transcript.steps[block.stepId];
                return s ? <StepLine key={block.stepId} step={s} steps={transcript.steps} compact={compact} /> : null;
              }
              if (block.kind === 'steer') {
                return (
                  <p key={block.id} className="flex items-start gap-2 text-[12.5px] text-shodh-text-secondary">
                    <CornerDownRight className="w-3.5 h-3.5 mt-[3px] shrink-0 text-shodh-accent-text" aria-hidden="true" />
                    <span className="min-w-0 break-words">
                      <span className="sr-only">You steered: </span>
                      {block.text}
                    </span>
                  </p>
                );
              }
              return (
                <MessageContentRenderer
                  key={block.id}
                  compact
                  content={block.text}
                  hits={hits}
                  activeCitation={activeCitation}
                  onOpenCitation={onOpenCitation}
                />
              );
            })}
          </div>
        </details>
      )}
      {transcript.blocks.map((block, index) => {
        if (fold && index < fold.answerIndex) {
          if (block.kind === 'text') textSeen += 1;
          return null;
        }
        if (block.kind === 'step') {
          const s = transcript.steps[block.stepId];
          return s ? (
            <StepLine key={block.stepId} step={s} steps={transcript.steps} onDecide={live ? onDecide : undefined} compact={compact} />
          ) : null;
        }
        if (block.kind === 'steer') {
          return (
            <p key={block.id} className={cn('ask-rise flex items-start gap-2 text-shodh-text-secondary', compact ? 'text-[12.5px]' : 'text-[13.5px]')}>
              <CornerDownRight className="w-3.5 h-3.5 mt-[3px] shrink-0 text-shodh-accent-text" aria-hidden="true" />
              <span className="min-w-0 break-words">
                <span className="sr-only">You steered: </span>
                {block.text}
              </span>
            </p>
          );
        }
        textSeen += 1;
        const isFinal = textSeen === textBlocks;
        return (
          <div key={block.id}>
            <MessageContentRenderer
              compact={compact}
              content={block.text}
              hits={hits}
              artifacts={!live && isFinal ? artifacts : undefined}
              activeCitation={activeCitation}
              onOpenCitation={onOpenCitation}
              onOpenArtifact={onOpenArtifact}
            />
          </div>
        );
      })}

      {activity && <ActivityRow text={activity} compact={compact} />}

      {transcript.status === 'aborted' && (
        <p className="text-[12.5px] text-shodh-text-muted">
          {transcript.error ?? (transcript.blocks.length === 0 ? 'You interrupted this before it started.' : 'Interrupted.')}
        </p>
      )}

      {transcript.status === 'error' && (transcript.errorCode === 'runtime_missing' || transcript.errorCode === 'runtime_invalid') && (
        <RuntimeCard
          reason={transcript.errorCode === 'runtime_missing' ? 'missing' : 'invalid'}
          message={transcript.error}
          onInstalled={onRuntimeInstalled}
          compact={compact}
        />
      )}

      {transcript.status === 'error' && transcript.errorCode !== 'runtime_missing' && transcript.errorCode !== 'runtime_invalid' && (
        <div role="alert" className="flex flex-col gap-2">
          <p className="text-[13px] leading-relaxed text-shodh-error break-words">
            {`The answer could not be completed: ${transcript.error ?? 'unknown error'}`}
          </p>
          {transcript.errorCode === 'model_config' && (
            <div>
              <button
                type="button"
                onClick={onOpenSettings}
                className={cn(
                  'inline-flex items-center gap-2 h-8 px-3 rounded-lg bg-shodh-raised-2 text-[12.5px] text-shodh-text hover:bg-shodh-pressed transition-colors duration-micro',
                  FOCUS_RING,
                )}
              >
                <Settings2 className="w-3.5 h-3.5" aria-hidden="true" />
                Open model settings
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export default Transcript;

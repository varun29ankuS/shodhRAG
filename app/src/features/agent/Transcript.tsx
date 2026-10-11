import React, { useMemo } from 'react';
import { ChevronRight, CornerDownRight, RefreshCw, Settings2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { MessageContentRenderer } from '../ask/MessageContentRenderer';
import type { SearchHit } from '../ask/types';
import { RuntimeCard } from './RuntimeCard';
import { Spinner, StepLine } from './StepLine';
import { currentStep, isLive, supersededBlocks } from './reducer';
import type { TranscriptBlock, TranscriptState } from './reducer';
import { workFold } from './workSummary';
import type { GroundingReport, RevisionReason } from './events';
import { answerReport, checksForMessage } from './grounding';
import { GroundingChip } from './GroundingFlags';
import { ProviderErrorCard } from '../modelPicker/ProviderErrorCard';
import type { ProviderErrorActions } from '../modelPicker/ProviderErrorCard';
import type { ProviderErrorKind } from '../modelPicker/modelTypes';

/** Why a fallback model answered, as the note under the answer says it. */
const FALLBACK_REASON: Record<ProviderErrorKind, string> = {
  rate_limited: 'rate-limited',
  quota_exhausted: 'out of credits or its daily limit',
  model_unavailable: 'unavailable',
  auth: 'refused (key rejected)',
  other: 'unable to answer',
};

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
  /** Retry and fallback for a provider failure (the newest answer in Ask only). */
  providerActions?: ProviderErrorActions | null;
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

/** What a follow-up turn is doing, in words. */
function revisionText(reason: RevisionReason, flagged: number, missingNeeds: readonly string[]): string {
  const fix = `Re-checking ${flagged} flagged ${flagged === 1 ? 'statement' : 'statements'} against the sources`;
  const search = missingNeeds.length === 1
    ? `Searching for a part no passage covered: ${missingNeeds[0]}`
    : `Searching for ${missingNeeds.length} parts no passage covered`;
  if (reason === 'repair') return fix;
  if (reason === 'coverage') return search;
  return `${fix}; ${search.charAt(0).toLowerCase()}${search.slice(1)}`;
}

function RevisionRow({ block, compact }: { block: Extract<TranscriptBlock, { kind: 'revision' }>; compact: boolean }) {
  return (
    <p className={cn('ask-rise flex items-start gap-2 text-shodh-text-muted', compact ? 'text-[12px]' : 'text-[12.5px]')}>
      <RefreshCw className="w-3.5 h-3.5 mt-[3px] shrink-0 text-shodh-accent-text" aria-hidden="true" />
      <span className="min-w-0 break-words">{revisionText(block.reason, block.flagged, block.missingNeeds)}</span>
    </p>
  );
}

/** The report whose claims come from `messageId` (a draft's own round). */
function reportFor(groundings: readonly GroundingReport[], messageId: string): GroundingReport | null {
  for (let i = groundings.length - 1; i >= 0; i--) {
    if (groundings[i].messageIds.includes(messageId)) return groundings[i];
  }
  return null;
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
  providerActions = null,
}: TranscriptProps) {
  const live = isLive(transcript);
  const step = currentStep(transcript);
  const lastBlock = transcript.blocks[transcript.blocks.length - 1];
  const superseded = useMemo(() => supersededBlocks(transcript), [transcript]);
  const textBlocks = useMemo(
    () => transcript.blocks.filter(b => b.kind === 'text' && !superseded.has(b.id)).length,
    [transcript.blocks, superseded],
  );
  const answer = useMemo(() => answerReport(transcript.groundings), [transcript.groundings]);
  // A replaced block after the answer is a follow-up reply that did not
  // replace it (e.g. "nothing more found"); one before it is an earlier draft.
  const answerBlockIndex = useMemo(() => {
    for (let i = transcript.blocks.length - 1; i >= 0; i--) {
      const b = transcript.blocks[i];
      if (b.kind === 'text' && !superseded.has(b.id)) return i;
    }
    return -1;
  }, [transcript.blocks, superseded]);
  let textSeen = 0;
  // A finished answer folds its working into one line so the answer starts at the top.
  const fold = useMemo(() => workFold(transcript), [transcript]);

  // While running with nothing in flight and no text streaming, the model is
  // working on its next move; say so instead of showing a frozen screen.
  let activity: string | null = null;
  if (transcript.status === 'starting') activity = 'Starting agent…';
  else if (transcript.status === 'running' && transcript.checking) activity = 'Checking the answer against its sources…';
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
              if (block.kind === 'revision') return <RevisionRow key={block.id} block={block} compact />;
              return (
                <TextBlock
                  key={block.id}
                  id={block.id}
                  text={block.text}
                  superseded={superseded.has(block.id)}
                  afterAnswer={answerBlockIndex >= 0 && transcript.blocks.indexOf(block) > answerBlockIndex}
                  groundings={transcript.groundings}
                  answer={answer}
                  hits={hits}
                  activeCitation={activeCitation}
                  onOpenCitation={onOpenCitation}
                  compact
                />
              );
            })}
          </div>
        </details>
      )}
      {transcript.blocks.map((block, index) => {
        if (fold && index < fold.answerIndex) {
          if (block.kind === 'text' && !superseded.has(block.id)) textSeen += 1;
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
        if (block.kind === 'revision') return <RevisionRow key={block.id} block={block} compact={compact} />;
        if (!superseded.has(block.id)) textSeen += 1;
        const isFinal = !superseded.has(block.id) && textSeen === textBlocks;
        return (
          <TextBlock
            key={block.id}
            id={block.id}
            text={block.text}
            superseded={superseded.has(block.id)}
            afterAnswer={answerBlockIndex >= 0 && index > answerBlockIndex}
            groundings={transcript.groundings}
            answer={answer}
            hits={hits}
            artifacts={!live && isFinal ? artifacts : undefined}
            activeCitation={activeCitation}
            onOpenCitation={onOpenCitation}
            onOpenArtifact={onOpenArtifact}
            compact={compact}
          />
        );
      })}

      {activity && <ActivityRow text={activity} compact={compact} />}

      {!live && answer && <GroundingChip report={answer} hits={hits} onOpenCitation={onOpenCitation} compact={compact} />}

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

      {transcript.fallback && (
        <p className="m-0 text-[12px] text-shodh-text-muted">
          {`Answered with a fallback model: ${transcript.fallback.from} was ${FALLBACK_REASON[transcript.fallback.kind]}${transcript.fallback.automatic ? ' (always fall back is on)' : ''}.`}
        </p>
      )}

      {transcript.status === 'error' && transcript.providerError && (
        <ProviderErrorCard
          error={transcript.providerError}
          runModel={transcript.model}
          message={transcript.error}
          fellBack={!!transcript.fallback}
          actions={providerActions}
          onOpenSettings={onOpenSettings}
          compact={compact}
        />
      )}

      {transcript.status === 'error' && !transcript.providerError && transcript.errorCode !== 'runtime_missing' && transcript.errorCode !== 'runtime_invalid' && (
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

interface TextBlockProps {
  id: string;
  text: string;
  /** Replaced by a revised answer: shown collapsed as an earlier draft. */
  superseded: boolean;
  /** A replaced block after the answer: a follow-up reply that kept the answer. */
  afterAnswer: boolean;
  groundings: readonly GroundingReport[];
  /** The report that describes the answer. */
  answer: GroundingReport | null;
  hits: readonly SearchHit[];
  artifacts?: any[];
  activeCitation: number | null;
  onOpenCitation: (hit: SearchHit, trigger: HTMLElement) => void;
  onOpenArtifact?: (artifactId: string) => void;
  compact: boolean;
}

/** One text block: the answer with its claim flags, or a collapsed earlier draft. */
function TextBlock({
  id,
  text,
  superseded,
  afterAnswer,
  groundings,
  answer,
  hits,
  artifacts,
  activeCitation,
  onOpenCitation,
  onOpenArtifact,
  compact,
}: TextBlockProps) {
  const report = superseded ? reportFor(groundings, id) : answer;
  const claims = useMemo(() => checksForMessage(report, id), [report, id]);
  const body = (
    <MessageContentRenderer
      compact={compact || superseded}
      content={text}
      hits={hits}
      artifacts={artifacts}
      activeCitation={activeCitation}
      onOpenCitation={onOpenCitation}
      onOpenArtifact={onOpenArtifact}
      claims={claims}
      flagUnknownCitations
    />
  );
  if (!superseded) return <div>{body}</div>;
  return (
    <details className="group/draft">
      <summary
        className={cn(
          'list-none [&::-webkit-details-marker]:hidden w-fit inline-flex items-center gap-1.5 rounded-md cursor-pointer select-none text-shodh-text-muted hover:text-shodh-text transition-colors duration-micro text-[12px]',
          FOCUS_RING,
        )}
      >
        <ChevronRight className="w-3.5 h-3.5 transition-transform duration-micro group-open/draft:rotate-90 motion-reduce:transition-none" aria-hidden="true" />
        {afterAnswer ? 'Follow-up reply; the answer above was kept' : 'Earlier draft, replaced after the grounding check'}
      </summary>
      <div className="mt-2 pl-3 border-l border-shodh-border-subtle opacity-80">{body}</div>
    </details>
  );
}

export default Transcript;

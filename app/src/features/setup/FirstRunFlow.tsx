import React, { useEffect, useId, useRef } from 'react';
import { AnimatePresence, motion } from 'framer-motion';
import { ArrowLeft, Check, CheckCircle2, FolderPlus, Loader2, Lock, MessageCircle, Quote, Search } from 'lucide-react';
import { useModelPicker } from '../modelPicker/modelApi';
import { ConnectPanel } from '../modelPicker/ConnectPanel';
import { ModelPicks } from '../modelPicker/ModelPicks';
import { cn } from '../../lib/utils';
import { ENTER_TRANSITION, EXIT_TRANSITION } from '../../lib/motion';
import { SearchSetupCard } from './SearchSetupCard';
import { useSearchModels } from './SearchModelsContext';
import { AnswerCheckModelRow } from '../../components/AnswerCheckSettings';
import { FIRST_RUN_STEPS, FIRST_RUN_STEP_LABELS, nextStep, previousStep, stepIndex } from './firstRun';
import type { FirstRunStep } from './firstRun';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const PRIMARY = cn(
  'h-9 px-4 inline-flex items-center gap-2 rounded-lg bg-shodh-accent text-shodh-on-accent text-[13px] font-semibold hover:bg-shodh-accent-hover disabled:opacity-60 transition-colors duration-micro',
  FOCUS_RING,
);

const QUIET = cn(
  'h-9 px-3 inline-flex items-center gap-1.5 rounded-lg text-[13px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
  FOCUS_RING,
);

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export interface FirstRunSource {
  id: string;
  name: string;
  status: string;
  fileCount: number;
  processedCount?: number;
  progress?: number;
}

interface FirstRunFlowProps {
  open: boolean;
  step: FirstRunStep;
  onStepChange: (step: FirstRunStep) => void;
  /** Setup finished (last step confirmed). */
  onFinish: () => void;
  /** Closed before the end; it can be resumed from the command palette. */
  onSkip: () => void;
  llmStatus: { connected: boolean; model: string; provider: string };
  sources: FirstRunSource[];
  onAddFolder: () => void;
}

/**
 * First-run setup over the (already usable) app: welcome, search models,
 * the model that answers, and a first folder. Every step can be skipped;
 * progress is saved, so closing it resumes at the same step later.
 */
export function FirstRunFlow(props: FirstRunFlowProps) {
  return (
    <AnimatePresence>
      {props.open && <FlowDialog key="first-run" {...props} />}
    </AnimatePresence>
  );
}

function FlowDialog({ step, onStepChange, onFinish, onSkip, llmStatus, sources, onAddFolder }: FirstRunFlowProps) {
  const titleId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const index = stepIndex(step);
  const wide = step === 'model';

  // Each step announces itself: focus its heading.
  useEffect(() => {
    headingRef.current?.focus();
  }, [step]);

  // Modal: Tab stays inside, Esc closes (resumable).
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !e.defaultPrevented) {
        e.preventDefault();
        onSkip();
        return;
      }
      if (e.key !== 'Tab' || !panelRef.current) return;
      const items = Array.from(panelRef.current.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(el => el.offsetParent !== null);
      if (items.length === 0) return;
      const first = items[0];
      const last = items[items.length - 1];
      if (e.shiftKey && (document.activeElement === first || document.activeElement === headingRef.current)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [onSkip]);

  const advance = () => (step === 'done' ? onFinish() : onStepChange(nextStep(step)));

  return (
    <div className="fixed inset-0 z-[9000] flex items-center justify-center p-6">
      <motion.div
        className="absolute inset-0 bg-black/55"
        initial={{ opacity: 0 }}
        animate={{ opacity: 1, transition: ENTER_TRANSITION }}
        exit={{ opacity: 0, transition: EXIT_TRANSITION }}
        aria-hidden="true"
      />
      <motion.div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className={cn(
          'relative w-full max-h-[min(720px,92vh)] flex flex-col rounded-2xl border border-shodh-border-strong bg-shodh-surface shadow-2xl overflow-hidden',
          'transition-[max-width] duration-panel ease-standard',
          wide ? 'max-w-3xl' : 'max-w-[560px]',
        )}
        initial={{ opacity: 0, y: 10, scale: 0.98 }}
        animate={{ opacity: 1, y: 0, scale: 1, transition: ENTER_TRANSITION }}
        exit={{ opacity: 0, y: 6, scale: 0.99, transition: EXIT_TRANSITION }}
      >
        {/* Progress */}
        <div className="shrink-0 px-6 pt-5 flex items-center justify-between gap-4">
          <ol className="flex items-center gap-1.5" aria-label="Setup steps">
            {FIRST_RUN_STEPS.map((s, i) => (
              <li key={s} className="flex items-center gap-1.5">
                <span
                  className={cn(
                    'h-1.5 rounded-full transition-[width,background-color] duration-panel ease-standard',
                    i === index ? 'w-6 bg-shodh-accent' : i < index ? 'w-1.5 bg-shodh-accent-text' : 'w-1.5 bg-shodh-border-strong',
                  )}
                  aria-hidden="true"
                />
                <span className="sr-only">
                  {`${FIRST_RUN_STEP_LABELS[s]}${i === index ? ' (current step)' : i < index ? ' (done)' : ''}`}
                </span>
              </li>
            ))}
          </ol>
          <span className="text-[11.5px] text-shodh-text-faint tabular-nums" aria-hidden="true">
            {`Step ${index + 1} of ${FIRST_RUN_STEPS.length}`}
          </span>
        </div>

        {/* Step */}
        <div key={step} className="shell-view-enter flex-1 min-h-0 overflow-y-auto scrollbar-thin px-6 pt-4 pb-2">
          {step === 'welcome' && <WelcomeStep titleId={titleId} headingRef={headingRef} />}
          {step === 'search' && <SearchStep titleId={titleId} headingRef={headingRef} />}
          {step === 'model' && (
            <ModelStep titleId={titleId} headingRef={headingRef} llmStatus={llmStatus} />
          )}
          {step === 'folder' && <FolderStep titleId={titleId} headingRef={headingRef} sources={sources} onAddFolder={onAddFolder} />}
          {step === 'done' && <DoneStep titleId={titleId} headingRef={headingRef} llmReady={llmStatus.connected} folders={sources.length} />}
        </div>

        {/* Footer */}
        <div className="shrink-0 px-6 py-4 border-t border-shodh-border-subtle flex items-center gap-2">
          {index > 0 && step !== 'done' && (
            <button type="button" onClick={() => onStepChange(previousStep(step))} className={QUIET}>
              <ArrowLeft className="w-3.5 h-3.5" aria-hidden="true" />
              Back
            </button>
          )}
          {step !== 'done' && (
            <button type="button" onClick={onSkip} className={cn(QUIET, 'mr-auto')}>
              Finish later
            </button>
          )}
          {step === 'done' && <span className="mr-auto" />}
          <button type="button" onClick={advance} className={PRIMARY}>
            {primaryLabel(step, llmStatus.connected, sources.length > 0)}
          </button>
        </div>
      </motion.div>
    </div>
  );
}

function primaryLabel(step: FirstRunStep, llmReady: boolean, hasFolder: boolean): string {
  switch (step) {
    case 'welcome':
      return 'Get started';
    case 'search':
      return 'Continue';
    case 'model':
      return llmReady ? 'Continue' : 'Skip for now';
    case 'folder':
      return hasFolder ? 'Continue' : 'Skip for now';
    case 'done':
      return 'Start asking';
  }
}

interface StepProps {
  titleId: string;
  headingRef: React.RefObject<HTMLHeadingElement | null>;
}

function StepHeading({ titleId, headingRef, children }: StepProps & { children: React.ReactNode }) {
  return (
    <h2 id={titleId} ref={headingRef} tabIndex={-1} className="text-[20px] font-bold text-shodh-text outline-none">
      {children}
    </h2>
  );
}

function WelcomeStep(props: StepProps) {
  const points = [
    { icon: Search, title: 'Search your own files', text: 'Folders on this computer are indexed locally. Nothing is uploaded.' },
    { icon: Quote, title: 'Answers with sources', text: 'Every answer cites the passage it came from, opened right in the app.' },
    { icon: Lock, title: 'You choose the model', text: 'Use a local model, or a provider with your own key. You decide what leaves this machine.' },
  ];
  return (
    <div className="flex flex-col gap-5">
      <div className="flex items-center gap-3">
        <img src="/shodh_logo_nobackground.svg" alt="" aria-hidden="true" className="w-10 h-10" />
        <div className="flex flex-col gap-0.5">
          <StepHeading {...props}>Welcome to Shodh</StepHeading>
          <p className="text-[13px] text-shodh-text-muted">Three short steps and you can ask questions about your documents.</p>
        </div>
      </div>
      <ul className="flex flex-col gap-2.5">
        {points.map(p => {
          const Icon = p.icon;
          return (
            <li key={p.title} className="flex items-start gap-3 p-3 rounded-xl bg-shodh-raised">
              <Icon className="w-[18px] h-[18px] mt-0.5 shrink-0 text-shodh-accent-text" aria-hidden="true" />
              <span className="flex flex-col gap-0.5">
                <span className="text-[13.5px] font-semibold text-shodh-text">{p.title}</span>
                <span className="text-[12.5px] text-shodh-text-muted">{p.text}</span>
              </span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function SearchStep(props: StepProps) {
  const { status, statusError, installing, installed } = useSearchModels();
  const ready = !!status?.ready && !installing;
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-col gap-1">
        <StepHeading {...props}>Set up search</StepHeading>
        <p className="text-[13px] text-shodh-text-muted">
          Search runs on this computer with two small models: one finds passages by meaning, the other ranks them.
        </p>
      </div>
      {!status && !statusError && (
        <p role="status" className="flex items-center gap-2 text-[13px] text-shodh-text-muted">
          <Loader2 className="w-4 h-4 agent-spin" aria-hidden="true" />
          Checking whether search is set up…
        </p>
      )}
      {ready && !installed && (
        <p role="status" className="flex items-center gap-2 p-3 rounded-xl bg-shodh-success-soft text-[13px] text-shodh-text">
          <CheckCircle2 className="w-[18px] h-[18px] text-shodh-success shrink-0" aria-hidden="true" />
          Search is set up and verified.
        </p>
      )}
      <SearchSetupCard />
      {installing && (
        <p className="text-[12px] text-shodh-text-faint">
          The download continues if you move on or close this window.
        </p>
      )}
      <section aria-label="Optional: answer checking" className="p-4 rounded-xl border border-shodh-border bg-shodh-surface flex flex-col gap-2">
        <p className="m-0 text-[12px] font-semibold uppercase tracking-[0.06em] text-shodh-text-faint">Optional</p>
        <AnswerCheckModelRow />
        <p className="m-0 text-[12px] text-shodh-text-faint">You can install it later in Settings → Search.</p>
      </section>
    </div>
  );
}

function ModelStep({ llmStatus, ...props }: StepProps & { llmStatus: FirstRunFlowProps['llmStatus'] }) {
  const picker = useModelPicker();
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-col gap-1">
        <StepHeading {...props}>Choose the model that answers</StepHeading>
        <p className="text-[13px] text-shodh-text-muted">
          Search finds the passages; a language model writes the answer from them. Connect one way below, then pick a model. You can change this any time in Settings.
        </p>
      </div>
      {llmStatus.connected && (
        <p role="status" className="flex items-center gap-2 p-3 rounded-xl bg-shodh-success-soft text-[13px] text-shodh-text">
          <CheckCircle2 className="w-[18px] h-[18px] text-shodh-success shrink-0" aria-hidden="true" />
          <span>
            {`Answers come from ${llmStatus.model}`}
            {llmStatus.provider && llmStatus.provider !== 'none' && ` via ${llmStatus.provider}`}.
          </span>
        </p>
      )}
      <div className="rounded-xl border border-shodh-border p-4">
        <ConnectPanel picker={picker} />
      </div>
      <div className="rounded-xl border border-shodh-border p-4 flex flex-col gap-2">
        <h3 className="m-0 text-[13.5px] font-semibold text-shodh-text">Model</h3>
        <ModelPicks picker={picker} />
      </div>
    </div>
  );
}

function FolderStep({ sources, onAddFolder, ...props }: StepProps & { sources: FirstRunSource[]; onAddFolder: () => void }) {
  const indexing = sources.find(s => s.status === 'indexing') ?? null;
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-col gap-1">
        <StepHeading {...props}>Add your first folder</StepHeading>
        <p className="text-[13px] text-shodh-text-muted">
          Pick a folder of documents: PDFs, Word, Excel, PowerPoint, text or code. Shodh indexes it and its subfolders on
          this computer.
        </p>
      </div>
      {sources.length > 0 && (
        <ul className="flex flex-col gap-1.5" aria-label="Folders in your Library">
          {sources.map(s => (
            <li key={s.id} className="flex items-center gap-2 px-3 py-2 rounded-lg bg-shodh-raised text-[13px]">
              {s.status === 'indexing' ? (
                <Loader2 className="w-4 h-4 agent-spin text-shodh-warning shrink-0" aria-hidden="true" />
              ) : (
                <Check className="w-4 h-4 text-shodh-success shrink-0" aria-hidden="true" />
              )}
              <span className="flex-1 truncate text-shodh-text">{s.name}</span>
              <span className="text-[12px] text-shodh-text-faint tabular-nums">
                {s.status === 'indexing'
                  ? s.fileCount > 0
                    ? `${(s.processedCount ?? 0).toLocaleString()} of ${s.fileCount.toLocaleString()}`
                    : 'Preparing…'
                  : `${s.fileCount.toLocaleString()} files`}
              </span>
            </li>
          ))}
        </ul>
      )}
      <div>
        <button type="button" onClick={onAddFolder} disabled={!!indexing} className={cn(PRIMARY, sources.length > 0 && 'bg-shodh-raised-2 text-shodh-text hover:bg-shodh-pressed')}>
          <FolderPlus className="w-4 h-4" aria-hidden="true" />
          {sources.length > 0 ? 'Add another folder' : 'Add folder'}
        </button>
      </div>
      {indexing && (
        <p className="text-[12px] text-shodh-text-faint">Indexing continues in the background; you can move on.</p>
      )}
    </div>
  );
}

function DoneStep({ llmReady, folders, ...props }: StepProps & { llmReady: boolean; folders: number }) {
  const missing = [
    !llmReady ? 'choose a model in Settings' : null,
    folders === 0 ? 'add a folder in the Library' : null,
  ].filter(Boolean);
  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center gap-3">
        <span className="w-10 h-10 rounded-xl bg-shodh-accent-soft inline-flex items-center justify-center" aria-hidden="true">
          <MessageCircle className="w-5 h-5 text-shodh-accent-text" />
        </span>
        <StepHeading {...props}>{missing.length === 0 ? 'You are ready' : 'Almost there'}</StepHeading>
      </div>
      <p className="text-[13px] text-shodh-text-muted">
        {missing.length === 0
          ? 'Ask anything about your files. Press Ctrl+K at any time to jump to a view, a chat or a folder.'
          : `To get answers from your files, ${missing.join(' and ')}. You can reopen this setup from the command palette (Ctrl+K).`}
      </p>
    </div>
  );
}

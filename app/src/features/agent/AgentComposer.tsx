import React, { forwardRef, useCallback, useImperativeHandle, useLayoutEffect, useRef } from 'react';
import { ArrowUp, CornerDownRight, Folder, ImagePlus, Mic, MicOff, Square } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useVoiceInput } from '../ask/useVoiceInput';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const MAX_TEXTAREA_HEIGHT = 200;
const MAX_COMPACT_HEIGHT = 120;

export interface AgentComposerHandle {
  focus: () => void;
}

interface AgentComposerProps {
  /** Distinguishes the Ask and dock inputs (label/description ids). */
  id: string;
  value: string;
  onChange: (value: string) => void;
  /** Send a new message (idle). */
  onSubmit: () => void;
  /** Redirect the running answer. */
  onSteer: () => void;
  /** Interrupt the running answer. */
  onStop: () => void;
  /**
   * Approve the pending step with Enter on an empty input. Leave unset for
   * destructive steps, which must be approved on the prompt itself.
   */
  onApprove?: () => void;
  /** An answer is running in this conversation. */
  running: boolean;
  /** The running answer accepts steering (it has started). */
  canSteer: boolean;
  /** A step is waiting for approval. */
  approvalPending?: boolean;
  /** Sending is blocked (e.g. an answer is running in another conversation). */
  blockedReason: string | null;
  placeholder: string;
  /** The model chip (Ask only): shows and changes the model the next answer uses. */
  modelChip?: React.ReactNode;
  scopeLabel?: string;
  scopeTitle?: string;
  onOpenLibrary?: () => void;
  /** A control next to the scope chip (e.g. "Search all my library" in a workspace). */
  scopeAction?: React.ReactNode;
  onPickImage?: () => void;
  autoFocus?: boolean;
  /** Dense variant for the conversation dock: no chips, smaller type. */
  compact?: boolean;
  /** Placeholder while an answer runs and cannot be steered from here. */
  runningPlaceholder?: string;
}

/**
 * Message input. Idle: Enter sends. While an answer runs: Enter steers it,
 * Esc interrupts it (handled by the hosting view), and with an approval
 * pending an empty Enter approves. Shift+Enter inserts a newline.
 */
export const AgentComposer = forwardRef<AgentComposerHandle, AgentComposerProps>(function AgentComposer(
  {
    id,
    value,
    onChange,
    onSubmit,
    onSteer,
    onStop,
    onApprove,
    running,
    canSteer,
    approvalPending = false,
    blockedReason,
    placeholder,
    modelChip,
    scopeLabel,
    scopeTitle,
    onOpenLibrary,
    scopeAction,
    onPickImage,
    autoFocus = false,
    compact = false,
    runningPlaceholder,
  },
  ref,
) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const valueRef = useRef(value);
  valueRef.current = value;
  const inputId = `${id}-input`;
  const blockedId = `${id}-blocked`;
  const maxHeight = compact ? MAX_COMPACT_HEIGHT : MAX_TEXTAREA_HEIGHT;

  useImperativeHandle(ref, () => ({ focus: () => textareaRef.current?.focus() }), []);

  const getText = useCallback(() => valueRef.current, []);
  const voice = useVoiceInput(getText, onChange);

  // Grow with content up to a cap, then scroll.
  useLayoutEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, maxHeight)}px`;
  }, [value, maxHeight]);

  const hasText = value.trim().length > 0;
  const canSend = hasText && !running && blockedReason === null;
  const canSteerNow = hasText && running && canSteer;

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key !== 'Enter' || e.shiftKey || e.nativeEvent.isComposing) return;
    e.preventDefault();
    if (running) {
      if (canSteerNow) onSteer();
      else if (!hasText && approvalPending && onApprove) onApprove();
    } else if (canSend) {
      onSubmit();
    }
  };

  const effectivePlaceholder = running
    ? approvalPending
      ? onApprove
        ? 'Enter to approve · Esc to deny · or type to steer…'
        : 'Approve or deny above · Esc to deny…'
      : !canSteer && runningPlaceholder
        ? runningPlaceholder
        : 'Steer the agent…'
    : placeholder;

  const iconButton = cn(
    'w-[34px] h-[34px] shrink-0 inline-flex items-center justify-center rounded-[10px] text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
    FOCUS_RING,
  );
  const chip = cn(
    'inline-flex items-center gap-[7px] h-[30px] px-2.5 rounded-full bg-shodh-raised-2 text-[12.5px] text-shodh-text-secondary hover:bg-shodh-pressed hover:text-shodh-text transition-colors duration-micro min-w-0',
    FOCUS_RING,
  );
  const roundButton = cn(
    'shrink-0 rounded-full inline-flex items-center justify-center bg-shodh-text text-shodh-ground hover:opacity-90 transition-opacity duration-micro disabled:bg-shodh-raised-2 disabled:text-shodh-text-faint disabled:cursor-not-allowed',
    compact ? 'w-8 h-8' : 'w-9 h-9',
    FOCUS_RING,
  );

  return (
    <div
      className={cn(
        'border border-shodh-border-strong bg-shodh-surface has-[textarea:focus-visible]:border-shodh-text-faint has-[textarea:focus-visible]:ring-1 has-[textarea:focus-visible]:ring-ring/60',
        compact ? 'rounded-[14px]' : 'rounded-[22px] shadow-[0_12px_40px_rgba(0,0,0,0.35)]',
      )}
    >
      <label htmlFor={inputId} className="sr-only">
        {running ? 'Steer the running answer' : 'Message Shodh'}
      </label>
      <textarea
        id={inputId}
        ref={textareaRef}
        rows={compact ? 1 : 2}
        value={value}
        onChange={e => onChange(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder={effectivePlaceholder}
        autoFocus={autoFocus}
        aria-describedby={blockedReason ? blockedId : undefined}
        className={cn(
          'block w-full resize-none border-0 bg-transparent text-shodh-text placeholder:text-shodh-text-faint outline-none focus:outline-none scrollbar-thin',
          compact ? 'px-3 pt-2.5 pb-0.5 text-[13.5px] leading-[1.45]' : 'px-[18px] pt-4 pb-1 text-[15px] leading-[1.5]',
        )}
        style={{ maxHeight }}
      />
      <div className={cn('flex flex-wrap items-center gap-x-1.5 gap-y-2 min-w-0', compact ? 'px-2 pt-1 pb-2' : 'px-2.5 pt-1.5 pb-2.5')}>
        {!compact && onPickImage && (
          <button
            type="button"
            onClick={onPickImage}
            aria-label="Add an image: extract its text and index it"
            title="Add an image (text is extracted and indexed)"
            className={iconButton}
          >
            <ImagePlus className="w-[17px] h-[17px]" aria-hidden="true" />
          </button>
        )}
        {!compact && voice.supported && (
          <button
            type="button"
            onClick={voice.toggle}
            aria-pressed={voice.listening}
            aria-label={voice.listening ? 'Stop dictation' : 'Dictate'}
            title={voice.listening ? 'Stop dictation' : 'Dictate'}
            className={cn(iconButton, voice.listening && 'text-shodh-error bg-shodh-raised')}
          >
            {voice.listening ? <MicOff className="w-[17px] h-[17px]" aria-hidden="true" /> : <Mic className="w-[17px] h-[17px]" aria-hidden="true" />}
          </button>
        )}
        {!compact && modelChip}
        {!compact && scopeLabel && onOpenLibrary && (
          <button
            type="button"
            onClick={onOpenLibrary}
            className={chip}
            title={scopeTitle}
            aria-label={scopeTitle ? `${scopeLabel}. ${scopeTitle}` : scopeLabel}
          >
            <Folder className="w-3 h-3 shrink-0" strokeWidth={2.2} aria-hidden="true" />
            <span className="truncate max-w-[200px]">{scopeLabel}</span>
          </button>
        )}
        {!compact && scopeAction}
        <div className="ml-auto flex items-center gap-1.5">
          {running && hasText && (
            <button
              type="button"
              onClick={onSteer}
              disabled={!canSteerNow}
              aria-label="Steer the answer (Enter)"
              title="Steer (Enter)"
              className={roundButton}
            >
              <CornerDownRight className="w-4 h-4" strokeWidth={2.4} aria-hidden="true" />
            </button>
          )}
          {running ? (
            <button type="button" onClick={onStop} aria-label="Interrupt the answer (Esc)" title="Interrupt (Esc)" className={roundButton}>
              <Square className="w-3.5 h-3.5" fill="currentColor" aria-hidden="true" />
            </button>
          ) : (
            <button type="button" onClick={onSubmit} disabled={!canSend} aria-label="Send" title="Send (Enter)" className={roundButton}>
              <ArrowUp className="w-4 h-4" strokeWidth={2.6} aria-hidden="true" />
            </button>
          )}
        </div>
      </div>
      {blockedReason && (
        <p id={blockedId} className={cn('-mt-1 text-[12px] text-shodh-text-muted', compact ? 'px-3 pb-2' : 'px-[18px] pb-3')}>
          {blockedReason}
        </p>
      )}
    </div>
  );
});

export default AgentComposer;

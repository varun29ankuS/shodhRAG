import React, { forwardRef, useImperativeHandle, useLayoutEffect, useRef } from 'react';
import { ArrowUp, CornerDownRight, ImagePlus, Mic, MicOff, Square } from 'lucide-react';
import { cn } from '../../lib/utils';
import { pasteDecision } from '../ask/imageNotice';
import { IS_MAC_PLATFORM, matchesShortcut, withShortcut } from '../../lib/shortcuts';
import { COMPOSER_ICON_CONTROL, COMPOSER_ROW_CLASS, COMPOSER_TEST_IDS } from './composerRow';

/** The engine grows the field with its content (no per-keystroke measuring). */
const NATIVE_FIELD_SIZING = typeof CSS !== 'undefined' && CSS.supports('field-sizing', 'content');
const MIN_COMPACT_HEIGHT = 32;
const MIN_TEXTAREA_HEIGHT = 66;

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const MAX_TEXTAREA_HEIGHT = 200;
const MAX_COMPACT_HEIGHT = 120;

export interface AgentComposerHandle {
  focus: () => void;
}

/** An image pasted into the composer: a file the webview exposes, or the system clipboard's bitmap. */
export type PastedImage = { kind: 'file'; file: File } | { kind: 'clipboard' };

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
  /** A one-click fix shown with `blockedReason`, e.g. "Turn on Code mode". */
  blockedAction?: { label: string; onClick: () => void; busy: boolean } | null;
  placeholder: string;
  /** The mode menu (`ModePicker`), third in the row. */
  modeControl?: React.ReactNode;
  /** The model control, fourth in the row: Ask's model chip, or the floating chat's trigger. */
  modelControl?: React.ReactNode;
  onPickImage?: () => void;
  /** An image (with no text) was pasted; the paste itself is cancelled. Text pastes are untouched. */
  onPasteImage?: (image: PastedImage) => void;
  /** Shown above the input: what goes out with the next question (image text chips). */
  attachments?: React.ReactNode;
  autoFocus?: boolean;
  /** Dense variant for the floating chat: the same row, smaller type. */
  compact?: boolean;
  /** Placeholder while an answer runs and cannot be steered from here. */
  runningPlaceholder?: string;
  /**
   * Shift+Tab in the empty input: switch to the next of Ask first, Accept edits and Plan
   * (the user's key, never the agent's; Auto is picked in the menu).
   */
  onCycleMode?: () => void;
  /**
   * Dictation, run by the caller (Ask runs `useVoiceInput` on its own draft; the floating
   * chat over the preview has the main window dictate). The microphone shows whenever it is
   * supported, compact or not; without it there is none.
   */
  voice?: { supported: boolean; listening: boolean; toggle: () => void };
}

/**
 * Message input: the text on top, and under it one row of controls, all one height, that
 * never wraps: `[+] [mic] [Mode] [Model] … [Send]`, Send pinned at the far right. Ask and
 * the floating chat both render it. Idle: Enter sends. While an answer runs: Enter steers
 * it, Esc interrupts it (handled by the hosting view), and with an approval pending an
 * empty Enter approves. Shift+Enter inserts a newline.
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
    blockedAction = null,
    placeholder,
    modeControl,
    modelControl,
    onPickImage,
    onPasteImage,
    attachments,
    autoFocus = false,
    compact = false,
    runningPlaceholder,
    onCycleMode,
    voice,
  },
  ref,
) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const inputId = `${id}-input`;
  const blockedId = `${id}-blocked`;
  const maxHeight = compact ? MAX_COMPACT_HEIGHT : MAX_TEXTAREA_HEIGHT;

  useImperativeHandle(ref, () => ({ focus: () => textareaRef.current?.focus() }), []);

  // Grow with content up to a cap, then scroll. Where the engine sizes the
  // field itself (CSS field-sizing) nothing is measured per keystroke;
  // measuring (height auto, then scrollHeight) forces a layout of the whole
  // page, which made typing lag in long conversations.
  useLayoutEffect(() => {
    if (NATIVE_FIELD_SIZING) return;
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, maxHeight)}px`;
  }, [value, maxHeight]);

  const hasText = value.trim().length > 0;
  const canSend = hasText && !running && blockedReason === null;
  const canSteerNow = hasText && running && canSteer;

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // Only in an empty message box: with text in it, Shift+Tab moves the focus back as
    // everywhere else.
    if (onCycleMode && value.trim() === '' && !e.nativeEvent.isComposing && matchesShortcut(e, 'cycleMode')) {
      e.preventDefault();
      onCycleMode();
      return;
    }
    if (e.key !== 'Enter' || e.shiftKey || e.nativeEvent.isComposing) return;
    e.preventDefault();
    if (running) {
      if (canSteerNow) onSteer();
      else if (!hasText && approvalPending && onApprove) onApprove();
    } else if (canSend) {
      onSubmit();
    }
  };

  // Decided while the event runs, so the default paste can still be cancelled.
  const handlePaste = (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    if (!onPasteImage) return;
    const files = Array.from(e.clipboardData.files);
    const decision = pasteDecision(e.clipboardData.getData('text/plain'), files.map(f => f.type));
    if (decision.kind === 'text') return;
    e.preventDefault();
    onPasteImage(decision.kind === 'file' ? { kind: 'file', file: files[decision.index] } : { kind: 'clipboard' });
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
    COMPOSER_ICON_CONTROL,
    'shrink-0 inline-flex items-center justify-center rounded-full text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
    FOCUS_RING,
  );
  const roundButton = cn(
    COMPOSER_ICON_CONTROL,
    'shrink-0 rounded-full inline-flex items-center justify-center bg-shodh-text text-shodh-ground hover:opacity-90 transition-opacity duration-micro disabled:bg-shodh-raised-2 disabled:text-shodh-text-faint disabled:cursor-not-allowed',
    FOCUS_RING,
  );

  return (
    <div
      className={cn(
        'border border-shodh-border-strong bg-shodh-surface has-[textarea:focus-visible]:border-shodh-text-faint has-[textarea:focus-visible]:ring-1 has-[textarea:focus-visible]:ring-ring/60',
        compact ? 'rounded-[14px]' : 'rounded-[22px] shadow-[0_12px_40px_rgba(0,0,0,0.35)]',
      )}
    >
      {/* Above the text, so a reason that comes and goes never moves the row of controls. */}
      {blockedReason && (
        <div className={cn('flex items-center gap-2 min-w-0', compact ? 'px-3 pt-2' : 'px-[18px] pt-3')}>
          <p id={blockedId} className="m-0 min-w-0 truncate text-caption text-shodh-text-muted" title={blockedReason}>
            {blockedReason}
          </p>
          {blockedAction && (
            <button
              type="button"
              onClick={blockedAction.onClick}
              disabled={blockedAction.busy}
              aria-busy={blockedAction.busy}
              className={cn(
                'shrink-0 whitespace-nowrap inline-flex items-center h-6 px-2.5 rounded-full text-caption font-medium bg-shodh-accent text-white hover:opacity-90 disabled:opacity-60 disabled:cursor-wait',
                FOCUS_RING,
              )}
            >
              {blockedAction.label}
            </button>
          )}
        </div>
      )}
      {attachments}
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
        onPaste={handlePaste}
        placeholder={effectivePlaceholder}
        autoFocus={autoFocus}
        aria-describedby={blockedReason ? blockedId : undefined}
        className={cn(
          'block w-full resize-none [field-sizing:content] border-0 bg-transparent text-shodh-text placeholder:text-shodh-text-faint outline-none focus:outline-none scrollbar-thin',
          compact ? 'px-3 pt-2.5 pb-0.5 text-body leading-[1.45]' : 'px-[18px] pt-4 pb-1 text-body-lg leading-[1.5]',
        )}
        style={NATIVE_FIELD_SIZING ? { maxHeight, minHeight: compact ? MIN_COMPACT_HEIGHT : MIN_TEXTAREA_HEIGHT } : { maxHeight }}
      />
      <div
        data-testid={COMPOSER_TEST_IDS.row}
        className={cn(COMPOSER_ROW_CLASS, 'box-content', compact ? 'px-2 pt-1 pb-2' : 'px-2.5 pt-1.5 pb-2.5')}
      >
        {onPickImage && (
          <button
            type="button"
            onClick={onPickImage}
            aria-label="Add an image"
            title="Add an image"
            data-testid={COMPOSER_TEST_IDS.image}
            className={iconButton}
          >
            <ImagePlus className="w-[17px] h-[17px]" aria-hidden="true" />
          </button>
        )}
        {voice?.supported && (
          <button
            type="button"
            onClick={voice.toggle}
            aria-pressed={voice.listening}
            aria-label={voice.listening ? 'Stop dictation' : 'Dictate'}
            title={voice.listening ? 'Stop dictation' : 'Dictate'}
            data-testid={COMPOSER_TEST_IDS.dictate}
            className={cn(iconButton, voice.listening && 'text-shodh-error bg-shodh-raised')}
          >
            {voice.listening ? <MicOff className="w-[17px] h-[17px]" aria-hidden="true" /> : <Mic className="w-[17px] h-[17px]" aria-hidden="true" />}
          </button>
        )}
        {/* Mode and model share what is left; their labels truncate, the row never wraps. */}
        {(modeControl || modelControl) && (
          <div className="flex flex-nowrap items-center gap-1.5 min-w-0 flex-1">
            {modeControl}
            {modelControl}
          </div>
        )}
        <div className="ml-auto shrink-0 flex flex-nowrap items-center gap-1.5">
          {running && hasText && (
            <button
              type="button"
              onClick={onSteer}
              disabled={!canSteerNow}
              aria-label={withShortcut('Steer the answer', 'steer', IS_MAC_PLATFORM)}
              title={withShortcut('Steer', 'steer', IS_MAC_PLATFORM)}
              data-testid={COMPOSER_TEST_IDS.steer}
              className={roundButton}
            >
              <CornerDownRight className="w-4 h-4" strokeWidth={2.4} aria-hidden="true" />
            </button>
          )}
          {running ? (
            <button type="button" onClick={onStop} aria-label={withShortcut('Interrupt the answer', 'interrupt', IS_MAC_PLATFORM)} title={withShortcut('Interrupt', 'interrupt', IS_MAC_PLATFORM)} data-testid={COMPOSER_TEST_IDS.stop} className={roundButton}>
              <Square className="w-3.5 h-3.5" fill="currentColor" aria-hidden="true" />
            </button>
          ) : (
            <button type="button" onClick={onSubmit} disabled={!canSend} aria-label="Send" title={withShortcut('Send', 'send', IS_MAC_PLATFORM)} data-testid={COMPOSER_TEST_IDS.send} className={roundButton}>
              <ArrowUp className="w-4 h-4" strokeWidth={2.6} aria-hidden="true" />
            </button>
          )}
        </div>
      </div>
    </div>
  );
});

export default AgentComposer;

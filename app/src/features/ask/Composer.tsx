import React, { forwardRef, useCallback, useImperativeHandle, useLayoutEffect, useRef } from 'react';
import { ArrowUp, Folder, ImagePlus, Mic, MicOff, Square } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useVoiceInput } from './useVoiceInput';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const MAX_TEXTAREA_HEIGHT = 200;

export interface ComposerHandle {
  focus: () => void;
}

interface ComposerProps {
  value: string;
  onChange: (value: string) => void;
  onSubmit: () => void;
  onStop: () => void;
  /** A request for this conversation is streaming. */
  streaming: boolean;
  /** Sending is blocked (e.g. a request is streaming in another conversation). */
  blockedReason: string | null;
  placeholder: string;
  modelLabel: string;
  modelConnected: boolean;
  onOpenModelSettings: () => void;
  scopeLabel: string;
  scopeTitle: string;
  onOpenLibrary: () => void;
  onPickImage?: () => void;
  autoFocus?: boolean;
}

/** Message composer: Enter sends, Shift+Enter inserts a newline. */
export const Composer = forwardRef<ComposerHandle, ComposerProps>(function Composer(
  {
    value,
    onChange,
    onSubmit,
    onStop,
    streaming,
    blockedReason,
    placeholder,
    modelLabel,
    modelConnected,
    onOpenModelSettings,
    scopeLabel,
    scopeTitle,
    onOpenLibrary,
    onPickImage,
    autoFocus = false,
  },
  ref,
) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const valueRef = useRef(value);
  valueRef.current = value;

  useImperativeHandle(ref, () => ({ focus: () => textareaRef.current?.focus() }), []);

  const getText = useCallback(() => valueRef.current, []);
  const voice = useVoiceInput(getText, onChange);

  // Grow with content up to a cap, then scroll.
  useLayoutEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, MAX_TEXTAREA_HEIGHT)}px`;
  }, [value]);

  const canSend = value.trim().length > 0 && !streaming && blockedReason === null;

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      if (canSend) onSubmit();
    }
  };

  const iconButton = cn(
    'w-[34px] h-[34px] shrink-0 inline-flex items-center justify-center rounded-[10px] text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
    FOCUS_RING,
  );
  const chip = cn(
    'inline-flex items-center gap-[7px] h-[30px] px-2.5 rounded-full bg-shodh-raised-2 text-[12.5px] text-shodh-text-secondary hover:bg-shodh-pressed hover:text-shodh-text transition-colors duration-micro min-w-0',
    FOCUS_RING,
  );

  return (
    <div className="rounded-[22px] border border-shodh-border-strong bg-shodh-surface shadow-[0_12px_40px_rgba(0,0,0,0.35)] has-[textarea:focus-visible]:border-shodh-text-faint has-[textarea:focus-visible]:ring-1 has-[textarea:focus-visible]:ring-ring/60">
      <label htmlFor="ask-composer-input" className="sr-only">Message Shodh</label>
      <textarea
        id="ask-composer-input"
        ref={textareaRef}
        rows={2}
        value={value}
        onChange={e => onChange(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder={placeholder}
        autoFocus={autoFocus}
        aria-describedby={blockedReason ? 'ask-composer-blocked' : undefined}
        className="block w-full resize-none border-0 bg-transparent px-[18px] pt-4 pb-1 text-[15px] leading-[1.5] text-shodh-text placeholder:text-shodh-text-faint outline-none focus:outline-none scrollbar-thin"
        style={{ maxHeight: MAX_TEXTAREA_HEIGHT }}
      />
      <div className="flex items-center gap-1.5 px-2.5 pt-1.5 pb-2.5 min-w-0">
        {onPickImage && (
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
        {voice.supported && (
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
        <button
          type="button"
          onClick={onOpenModelSettings}
          className={chip}
          title="Model settings"
          aria-label={`Model: ${modelLabel}. Open model settings`}
        >
          <span
            className={cn('w-[7px] h-[7px] rounded-full shrink-0', modelConnected ? 'bg-shodh-info' : 'bg-shodh-warning')}
            aria-hidden="true"
          />
          <span className="truncate max-w-[200px]">{modelLabel}</span>
        </button>
        <button
          type="button"
          onClick={onOpenLibrary}
          className={chip}
          title={scopeTitle}
          aria-label={`${scopeLabel}. ${scopeTitle}`}
        >
          <Folder className="w-3 h-3 shrink-0" strokeWidth={2.2} aria-hidden="true" />
          <span className="truncate max-w-[200px]">{scopeLabel}</span>
        </button>
        {streaming ? (
          <button
            type="button"
            onClick={onStop}
            aria-label="Stop answer (Esc)"
            title="Stop (Esc)"
            className={cn(
              'ml-auto w-9 h-9 shrink-0 rounded-full inline-flex items-center justify-center bg-shodh-text text-shodh-ground hover:opacity-90 transition-opacity duration-micro',
              FOCUS_RING,
            )}
          >
            <Square className="w-3.5 h-3.5" fill="currentColor" aria-hidden="true" />
          </button>
        ) : (
          <button
            type="button"
            onClick={onSubmit}
            disabled={!canSend}
            aria-label="Send"
            title="Send (Enter)"
            className={cn(
              'ml-auto w-9 h-9 shrink-0 rounded-full inline-flex items-center justify-center bg-shodh-text text-shodh-ground hover:opacity-90 transition-opacity duration-micro disabled:bg-shodh-raised-2 disabled:text-shodh-text-faint disabled:cursor-not-allowed',
              FOCUS_RING,
            )}
          >
            <ArrowUp className="w-4 h-4" strokeWidth={2.6} aria-hidden="true" />
          </button>
        )}
      </div>
      {blockedReason && (
        <p id="ask-composer-blocked" className="px-[18px] pb-3 -mt-1 text-[12px] text-shodh-text-muted">
          {blockedReason}
        </p>
      )}
    </div>
  );
});

export default Composer;

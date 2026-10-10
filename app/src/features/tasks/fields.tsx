import React, { useEffect, useId, useRef, useState } from 'react';
import { X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { dateInputValue, fromInputs, timeInputValue } from './dueDate';
import { addTags, removeTag, splitDraft } from './tags';

export const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

/** Borderless until hovered or focused: reads as text, edits in place. */
export const INLINE_INPUT = cn(
  'w-full rounded-lg border border-transparent bg-transparent px-2.5 text-shodh-text placeholder:text-shodh-text-faint',
  'hover:border-shodh-border focus:border-shodh-border-strong focus:bg-shodh-raised focus:outline-none',
  'transition-colors duration-micro',
);

export const FIELD_LABEL = 'text-[11.5px] font-medium text-shodh-text-muted';

/**
 * After Esc cancels a field edit, park focus on the enclosing dialog so the
 * next Esc closes it (and focus never falls back to the page behind).
 */
function parkFocus(el: HTMLElement) {
  const dialog = el.closest<HTMLElement>('[role="dialog"]');
  if (dialog) dialog.focus();
  else el.blur();
}

/**
 * Keeps a draft while focused; follows `value` otherwise (e.g. after a
 * refetch). `dirty` is set only by typing, so leaving a field the user did
 * not change never writes a stale draft over a newer value.
 */
function useDraft(value: string) {
  const [draft, setDraft] = useState(value);
  const focused = useRef(false);
  const cancelled = useRef(false);
  const dirty = useRef(false);
  useEffect(() => {
    if (!focused.current) setDraft(value);
  }, [value]);
  return { draft, setDraft, focused, cancelled, dirty };
}

/**
 * Text that saves on blur or Enter (Ctrl/Cmd+Enter when multiline) and
 * reverts on Esc. An empty required value reverts instead of saving.
 */
export function InlineText({
  value,
  onCommit,
  label,
  labelHidden = false,
  multiline = false,
  required = false,
  placeholder,
  className,
  hint,
  list,
}: {
  value: string;
  onCommit: (next: string) => void;
  label: string;
  labelHidden?: boolean;
  multiline?: boolean;
  required?: boolean;
  placeholder?: string;
  className?: string;
  hint?: string;
  /** Id of a `<datalist>` with suggestions. */
  list?: string;
}) {
  const id = useId();
  const hintId = useId();
  const { draft, setDraft, focused, cancelled, dirty } = useDraft(value);
  const ref = useRef<HTMLInputElement & HTMLTextAreaElement>(null);

  // Grow the notes box with its content.
  useEffect(() => {
    if (!multiline || !ref.current) return;
    ref.current.style.height = 'auto';
    ref.current.style.height = `${ref.current.scrollHeight + 2}px`;
  }, [draft, multiline]);

  const commit = () => {
    focused.current = false;
    const changed = dirty.current;
    dirty.current = false;
    if (cancelled.current || !changed) {
      cancelled.current = false;
      setDraft(value);
      return;
    }
    const next = multiline ? draft.replace(/\s+$/, '') : draft.trim();
    if (required && !next) {
      setDraft(value);
      return;
    }
    if (next !== value) onCommit(next);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement | HTMLTextAreaElement>) => {
    if (e.key === 'Escape') {
      cancelled.current = true;
      setDraft(value);
      parkFocus(e.currentTarget);
    } else if (e.key === 'Enter' && (!multiline || e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      e.currentTarget.blur();
    }
  };

  const common = {
    id,
    value: draft,
    placeholder,
    onFocus: () => { focused.current = true; },
    onChange: (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => {
      dirty.current = true;
      setDraft(e.target.value);
    },
    onBlur: commit,
    onKeyDown,
    'aria-describedby': hint ? hintId : undefined,
  };

  return (
    <div className="flex flex-col gap-1">
      <label htmlFor={id} className={labelHidden ? 'sr-only' : FIELD_LABEL}>{label}</label>
      {multiline ? (
        <textarea
          ref={ref}
          {...common}
          rows={3}
          className={cn(INLINE_INPUT, 'py-2 text-[13px] leading-relaxed resize-none min-h-[72px]', className)}
        />
      ) : (
        <input
          ref={ref}
          {...common}
          type="text"
          list={list}
          className={cn(INLINE_INPUT, 'h-8 text-[13px]', className)}
        />
      )}
      {hint && <p id={hintId} className="px-2.5 text-[11px] text-shodh-text-faint">{hint}</p>}
    </div>
  );
}

export interface SegmentOption {
  value: string;
  label: string;
  /** Tailwind classes for the selected state. */
  selectedClass?: string;
}

/** Radio group of buttons; arrow keys move and select, like native radios. */
export function Segmented({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string;
  options: SegmentOption[];
  onChange: (next: string) => void;
}) {
  const labelId = useId();
  const groupRef = useRef<HTMLDivElement>(null);
  const all = options.some(o => o.value === value) ? options : [...options, { value, label: value }];

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const step = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    const index = Math.max(0, all.findIndex(o => o.value === value));
    const next = all[(index + step + all.length) % all.length];
    onChange(next.value);
    groupRef.current?.querySelector<HTMLButtonElement>(`[data-value="${CSS.escape(next.value)}"]`)?.focus();
  };

  return (
    <div className="flex flex-col gap-1">
      <span id={labelId} className={FIELD_LABEL}>{label}</span>
      <div
        ref={groupRef}
        role="radiogroup"
        aria-labelledby={labelId}
        onKeyDown={onKeyDown}
        className="inline-flex self-start p-0.5 rounded-lg bg-shodh-raised border border-shodh-border"
      >
        {all.map(o => {
          const checked = o.value === value;
          return (
            <button
              key={o.value}
              type="button"
              role="radio"
              aria-checked={checked}
              tabIndex={checked ? 0 : -1}
              data-value={o.value}
              onClick={() => { if (!checked) onChange(o.value); }}
              className={cn(
                'h-7 px-3 rounded-md text-[12.5px] transition-colors duration-micro',
                checked
                  ? cn('bg-shodh-surface font-semibold shadow-sm', o.selectedClass ?? 'text-shodh-text')
                  : 'text-shodh-text-muted hover:text-shodh-text',
                FOCUS_RING,
              )}
            >
              {o.label}
            </button>
          );
        })}
      </div>
    </div>
  );
}

const DATE_INPUT = cn(
  'h-8 rounded-lg border border-shodh-border bg-shodh-surface px-2 text-[12.5px] text-shodh-text tabular-nums',
  'hover:border-shodh-border-strong focus:border-shodh-border-strong focus:outline-none focus-visible:ring-2 focus-visible:ring-ring',
  'disabled:opacity-50',
);

/**
 * Native date + optional time inputs for a stored date/time (see dueDate.ts).
 * Saves when focus leaves the pair or on Enter; Esc reverts. With no time
 * the value is date-only (unless `requireTime`). With `onClear`, emptying
 * the date (or the Remove button) clears the value; without it the value is
 * required and an emptied date reverts.
 */
export function DateTimeField({
  label,
  value,
  onCommit,
  onClear,
  allowTime = true,
  requireTime = false,
  emptyHint,
  validate,
}: {
  label: string;
  value: string | null | undefined;
  onCommit: (next: string) => void;
  /** Clear the value; omit for a required field. */
  onClear?: () => void;
  allowTime?: boolean;
  /** A time must be given (reminders). */
  requireTime?: boolean;
  emptyHint?: string;
  /** Returns an error message to reject the value. */
  validate?: (next: string) => string | null;
}) {
  const labelId = useId();
  const noteId = useId();
  const [date, setDate] = useState(dateInputValue(value));
  const [time, setTime] = useState(timeInputValue(value));
  const [note, setNote] = useState<string | null>(null);
  const groupRef = useRef<HTMLDivElement>(null);
  const timeRef = useRef<HTMLInputElement>(null);
  const editing = useRef(false);
  const cancelled = useRef(false);
  const dirty = useRef(false);

  useEffect(() => {
    if (editing.current) return;
    setDate(dateInputValue(value));
    setTime(timeInputValue(value));
  }, [value]);

  const revert = () => {
    setDate(dateInputValue(value));
    setTime(timeInputValue(value));
  };

  const commit = (nextDate: string, nextTime: string) => {
    if (!nextDate) {
      if (value && onClear) {
        setNote(null);
        onClear();
        return;
      }
      revert();
      if (value) setNote(`${label} is required.`);
      return;
    }
    if (requireTime && !nextTime) {
      // Wait for the time: moving from the date to the time input is not a commit.
      setNote('Pick a time as well.');
      return;
    }
    const next = fromInputs(nextDate, allowTime ? nextTime : '');
    if (!next) {
      revert();
      return;
    }
    const problem = validate?.(next) ?? null;
    if (problem) {
      revert();
      setNote(problem);
      return;
    }
    setNote(null);
    onCommit(next);
  };

  // Save when focus leaves the date/time pair, not when moving between them.
  const onBlur = (e: React.FocusEvent<HTMLDivElement>) => {
    if (groupRef.current?.contains(e.relatedTarget as Node | null)) return;
    editing.current = false;
    const changed = dirty.current;
    dirty.current = false;
    if (cancelled.current || !changed) {
      cancelled.current = false;
      revert();
      return;
    }
    commit(date, time);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      cancelled.current = true;
      revert();
      setNote(null);
      parkFocus(e.target as HTMLElement);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      (e.target as HTMLElement).blur();
    }
  };

  return (
    <div className="flex flex-col gap-1">
      <span id={labelId} className={FIELD_LABEL}>{label}</span>
      <div
        ref={groupRef}
        role="group"
        aria-labelledby={labelId}
        aria-describedby={note || (!value && emptyHint) ? noteId : undefined}
        onFocus={() => { editing.current = true; }}
        onBlur={onBlur}
        onKeyDown={onKeyDown}
        className="flex items-center gap-2 flex-wrap"
      >
        <input
          type="date"
          aria-label={`${label} date`}
          value={date}
          onChange={e => { dirty.current = true; setDate(e.target.value); }}
          className={DATE_INPUT}
        />
        {allowTime && (
          <>
            <input
              ref={timeRef}
              type="time"
              aria-label={`${label} time (optional)`}
              value={time}
              disabled={!date}
              onChange={e => { dirty.current = true; setTime(e.target.value); }}
              className={DATE_INPUT}
            />
            {time && !requireTime && (
              <button
                type="button"
                onClick={() => {
                  // Removing the time makes the value date-only right away;
                  // focus moves to the time input because this button goes away.
                  setTime('');
                  dirty.current = false;
                  if (date) commit(date, '');
                  timeRef.current?.focus();
                }}
                className={cn('h-8 px-2 rounded-lg text-[12px] text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro', FOCUS_RING)}
              >
                No time
              </button>
            )}
          </>
        )}
        {value && onClear && (
          <button
            type="button"
            aria-label={`Remove ${label.toLowerCase()}`}
            onClick={() => {
              dirty.current = false;
              setNote(null);
              onClear();
            }}
            className={cn('h-8 px-2 rounded-lg text-[12px] text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro', FOCUS_RING)}
          >
            Remove
          </button>
        )}
      </div>
      {(note || (!value && emptyHint)) && (
        <p id={noteId} role={note ? 'status' : undefined} className="text-[11px] text-shodh-text-faint">
          {note ?? emptyHint}
        </p>
      )}
    </div>
  );
}

/**
 * Tags as removable chips. Comma, semicolon or Enter completes a tag;
 * Backspace in an empty input removes the last chip; Esc discards the draft.
 */
export function ChipInput({
  label,
  tags,
  onChange,
}: {
  label: string;
  tags: string[];
  onChange: (next: string[]) => void;
}) {
  const id = useId();
  const [draft, setDraft] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);
  const discard = useRef(false);

  const add = (text: string) => {
    const next = addTags(tags, text);
    if (next.length !== tags.length) onChange(next);
  };

  return (
    <div className="flex flex-col gap-1">
      <label htmlFor={id} className={FIELD_LABEL}>{label}</label>
      <div
        className="flex flex-wrap items-center gap-1.5 min-h-8 px-1.5 py-1 rounded-lg border border-transparent hover:border-shodh-border focus-within:border-shodh-border-strong focus-within:bg-shodh-raised transition-colors duration-micro"
        onClick={e => { if (e.target === e.currentTarget) inputRef.current?.focus(); }}
      >
        <ul className="contents" aria-label={`${label}: ${tags.length === 0 ? 'none' : tags.join(', ')}`}>
          {tags.map(tag => (
            <li key={tag} className="inline-flex items-center gap-1 h-6 pl-2 pr-1 rounded-md bg-shodh-raised-2 text-[12px] text-shodh-text-secondary">
              {tag}
              <button
                type="button"
                aria-label={`Remove tag ${tag}`}
                onClick={() => onChange(removeTag(tags, tag))}
                className={cn('w-4 h-4 rounded inline-flex items-center justify-center text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-pressed', FOCUS_RING)}
              >
                <X className="w-3 h-3" aria-hidden="true" />
              </button>
            </li>
          ))}
        </ul>
        <input
          ref={inputRef}
          id={id}
          type="text"
          value={draft}
          placeholder={tags.length === 0 ? 'Add tags, separated by commas' : 'Add tag'}
          onChange={e => {
            const { complete, draft: rest } = splitDraft(e.target.value);
            if (complete) add(complete);
            setDraft(rest);
          }}
          onKeyDown={e => {
            if (e.key === 'Enter') {
              e.preventDefault();
              if (draft.trim()) {
                add(draft);
                setDraft('');
              }
            } else if (e.key === 'Backspace' && draft === '' && tags.length > 0) {
              e.preventDefault();
              onChange(tags.slice(0, -1));
            } else if (e.key === 'Escape') {
              discard.current = true;
              setDraft('');
              parkFocus(e.currentTarget);
            }
          }}
          onBlur={() => {
            if (!discard.current && draft.trim()) add(draft);
            discard.current = false;
            setDraft('');
          }}
          className="flex-1 min-w-[8rem] h-6 bg-transparent px-1 text-[12.5px] text-shodh-text placeholder:text-shodh-text-faint focus:outline-none"
        />
      </div>
    </div>
  );
}

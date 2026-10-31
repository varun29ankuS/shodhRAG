import React, { useId } from 'react';
import { cn } from '../lib/utils';
import { Kbd } from './ui/Kbd';
import { TEXT_SIZES, TEXT_SIZE_LABELS } from '../lib/textSize';
import { useTextSize } from '../hooks/useTextSize';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

/**
 * Settings → Appearance → Text size: Small, Default, Large or Larger, for the
 * whole interface. It scales on top of the operating system's display scaling,
 * and Ctrl + / Ctrl − / Ctrl 0 change it from anywhere.
 */
export default function AppearanceSettings() {
  const { size, setSize, error } = useTextSize();
  const labelId = useId();
  const hintId = useId();
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-0.5">
        <p id={labelId} className="m-0 text-body font-semibold text-shodh-text">Text size</p>
        <p id={hintId} className="m-0 text-small text-shodh-text-muted">
          How large everything in Shodh is drawn, on top of your display&apos;s own scaling.
        </p>
      </div>
      <div
        role="radiogroup"
        aria-labelledby={labelId}
        aria-describedby={hintId}
        className="inline-flex w-fit flex-wrap gap-1 p-1 rounded-xl bg-shodh-raised-2"
        onKeyDown={e => {
          // A radio group: the arrows move the choice (and the focus with it).
          const step = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0;
          if (step === 0) return;
          e.preventDefault();
          const i = TEXT_SIZES.indexOf(size);
          const next = TEXT_SIZES[(i + step + TEXT_SIZES.length) % TEXT_SIZES.length];
          setSize(next);
          e.currentTarget.querySelector<HTMLButtonElement>(`[data-size="${next}"]`)?.focus();
        }}
      >
        {TEXT_SIZES.map(option => {
          const selected = option === size;
          return (
            <button
              key={option}
              type="button"
              role="radio"
              aria-checked={selected}
              data-size={option}
              tabIndex={selected ? 0 : -1}
              onClick={() => { if (!selected) setSize(option); }}
              className={cn(
                'h-8 px-3.5 rounded-lg text-small font-medium transition-colors duration-micro',
                selected ? 'bg-shodh-surface text-shodh-text shadow-sm' : 'text-shodh-text-muted hover:text-shodh-text',
                FOCUS_RING,
              )}
            >
              {TEXT_SIZE_LABELS[option]}
            </button>
          );
        })}
      </div>
      <p className="m-0 flex flex-wrap items-center gap-x-4 gap-y-1 text-small text-shodh-text-secondary">
        <span className="inline-flex items-center gap-1.5"><Kbd shortcut="textLarger" /> Larger</span>
        <span className="inline-flex items-center gap-1.5"><Kbd shortcut="textSmaller" /> Smaller</span>
        <span className="inline-flex items-center gap-1.5"><Kbd shortcut="textReset" /> Default</span>
      </p>
      {error && <p role="alert" className="m-0 text-small text-shodh-error">{error}</p>}
    </div>
  );
}

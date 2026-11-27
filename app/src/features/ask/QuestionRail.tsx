import React, { useEffect, useId, useRef, useState } from 'react';
import { cn } from '../../lib/utils';
import { nextMenuIndex } from '../../lib/menuKeys';
import type { RailQuestion } from './questionDots';
import { keepInView } from './questionDots';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-ground';

/** One dot's height (its click target): the rail is a column of these. */
const DOT_PX = 24;

export interface QuestionRailProps {
  questions: readonly RailQuestion[];
  /** The position in `questions` of the question in view. */
  current: number;
  /** Brings the message at `index` into view. */
  onJump: (index: number) => void;
  /** Where the rail sits (AskView places it in the conversation's right gutter). */
  style: React.CSSProperties;
}

/**
 * The question dots rail: one dot per question at the conversation's right edge, the
 * dot of the question in view filled. Hover or focus shows the question's first line;
 * a click or Enter jumps to it. One Tab stop; the arrows, Home and End move between
 * dots as in a menu (`menuKeys.ts`). Shown from three questions (`questionDots.ts`).
 */
export function QuestionRail({ questions, current, onJump, style }: QuestionRailProps) {
  const listRef = useRef<HTMLOListElement>(null);
  const navRef = useRef<HTMLElement>(null);
  const labelId = useId();
  // The dot whose question is shown beside it, and where (from the rail's top).
  const [hint, setHint] = useState<{ at: number; top: number } | null>(null);
  // The dot Tab lands on: the one in view until the arrows move away from it.
  const [focusAt, setFocusAt] = useState<number | null>(null);
  const tabStop = focusAt ?? Math.max(0, current);

  // The filled dot stays in view of a rail too long for the window.
  useEffect(() => {
    const list = listRef.current;
    if (!list || current < 0) return;
    list.scrollTop = keepInView(list.scrollTop, list.clientHeight, current * DOT_PX, DOT_PX);
  }, [current, questions.length]);

  const show = (at: number, el: HTMLElement) => {
    const nav = navRef.current;
    if (!nav) return;
    const top = el.getBoundingClientRect().top - nav.getBoundingClientRect().top + DOT_PX / 2;
    setHint({ at, top });
  };

  const dots = () => Array.from(listRef.current?.querySelectorAll<HTMLButtonElement>('button[data-rail-dot]') ?? []);

  const onKeyDown = (e: React.KeyboardEvent<HTMLElement>) => {
    const all = dots();
    const at = all.indexOf(document.activeElement as HTMLButtonElement);
    const next = nextMenuIndex(e.key, at, all.length);
    if (next === null) return;
    e.preventDefault();
    setFocusAt(next);
    all[next]?.focus();
  };

  const hinted = hint ? questions[hint.at] : null;

  return (
    <nav
      ref={navRef}
      aria-label="Questions"
      className="absolute z-10 flex flex-col justify-center"
      style={style}
      onKeyDown={onKeyDown}
      onBlur={e => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
          setFocusAt(null);
          setHint(null);
        }
      }}
      onPointerLeave={() => setHint(null)}
    >
      <ol
        ref={listRef}
        onScroll={() => setHint(null)}
        className="m-0 p-0 list-none flex flex-col overflow-y-auto scrollbar-none max-h-full"
      >
        {questions.map((question, at) => {
          const filled = at === current;
          return (
            <li key={question.index} className="shrink-0">
              <button
                type="button"
                data-rail-dot=""
                tabIndex={at === tabStop ? 0 : -1}
                aria-label={`Question ${at + 1}: ${question.label}`}
                aria-current={filled ? 'location' : undefined}
                aria-describedby={hint?.at === at ? labelId : undefined}
                onClick={() => onJump(question.index)}
                onPointerEnter={e => show(at, e.currentTarget)}
                onFocus={e => {
                  setFocusAt(at);
                  show(at, e.currentTarget);
                }}
                className={cn('group w-6 h-6 rounded-full inline-flex items-center justify-center', FOCUS_RING)}
              >
                <span
                  aria-hidden="true"
                  className={cn(
                    'block w-2 h-2 rounded-full border transition-colors duration-micro',
                    filled
                      ? 'bg-shodh-accent border-shodh-accent'
                      : 'bg-shodh-ground border-shodh-border-strong group-hover:bg-shodh-text-faint group-hover:border-shodh-text-faint',
                  )}
                />
              </button>
            </li>
          );
        })}
      </ol>
      {hint && hinted && (
        <span
          id={labelId}
          role="tooltip"
          className="pointer-events-none absolute end-full me-2 w-max max-w-[320px] -translate-y-1/2 rounded-lg border border-shodh-border bg-shodh-surface px-2.5 py-1.5 text-small text-shodh-text shadow-md"
          style={{ top: hint.top }}
        >
          {hinted.label}
        </span>
      )}
    </nav>
  );
}

export default QuestionRail;

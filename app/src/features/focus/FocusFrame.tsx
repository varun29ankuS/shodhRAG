import React, { useCallback, useRef } from 'react';
import { Maximize2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import type { FocusTarget } from './focusTypes';
import { useFocus, useFocusAnchor } from './FocusContext';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

export interface FocusFrameProps {
  /** Short name used in the button label, e.g. "diagram". */
  noun: string;
  /** Builds the target when opened (may read the rendered element). */
  getTarget: (element: HTMLElement) => FocusTarget | null;
  /** Double-click opens too. Off where double-click selects text (tables). */
  doubleClick?: boolean;
  className?: string;
  children: React.ReactNode;
}

/**
 * Gives a visual in an answer its way into the focus pop-out: an Expand
 * button shown on hover and keyboard focus (always reachable with Tab;
 * Enter opens), plus double-click. Renders the visual unchanged outside an
 * answer (no anchor), e.g. inside the pop-out's own side discussion.
 */
export function FocusFrame({ noun, getTarget, doubleClick = true, className, children }: FocusFrameProps) {
  const focus = useFocus();
  const anchor = useFocusAnchor();
  const frameRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);

  const open = useCallback((trigger: HTMLElement | null) => {
    const el = frameRef.current;
    if (!focus || !anchor || !el) return;
    const target = getTarget(el);
    if (!target) return;
    focus.openFocus({
      target,
      conversationId: anchor.conversationId,
      parentMessageId: anchor.messageId,
      trigger: trigger ?? buttonRef.current,
    });
  }, [focus, anchor, getTarget]);

  if (!focus || !anchor) return <>{children}</>;

  const onDoubleClick = (e: React.MouseEvent<HTMLDivElement>) => {
    if (e.target instanceof Element && e.target.closest('button, a, input, textarea, select')) return;
    e.preventDefault();
    window.getSelection()?.removeAllRanges();
    open(buttonRef.current);
  };

  return (
    <div ref={frameRef} className={cn('group/focus relative', className)} onDoubleClick={doubleClick ? onDoubleClick : undefined}>
      {children}
      <button
        ref={buttonRef}
        type="button"
        onClick={e => open(e.currentTarget)}
        aria-label={`Expand ${noun}: zoom and ask about it`}
        title={doubleClick ? 'Expand (or double-click): zoom and ask about it' : 'Expand: zoom and ask about it'}
        className={cn(
          'absolute top-2 right-2 z-10 h-7 px-2 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border bg-shodh-surface/95 text-[11.5px] font-medium text-shodh-text-secondary shadow-sm',
          'opacity-0 group-hover/focus:opacity-100 focus-visible:opacity-100 [@media(hover:none)]:opacity-100 hover:text-shodh-text hover:bg-shodh-raised transition-opacity duration-micro',
          FOCUS_RING,
        )}
      >
        <Maximize2 className="w-3.5 h-3.5" aria-hidden="true" />
        Expand
      </button>
    </div>
  );
}

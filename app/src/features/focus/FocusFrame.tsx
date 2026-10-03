import React, { useCallback, useRef } from 'react';
import { MessageSquareText } from 'lucide-react';
import { cn } from '../../lib/utils';
import type { FocusTarget } from './focusTypes';
import { notifyDepthLimit, useFocus, useFocusAnchor, useFocusDrill } from './FocusContext';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

export interface FocusFrameProps {
  /** Short name used in the button label, e.g. "diagram". */
  noun: string;
  /** Builds the target when opened (may read the rendered element). */
  getTarget: (element: HTMLElement) => FocusTarget | null;
  /** Double-click opens too. Off where double-click selects text (tables). */
  doubleClick?: boolean;
  /** Inside running text (images in a paragraph): renders as an inline block. */
  inline?: boolean;
  className?: string;
  children: React.ReactNode;
}

/**
 * Gives a visual in an answer its way into the focus pop-out: an always
 * visible "Expand & ask" button (Tab reaches it, Enter opens), plus
 * double-click. It stays subdued until hovered so it does not compete
 * with the visual. Inside a finished side answer it opens the visual as a
 * nested level of the pop-out. Renders the visual unchanged anywhere else
 * (no anchor), e.g. a side answer still being written.
 */
export function FocusFrame({ noun, getTarget, doubleClick = true, inline = false, className, children }: FocusFrameProps) {
  const focus = useFocus();
  const drill = useFocusDrill();
  const answerAnchor = useFocusAnchor();
  // Inside a side answer the drill context wins: the pop-out's own levels.
  const anchor = drill ? null : answerAnchor;
  const frameRef = useRef<HTMLElement>(null);
  const Wrapper = inline ? 'span' : 'div';
  const buttonRef = useRef<HTMLButtonElement>(null);

  const open = useCallback((trigger: HTMLElement | null) => {
    const el = frameRef.current;
    if (!focus || (!anchor && !drill) || !el) return;
    const target = getTarget(el);
    if (!target) return;
    if (drill) {
      const result = focus.drillDown({ target, parentThreadId: drill.parentThreadId, parentTurnId: drill.parentTurnId });
      if (result === 'depth') notifyDepthLimit();
      return;
    }
    if (!anchor) return;
    focus.openFocus({
      target,
      conversationId: anchor.conversationId,
      parentMessageId: anchor.messageId,
      trigger: trigger ?? buttonRef.current,
    });
  }, [focus, anchor, drill, getTarget]);

  if (!focus || (!anchor && !drill)) return <Wrapper className={cn(inline && 'inline-block', className)}>{children}</Wrapper>;

  const onDoubleClick = (e: React.MouseEvent<HTMLElement>) => {
    if (e.target instanceof Element && e.target.closest('button, a, input, textarea, select')) return;
    e.preventDefault();
    window.getSelection()?.removeAllRanges();
    open(buttonRef.current);
  };

  return (
    <Wrapper
      ref={frameRef as React.Ref<HTMLDivElement & HTMLSpanElement>}
      className={cn('group/focus relative', inline && 'inline-block', className)}
      onDoubleClick={doubleClick ? onDoubleClick : undefined}
    >
      {children}
      <button
        ref={buttonRef}
        type="button"
        onClick={e => open(e.currentTarget)}
        aria-label={`Expand ${noun}: zoom and ask about it`}
        title={doubleClick ? 'Expand (or double-click): zoom and ask about it' : 'Expand: zoom and ask about it'}
        className={cn(
          'absolute top-2 right-2 z-10 h-7 px-2 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border bg-shodh-surface/95 text-[11.5px] font-medium text-shodh-text-secondary shadow-sm',
          'opacity-75 group-hover/focus:opacity-100 focus-visible:opacity-100 hover:text-shodh-text hover:bg-shodh-raised transition-opacity duration-micro',
          FOCUS_RING,
        )}
      >
        <MessageSquareText className="w-3.5 h-3.5" aria-hidden="true" />
        Expand &amp; ask
      </button>
    </Wrapper>
  );
}

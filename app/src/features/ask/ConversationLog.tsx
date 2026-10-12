import React, { forwardRef, useCallback, useEffect, useImperativeHandle, useLayoutEffect, useRef } from 'react';
import { defaultRangeExtractor, useVirtualizer } from '@tanstack/react-virtual';
import type { Range } from '@tanstack/react-virtual';
import {
  MESSAGE_GAP_PX,
  distanceFromBottom,
  estimateMessageHeight,
  followsOutput,
  mountedIndexes,
} from './conversationWindow';
import type { ChatMessage } from './types';

import { CONVERSATION_TOP_PX, MESSAGE_INDEX_ATTR } from './windowLayout';

/** Messages rendered beyond the visible ones, above and below. */
const OVERSCAN_MESSAGES = 3;

/** Frames to wait for a message brought into view to mount. */
const REVEAL_MAX_FRAMES = 90;

/** Estimates per message object (a streamed answer is a new object per frame). */
const heightEstimates = new WeakMap<ChatMessage, number>();
function estimatedHeight(message: ChatMessage): number {
  let height = heightEstimates.get(message);
  if (height === undefined) {
    height = estimateMessageHeight(message);
    heightEstimates.set(message, height);
  }
  return height;
}

/**
 * Rises into place when first shown. Decided once per mount, so a message
 * scrolled out of the window and back is not animated again.
 */
function RiseOnce({ animate, children }: { animate: boolean; children: React.ReactNode }) {
  const [rise] = React.useState(animate);
  return <div className={rise ? 'ask-rise' : undefined}>{children}</div>;
}

export interface ConversationLogHandle {
  /** Bring the message at `index` into view and return its element once mounted. */
  reveal: (index: number) => Promise<HTMLElement | null>;
  /** Index of the first message in view. */
  firstVisibleIndex: () => number;
}

export interface ConversationLogProps {
  /** The scrolling element the conversation is laid out in. */
  scroller: HTMLDivElement | null;
  conversationId: string | null;
  messages: readonly ChatMessage[];
  /** Messages mounted wherever the view is (focus, open source, go-to or find target). */
  keep: readonly (number | null)[];
  /** Room kept below the last message (the floating composer). */
  bottomSpace: number;
  /** Whether a message was already shown (it does not rise again). */
  wasShown: (id: string) => boolean;
  renderMessage: (message: ChatMessage, index: number) => React.ReactNode;
}

/**
 * The conversation, windowed: only the messages in view (and kept ones) are
 * mounted, so long conversations open, scroll and take typing without every
 * answer's markdown, charts and tables in the document. Older messages render
 * as the reader scrolls up to them. Loaded with the first conversation that
 * has messages, not at startup.
 */
export const ConversationLog = forwardRef<ConversationLogHandle, ConversationLogProps>(function ConversationLog(
  { scroller, conversationId, messages, keep, bottomSpace, wasShown, renderMessage },
  ref,
) {
  const keepKey = keep.join(',');
  const rangeExtractor = useCallback(
    (range: Range) => mountedIndexes(defaultRangeExtractor(range), [range.count - 1, ...keep], range.count),
    // keepKey stands for keep's content.
    [keepKey],
  );
  // Stable while the messages are: the window's measurements are rebuilt
  // when these change, so typing in the composer must not change them.
  const estimateSize = useCallback((index: number) => estimatedHeight(messages[index]), [messages]);
  const getItemKey = useCallback((index: number) => messages[index].id, [messages]);
  const virtualizer = useVirtualizer({
    count: messages.length,
    getScrollElement: () => scroller,
    estimateSize,
    getItemKey,
    overscan: OVERSCAN_MESSAGES,
    scrollMargin: CONVERSATION_TOP_PX,
    indexAttribute: MESSAGE_INDEX_ATTR,
    rangeExtractor,
  });
  const totalSize = virtualizer.getTotalSize();

  // Whether the view follows new output: decided from scroll events (where
  // the reader put the view), never from the size of what just arrived.
  const followRef = useRef(true);
  useEffect(() => {
    if (!scroller) return;
    const onScroll = () => {
      followRef.current = followsOutput(scroller.scrollHeight, scroller.scrollTop, scroller.clientHeight);
    };
    scroller.addEventListener('scroll', onScroll, { passive: true });
    return () => scroller.removeEventListener('scroll', onScroll);
  }, [scroller]);

  // Keep the newest content in view: always for a new message (or another
  // conversation), and while streaming or measuring only when following.
  const lastShownRef = useRef<{ conversationId: string | null; count: number }>({ conversationId: null, count: 0 });
  useLayoutEffect(() => {
    const last = lastShownRef.current;
    const isNew = last.conversationId !== conversationId || last.count !== messages.length;
    lastShownRef.current = { conversationId, count: messages.length };
    if (isNew) followRef.current = true;
    if (!scroller) return;
    if (followRef.current && distanceFromBottom(scroller.scrollHeight, scroller.scrollTop, scroller.clientHeight) > 0) {
      scroller.scrollTop = scroller.scrollHeight;
    }
  }, [scroller, messages, conversationId, totalSize, bottomSpace]);

  useImperativeHandle(ref, () => ({
    reveal: async (index: number) => {
      if (!scroller) return null;
      followRef.current = false;
      virtualizer.scrollToIndex(index, { align: 'start' });
      for (let frame = 0; frame < REVEAL_MAX_FRAMES; frame++) {
        await new Promise(resolve => requestAnimationFrame(resolve));
        const element = scroller.querySelector<HTMLElement>(`[${MESSAGE_INDEX_ATTR}="${index}"]`);
        if (element) return element;
      }
      return null;
    },
    firstVisibleIndex: () => {
      const offset = scroller ? scroller.scrollTop : 0;
      const item = virtualizer.getVirtualItems().find(v => v.end > offset);
      return item ? item.index : 0;
    },
  }), [scroller, virtualizer]);

  return (
    <div
      className="relative max-w-[700px] mx-auto"
      style={{ height: totalSize }}
      role="log"
      aria-label="Conversation"
      aria-live="off"
    >
      {virtualizer.getVirtualItems().map(item => {
        const index = item.index;
        const message = messages[index];
        return (
          <div
            key={item.key}
            {...{ [MESSAGE_INDEX_ATTR]: index }}
            ref={virtualizer.measureElement}
            className="absolute left-0 right-0 top-0 px-7"
            style={{
              transform: `translateY(${item.start - CONVERSATION_TOP_PX}px)`,
              paddingBottom: index === messages.length - 1 ? 0 : MESSAGE_GAP_PX,
            }}
          >
            <RiseOnce animate={!wasShown(message.id)}>{renderMessage(message, index)}</RiseOnce>
          </div>
        );
      })}
    </div>
  );
});

export default ConversationLog;

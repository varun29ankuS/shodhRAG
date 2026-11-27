import React, { useCallback, useLayoutEffect, useMemo, useState } from 'react';
import { defaultRangeExtractor, useVirtualizer } from '@tanstack/react-virtual';
import type { Range, VirtualItem, Virtualizer } from '@tanstack/react-virtual';
import { drawnIndexes } from '../lib/windowedRows';

/** Frames to wait for a row brought into view to be drawn. */
const REVEAL_MAX_FRAMES = 60;

export interface WindowedListOptions {
  /** How many rows the list has. */
  count: number;
  /** The element that scrolls: the list itself, or a page around it. */
  scroller: HTMLElement | null;
  /** The element the rows are placed in (`position: relative`, `height: total`). */
  list: HTMLElement | null;
  /** A row's height before it is measured (rows are measured once drawn). */
  estimate: number;
  /** A row's stable key. */
  getKey: (index: number) => string | number;
  /** Space between rows, and before the first and after the last. */
  gap?: number;
  paddingStart?: number;
  paddingEnd?: number;
  /** Rows drawn wherever the list is scrolled (focus, the tab stop). */
  keep?: readonly (number | null | undefined)[];
  /** Rows drawn beyond the visible ones, above and below. */
  overscan?: number;
}

export interface WindowedList {
  virtualizer: Virtualizer<HTMLElement, Element>;
  /** The rows to draw: each at `top` in the list, with `index` and `key`. */
  rows: Array<VirtualItem & { top: number }>;
  /** The list's height: every row, drawn or not. */
  total: number;
  /** Brings row `index` into view (`align` as `scrollIntoView`'s block) and returns its element (`data-index`) once drawn. */
  reveal: (index: number, align?: 'auto' | 'start' | 'center' | 'end') => Promise<HTMLElement | null>;
}

/**
 * A long list drawn a window at a time: only the rows in view (and kept ones) are in the
 * document, placed absolutely in `list` at their measured offsets, so a list of
 * thousands opens and scrolls as fast as one of twenty. The list may sit anywhere in its
 * scroller (under a page's heading): its offset there is measured and kept up to date.
 * Each drawn row needs `data-index={row.index}` and `ref={virtualizer.measureElement}`.
 */
export function useWindowedList({
  count,
  scroller,
  list,
  estimate,
  getKey,
  gap = 0,
  paddingStart = 0,
  paddingEnd = 0,
  keep = [],
  overscan = 6,
}: WindowedListOptions): WindowedList {
  // Where the list starts in its scroller's content: what is above it scrolls too.
  const [margin, setMargin] = useState(0);
  useLayoutEffect(() => {
    if (!scroller || !list) return;
    const measure = () => {
      const offset = scroller === list
        ? 0
        : list.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
      setMargin(prev => (Math.abs(prev - offset) < 0.5 ? prev : offset));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(scroller);
    if (scroller !== list) {
      // Content above the list that grows or shrinks moves it.
      for (let el = list.parentElement; el && el !== scroller; el = el.parentElement) observer.observe(el);
    }
    return () => observer.disconnect();
  }, [scroller, list]);

  const keepKey = keep.join(',');
  const rangeExtractor = useCallback(
    (range: Range) => drawnIndexes(defaultRangeExtractor(range), keep, range.count),
    // keepKey stands for keep's content.
    [keepKey],
  );
  const virtualizer = useVirtualizer<HTMLElement, Element>({
    count,
    getScrollElement: () => scroller,
    estimateSize: () => estimate,
    getItemKey: getKey,
    gap,
    paddingStart,
    paddingEnd,
    overscan,
    scrollMargin: margin,
    rangeExtractor,
  });

  const items = virtualizer.getVirtualItems();
  const rows = useMemo(() => items.map(item => ({ ...item, top: item.start - margin })), [items, margin]);

  const reveal = useCallback(
    async (index: number, align: 'auto' | 'start' | 'center' | 'end' = 'auto'): Promise<HTMLElement | null> => {
      if (!list || index < 0 || index >= count) return null;
      virtualizer.scrollToIndex(index, { align });
      for (let frame = 0; frame < REVEAL_MAX_FRAMES; frame++) {
        const element = list.querySelector<HTMLElement>(`[data-index="${index}"]`);
        if (element) return element;
        await new Promise(resolve => requestAnimationFrame(resolve));
      }
      return null;
    },
    [list, count, virtualizer],
  );

  return { virtualizer, rows, total: virtualizer.getTotalSize(), reveal };
}

/** The elements Tab stops on. */
const TABBABLE = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/**
 * The nearest ancestor of `el` that scrolls vertically (a windowed grid inside a page, a
 * dialog or a pane), or null.
 */
export function scrollParentOf(el: HTMLElement): HTMLElement | null {
  for (let box = el.parentElement; box; box = box.parentElement) {
    const overflowY = getComputedStyle(box).overflowY;
    if (overflowY === 'auto' || overflowY === 'scroll') return box;
  }
  return null;
}

/**
 * Tab out of the last card drawn of a windowed grid (Shift+Tab out of the first) goes
 * on to the next card, drawn first, as it would in a grid that drew every card. Cards
 * carry `data-card-index`; `rowOf` names the grid row a card is in.
 */
export function tabPastWindow(
  e: React.KeyboardEvent<HTMLElement>,
  count: number,
  rowOf: (card: number) => number,
  reveal: (row: number) => Promise<HTMLElement | null>,
): void {
  if (e.key !== 'Tab') return;
  const target = e.target as HTMLElement;
  const card = target.closest<HTMLElement>('[data-card-index]');
  if (!card) return;
  const stops = Array.from(card.querySelectorAll<HTMLElement>(TABBABLE));
  const leaving = e.shiftKey ? stops[0] === target : stops[stops.length - 1] === target;
  if (!leaving) return;
  const next = Number(card.dataset.cardIndex) + (e.shiftKey ? -1 : 1);
  if (!Number.isInteger(next) || next < 0 || next >= count) return;
  if (e.currentTarget.querySelector(`[data-card-index="${next}"]`)) return;
  e.preventDefault();
  void reveal(rowOf(next)).then(row => {
    const into = row?.querySelector<HTMLElement>(`[data-card-index="${next}"]`);
    const stopsInto = into ? Array.from(into.querySelectorAll<HTMLElement>(TABBABLE)) : [];
    (e.shiftKey ? stopsInto[stopsInto.length - 1] : stopsInto[0])?.focus();
  });
}

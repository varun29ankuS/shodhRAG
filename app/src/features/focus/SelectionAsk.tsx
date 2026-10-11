import React, { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { MessageSquareText } from 'lucide-react';
import { cn } from '../../lib/utils';
import { FOCUS_OVERLAY_SELECTOR } from './focusDom';
import { notifyDepthLimit, useFocus } from './FocusContext';
import type { DrillResult } from './FocusContext';
import { MAX_SELECTED_CHARS, selectionTarget } from './targets';
import { createFrameSlot } from './frameSlot';

/** Attribute marking where selected text can be asked about. */
export const ASK_SCOPE_ATTR = 'data-ask-scope';

/** Blocks whose text is the "surrounding paragraph" of a selection in an answer. */
const BLOCK_SELECTOR = 'p, li, td, th, blockquote, h1, h2, h3, h4, h5, h6, pre, dd, dt, figcaption';
const EDITABLE_SELECTOR = 'input, textarea, select, [contenteditable=""], [contenteditable="true"]';
const BUTTON_HEIGHT = 30;
const GAP = 6;
const EDGE = 8;

interface Pending {
  /** Where the button goes (viewport coordinates). */
  left: number;
  top: number;
  container: HTMLElement;
  act: () => void;
}

/** Characters of a block read around a selection (a large text file is one long block). */
const CONTEXT_WINDOW = 4_000;

/**
 * The block's text near the selection, bounded so a huge block (a whole
 * text file) is never processed on every selection change.
 */
function nearText(block: Element, selected: string): string {
  const full = block.textContent ?? '';
  if (full.length <= CONTEXT_WINDOW * 2) return full;
  const probe = selected.trim().slice(0, 120);
  const at = probe ? full.indexOf(probe) : -1;
  const center = at >= 0 ? at : 0;
  return full.slice(Math.max(0, center - CONTEXT_WINDOW), center + probe.length + CONTEXT_WINDOW);
}

function elementOf(node: Node | null): Element | null {
  if (!node) return null;
  return node instanceof Element ? node : node.parentElement;
}

/**
 * The current selection resolved to an action, or null when it is not
 * inside an ask scope. Everything (text, context, place) is captured now:
 * clicking the button must not depend on the selection still existing.
 */
function readSelection(focus: NonNullable<ReturnType<typeof useFocus>>): Omit<Pending, 'left' | 'top'> & { rect: DOMRect } | null {
  const sel = document.getSelection();
  if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return null;
  // Bounded: select-all in a large document must not be processed whole on every change.
  const raw = sel.toString().slice(0, MAX_SELECTED_CHARS * 2);
  if (raw.replace(/\s+/g, ' ').trim().length < 2) return null;
  const range = sel.getRangeAt(0);
  const start = elementOf(range.startContainer);
  const end = elementOf(range.endContainer);
  if (!start || !end) return null;
  if (start.closest(EDITABLE_SELECTOR) || end.closest(EDITABLE_SELECTOR)) return null;
  const scope = start.closest<HTMLElement>(`[${ASK_SCOPE_ATTR}]`);
  if (!scope || !scope.contains(end)) return null;
  const overlay = scope.closest<HTMLElement>(FOCUS_OVERLAY_SELECTOR);
  // While the pop-out is open the page behind it is inert.
  if (focus.session && !overlay) return null;

  const kind = scope.getAttribute(ASK_SCOPE_ATTR);
  const common = elementOf(range.commonAncestorContainer);
  const rects = range.getClientRects();
  const rect = rects.length > 0 ? rects[rects.length - 1] : range.getBoundingClientRect();
  const container = overlay ?? document.body;
  const finish = (result: DrillResult) => {
    if (result === 'depth') notifyDepthLimit();
  };

  if (kind === 'answer' || kind === 'thread') {
    const block = common?.closest<HTMLElement>(BLOCK_SELECTOR);
    const context = nearText(block && scope.contains(block) ? block : scope, raw);
    const target = selectionTarget({ text: raw, context, origin: 'answer' });
    if (!target) return null;
    if (kind === 'answer') {
      const conversationId = scope.dataset.conversationId;
      const messageId = scope.dataset.messageId;
      if (!conversationId || !messageId) return null;
      return { rect, container, act: () => focus.openFocus({ target, conversationId, parentMessageId: messageId }) };
    }
    const parentThreadId = scope.dataset.threadId;
    const parentTurnId = scope.dataset.turnId;
    if (!parentThreadId || !parentTurnId) return null;
    return { rect, container, act: () => finish(focus.drillDown({ target, parentThreadId, parentTurnId })) };
  }

  if (kind === 'document') {
    const sourceFile = scope.dataset.sourceFile;
    if (!sourceFile) return null;
    const pageEl = start.closest<HTMLElement>('[data-page]');
    const pageNumber = pageEl ? Number(pageEl.dataset.page) : NaN;
    const page = Number.isInteger(pageNumber) && pageNumber > 0 ? pageNumber : null;
    const block = pageEl ?? common?.closest<HTMLElement>(BLOCK_SELECTOR) ?? scope;
    const target = selectionTarget({
      text: raw,
      context: nearText(block, raw),
      origin: 'document',
      document: { sourceFile, fileName: scope.dataset.fileName ?? '', page },
    });
    if (!target) return null;
    if (overlay) {
      const stack = focus.session?.stack;
      const here = stack ? stack.levels[stack.index] : null;
      if (!here) return null;
      return { rect, container, act: () => finish(focus.drillDown({ target, parentThreadId: here.threadId, parentTurnId: null })) };
    }
    const messageId = scope.closest<HTMLElement>('[data-ask-message-id]')?.dataset.askMessageId ?? null;
    return { rect, container, act: () => focus.openFocus({ target, parentMessageId: messageId }) };
  }
  return null;
}

function place(rect: DOMRect): { left: number; top: number } {
  const width = 150;
  const below = rect.bottom + GAP;
  const top = below + BUTTON_HEIGHT + EDGE <= window.innerHeight ? below : Math.max(EDGE, rect.top - GAP - BUTTON_HEIGHT);
  const left = Math.min(window.innerWidth - width - EDGE, Math.max(EDGE, rect.right - width / 2));
  return { left, top };
}

/**
 * "Ask about this" for selected text: a small floating button next to a
 * selection inside an answer, a side answer or a document viewer (elements
 * marked with `data-ask-scope`). Ctrl+Shift+A does the same from the
 * keyboard. The selection itself is never changed, so copying still works.
 */
export function SelectionAsk() {
  const focus = useFocus();
  const [pending, setPending] = useState<Pending | null>(null);
  const focusRef = useRef(focus);
  focusRef.current = focus;
  const pointerDown = useRef(false);
  const frame = useRef(createFrameSlot());

  const update = useCallback(() => {
    const api = focusRef.current;
    if (!api || pointerDown.current) {
      setPending(null);
      return;
    }
    const read = readSelection(api);
    if (!read) {
      setPending(null);
      return;
    }
    const { rect, ...rest } = read;
    setPending({ ...rest, ...place(rect) });
  }, []);

  const schedule = useCallback(() => {
    frame.current.schedule(update);
  }, [update]);

  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      if (e.target instanceof Element && e.target.closest('[data-selection-ask]')) return;
      pointerDown.current = true;
      setPending(null);
    };
    const onUp = () => {
      pointerDown.current = false;
      schedule();
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || !e.shiftKey || e.altKey || e.code !== 'KeyA') return;
      const api = focusRef.current;
      if (!api) return;
      const read = readSelection(api);
      if (!read) return;
      e.preventDefault();
      e.stopPropagation();
      setPending(null);
      read.act();
    };
    document.addEventListener('selectionchange', schedule);
    document.addEventListener('pointerdown', onDown, true);
    document.addEventListener('pointerup', onUp, true);
    document.addEventListener('keydown', onKeyDown, true);
    window.addEventListener('scroll', schedule, true);
    window.addEventListener('resize', schedule);
    return () => {
      document.removeEventListener('selectionchange', schedule);
      document.removeEventListener('pointerdown', onDown, true);
      document.removeEventListener('pointerup', onUp, true);
      document.removeEventListener('keydown', onKeyDown, true);
      window.removeEventListener('scroll', schedule, true);
      window.removeEventListener('resize', schedule);
      frame.current.cancel();
    };
  }, [schedule]);

  // The pop-out opening or changing level moves the selection's context.
  useEffect(() => {
    schedule();
  }, [focus?.session, schedule]);

  if (!pending || !pending.container.isConnected) return null;
  return createPortal(
    <button
      type="button"
      data-selection-ask=""
      // Keep the selection: a press must not collapse it before the click.
      onPointerDown={e => e.preventDefault()}
      onMouseDown={e => e.preventDefault()}
      onClick={() => {
        const act = pending.act;
        setPending(null);
        act();
      }}
      title="Ask about the selected text (Ctrl+Shift+A)"
      style={{ left: pending.left, top: pending.top }}
      className={cn(
        'ask-fade-in fixed z-[70] h-[30px] px-2.5 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border-strong bg-shodh-surface text-[12px] font-medium text-shodh-text shadow-[0_6px_24px_rgba(0,0,0,0.25)] hover:bg-shodh-raised transition-colors duration-micro',
        'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
      )}
    >
      <MessageSquareText className="w-3.5 h-3.5 text-shodh-accent-text" aria-hidden="true" />
      Ask about this
      <kbd className="ml-0.5 text-[10.5px] font-sans text-shodh-text-faint">Ctrl+Shift+A</kbd>
    </button>,
    pending.container,
  );
}

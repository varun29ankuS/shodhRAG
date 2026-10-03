import React, { useCallback, useEffect, useId, useRef, useState } from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import { ArrowLeft, Maximize2, MessageSquareText, Minimize2, X } from 'lucide-react';
import { useTheme } from '../../contexts/ThemeContext';
import { cn } from '../../lib/utils';
import { pendingApproval } from '../agent/reducer';
import { sourceLabel } from '../ask/searchResults';
import type { SearchHit } from '../ask/types';
import { MAX_SELECTION_CHARS } from './contextBlock';
import { focusCommand } from './focusKeys';
import type { FocusKind, FocusTarget, FocusThread } from './focusTypes';
import { useFocus } from './FocusContext';
import type { OpenFocus } from './FocusContext';
import { FocusStage } from './FocusStage';
import type { StageCommandRef } from './FocusStage';
import { SideThread } from './SideThread';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const KIND_LABEL: Record<FocusKind, string> = {
  mermaid: 'Diagram',
  chart: 'Chart',
  equation: 'Equation',
  table: 'Table',
  image: 'Image',
  source: 'Source',
  task: 'Task',
};

const MIN_WIDTH = 560;
const MIN_HEIGHT = 380;
const EDGE = 16;
const RESIZE_STEP = 32;
const THREAD_MIN = 300;
const THREAD_DEFAULT = 400;
const THREAD_STEP = 24;
/** Below this pop-out width the side discussion starts hidden. */
const NARROW_WIDTH = 900;

interface Box {
  width: number;
  height: number;
}

function viewportBox(): Box {
  return { width: window.innerWidth, height: window.innerHeight };
}

function clampBox(box: Box): Box {
  const vp = viewportBox();
  return {
    width: Math.round(Math.min(vp.width - EDGE, Math.max(Math.min(MIN_WIDTH, vp.width - EDGE), box.width))),
    height: Math.round(Math.min(vp.height - EDGE, Math.max(Math.min(MIN_HEIGHT, vp.height - EDGE), box.height))),
  };
}

function defaultBox(): Box {
  const vp = viewportBox();
  return clampBox({ width: Math.min(1320, vp.width - 48), height: Math.min(900, vp.height - 48) });
}

function isEditable(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest('input, textarea, select, [contenteditable=""], [contenteditable="true"]') !== null;
}

function hitTarget(hit: SearchHit): FocusTarget {
  return {
    kind: 'source',
    label: sourceLabel(hit),
    hit: { ...hit },
  };
}

export interface FocusOverlayProps {
  open: OpenFocus;
  thread: FocusThread | null;
  onClose: () => void;
}

/**
 * The focus pop-out: a modal dialog showing one object (diagram, chart,
 * equation, table, image, document page or task) large and zoomable, with a
 * side discussion about it. Esc stops a running side answer first, then
 * closes; focus returns to the element that opened it.
 */
export function FocusOverlay({ open, thread, onClose }: FocusOverlayProps) {
  const { theme } = useTheme();
  const focus = useFocus();
  const descriptionId = useId();
  const [box, setBox] = useState<Box>(defaultBox);
  const [maximized, setMaximized] = useState(false);
  const [threadOpen, setThreadOpen] = useState(() => box.width >= NARROW_WIDTH || (thread?.turns.length ?? 0) > 0);
  const [threadWidth, setThreadWidth] = useState(THREAD_DEFAULT);
  const [peek, setPeek] = useState<SearchHit | null>(null);
  const [page, setPage] = useState<number | null>(null);
  const [selection, setSelection] = useState<string | null>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const stageAreaRef = useRef<HTMLDivElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const commandRef: StageCommandRef = useRef(null);

  const target = open.target;
  const live = focus?.sideLive?.threadId === open.threadId ? focus.sideLive.transcript : null;

  // Keep the pop-out inside the window when the window shrinks.
  useEffect(() => {
    const onResize = () => setBox(b => clampBox(b));
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);

  // Text selected inside the focused object is attached to the next question.
  // Tracked as it changes: clicking into the composer moves the selection.
  useEffect(() => {
    const onSelection = () => {
      const area = stageAreaRef.current;
      const sel = document.getSelection();
      if (!area || !sel || sel.rangeCount === 0 || !sel.anchorNode || !area.contains(sel.anchorNode)) return;
      const text = sel.toString().replace(/\s+/g, ' ').trim();
      setSelection(text ? Array.from(text).slice(0, MAX_SELECTION_CHARS).join('') : null);
    };
    document.addEventListener('selectionchange', onSelection);
    return () => document.removeEventListener('selectionchange', onSelection);
  }, []);

  // A peeked citation is not the focused object: its selection is not attached.
  useEffect(() => {
    setSelection(null);
  }, [peek]);

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.defaultPrevented) return;
    const inStage = e.target instanceof Node && stageAreaRef.current?.contains(e.target) === true;
    const command = focusCommand({
      key: e.key,
      ctrlKey: e.ctrlKey,
      metaKey: e.metaKey,
      altKey: e.altKey,
      shiftKey: e.shiftKey,
      editable: isEditable(e.target),
      pannable: inStage,
    });
    if (!command) return;
    if (command === 'toggleMaximize') {
      e.preventDefault();
      setMaximized(m => !m);
      return;
    }
    // Ctrl zoom chords always belong to the pop-out, never the window.
    if (e.ctrlKey || e.metaKey) e.preventDefault();
    if (commandRef.current?.(command, e.shiftKey)) e.preventDefault();
  };

  const onEscapeKeyDown = (e: KeyboardEvent) => {
    const t = e.target instanceof Element ? e.target : null;
    // The document viewer's find field closes itself on Esc.
    if (t?.closest('[role="search"]') && stageAreaRef.current?.contains(t)) {
      e.preventDefault();
      return;
    }
    if (live) {
      e.preventDefault();
      const step = pendingApproval(live);
      if (step) focus?.approve(step.id, false);
      else focus?.stop();
      return;
    }
    if (peek) {
      e.preventDefault();
      setPeek(null);
    }
  };

  const onOpenAutoFocus = (e: Event) => {
    e.preventDefault();
    const stageFocus = stageAreaRef.current?.querySelector<HTMLElement>('[tabindex="0"], button, [href]');
    (stageFocus ?? contentRef.current)?.focus();
  };

  const onCloseAutoFocus = (e: Event) => {
    const trigger = open.trigger;
    if (trigger && trigger.isConnected) {
      e.preventDefault();
      // No scroll: the opener was on screen, and after "Add to main
      // conversation" the view must stay on the new answer.
      trigger.focus({ preventScroll: true });
    }
  };

  // Resize from the corner: the pop-out stays centred, so it grows on both sides.
  const resizeDrag = useRef<{ id: number; x: number; y: number; box: Box } | null>(null);
  const onResizePointerDown = (e: React.PointerEvent<HTMLButtonElement>) => {
    if (e.button !== 0) return;
    resizeDrag.current = { id: e.pointerId, x: e.clientX, y: e.clientY, box };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onResizePointerMove = (e: React.PointerEvent<HTMLButtonElement>) => {
    const d = resizeDrag.current;
    if (!d || d.id !== e.pointerId) return;
    setBox(clampBox({ width: d.box.width + (e.clientX - d.x) * 2, height: d.box.height + (e.clientY - d.y) * 2 }));
  };
  const onResizePointerUp = (e: React.PointerEvent<HTMLButtonElement>) => {
    if (resizeDrag.current?.id === e.pointerId) resizeDrag.current = null;
  };
  const onResizeKeyDown = (e: React.KeyboardEvent<HTMLButtonElement>) => {
    const step = e.shiftKey ? RESIZE_STEP * 4 : RESIZE_STEP;
    const delta: Record<string, [number, number]> = {
      ArrowRight: [step, 0],
      ArrowLeft: [-step, 0],
      ArrowDown: [0, step],
      ArrowUp: [0, -step],
    };
    const d = delta[e.key];
    if (!d) return;
    e.preventDefault();
    setBox(b => clampBox({ width: b.width + d[0], height: b.height + d[1] }));
  };

  // Splitter between the object and the side discussion.
  const maxThreadWidth = useCallback(() => {
    const total = bodyRef.current?.clientWidth ?? box.width;
    return Math.max(THREAD_MIN, Math.round(total * 0.6));
  }, [box.width]);
  const splitDrag = useRef<{ id: number; x: number; width: number } | null>(null);
  const onSplitPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    splitDrag.current = { id: e.pointerId, x: e.clientX, width: threadWidth };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onSplitPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const d = splitDrag.current;
    if (!d || d.id !== e.pointerId) return;
    setThreadWidth(Math.min(maxThreadWidth(), Math.max(THREAD_MIN, d.width - (e.clientX - d.x))));
  };
  const onSplitPointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    if (splitDrag.current?.id === e.pointerId) splitDrag.current = null;
  };
  const onSplitKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const max = maxThreadWidth();
    let next: number | null = null;
    if (e.key === 'ArrowLeft') next = threadWidth + THREAD_STEP;
    else if (e.key === 'ArrowRight') next = threadWidth - THREAD_STEP;
    else if (e.key === 'Home') next = max;
    else if (e.key === 'End') next = THREAD_MIN;
    if (next === null) return;
    e.preventDefault();
    setThreadWidth(Math.min(max, Math.max(THREAD_MIN, next)));
  };
  // Keep the side discussion within bounds when the pop-out shrinks.
  useEffect(() => {
    setThreadWidth(w => Math.min(maxThreadWidth(), Math.max(THREAD_MIN, w)));
  }, [box.width, maximized, maxThreadWidth]);

  const clearSelection = useCallback(() => setSelection(null), []);
  const onPageChange = useCallback((p: number) => setPage(p), []);
  const stageTarget = peek ? hitTarget(peek) : target;
  const extras = { selection: peek ? null : selection, page: target.kind === 'source' ? page : null };

  return (
    <Dialog.Root open onOpenChange={next => { if (!next) onClose(); }}>
      <Dialog.Portal>
        <Dialog.Overlay className="ask-fade-in fixed inset-0 z-[60] bg-black/50" />
        <Dialog.Content
          ref={contentRef}
          data-focus-overlay=""
          aria-describedby={descriptionId}
          onEscapeKeyDown={onEscapeKeyDown}
          onOpenAutoFocus={onOpenAutoFocus}
          onCloseAutoFocus={onCloseAutoFocus}
          onKeyDown={onKeyDown}
          className={cn(
            'ask-fade-in fixed z-[60] m-auto flex flex-col overflow-hidden border border-shodh-border-strong bg-shodh-surface text-shodh-text shadow-[0_24px_80px_rgba(0,0,0,0.45)] focus:outline-none',
            maximized ? 'inset-2 rounded-[14px]' : 'inset-0 rounded-[18px]',
          )}
          style={maximized ? undefined : { width: box.width, height: box.height }}
        >
          <header className="shrink-0 flex items-center gap-2.5 pl-5 pr-3 py-2.5 border-b border-shodh-border-subtle">
            <span className="text-[11px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint" aria-hidden="true">
              {KIND_LABEL[target.kind]}
            </span>
            <Dialog.Title className="flex-1 min-w-0 text-[14px] font-semibold text-shodh-text truncate" title={target.label}>
              {target.label}
            </Dialog.Title>
            <Dialog.Description id={descriptionId} className="sr-only">
              {target.kind === 'source' || target.kind === 'task'
                ? 'Shown large, with a side discussion about it. Press Escape to close.'
                : 'Zoom with plus, minus or Control and the mouse wheel; 0 fits, 1 shows actual size; drag or use the arrow keys to pan. M maximises. Escape closes.'}
            </Dialog.Description>
            <button
              type="button"
              onClick={() => setThreadOpen(v => !v)}
              aria-expanded={threadOpen}
              aria-controls={`${descriptionId}-thread`}
              className={cn(
                'h-8 px-2.5 inline-flex items-center gap-1.5 rounded-lg text-[12.5px] transition-colors duration-micro',
                threadOpen ? 'bg-shodh-raised text-shodh-text' : 'text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text',
                FOCUS_RING,
              )}
            >
              <MessageSquareText className="w-4 h-4" aria-hidden="true" />
              Ask about this
              {thread && thread.turns.length > 0 && (
                <span className="ml-0.5 px-1.5 rounded-full bg-shodh-raised-2 text-[11px] tabular-nums text-shodh-text-muted">
                  {thread.turns.filter(t => t.role === 'assistant').length}
                </span>
              )}
            </button>
            <button
              type="button"
              onClick={() => setMaximized(m => !m)}
              aria-pressed={maximized}
              aria-label={maximized ? 'Restore size' : 'Maximise'}
              title={maximized ? 'Restore size (M)' : 'Maximise (M)'}
              className={cn(
                'w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
                FOCUS_RING,
              )}
            >
              {maximized ? <Minimize2 className="w-4 h-4" aria-hidden="true" /> : <Maximize2 className="w-4 h-4" aria-hidden="true" />}
            </button>
            <Dialog.Close
              aria-label="Close"
              title="Close (Esc)"
              className={cn(
                'w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
                FOCUS_RING,
              )}
            >
              <X className="w-4 h-4" strokeWidth={2.4} aria-hidden="true" />
            </Dialog.Close>
          </header>

          <div ref={bodyRef} className="flex-1 min-h-0 flex">
            <div ref={stageAreaRef} className="flex-1 min-w-0 min-h-0 flex flex-col">
              {peek && (
                <div className="shrink-0 flex items-center gap-2 px-3 py-1.5 border-b border-shodh-border-subtle bg-shodh-surface-2 text-[12.5px] text-shodh-text-secondary">
                  <button
                    type="button"
                    onClick={() => setPeek(null)}
                    className={cn('h-7 px-2 inline-flex items-center gap-1.5 rounded-lg hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro', FOCUS_RING)}
                  >
                    <ArrowLeft className="w-3.5 h-3.5" aria-hidden="true" />
                    {`Back to ${KIND_LABEL[target.kind].toLowerCase()}`}
                  </button>
                  <span className="truncate">{`Source ${peek.number}: ${sourceLabel(peek)}`}</span>
                </div>
              )}
              <FocusStage
                key={peek ? `peek-${peek.sourceFile}-${peek.number}` : 'target'}
                target={stageTarget}
                theme={theme}
                commandRef={commandRef}
                onPageChange={peek ? undefined : onPageChange}
              />
            </div>

            {threadOpen && (
              <>
                <div
                  role="separator"
                  aria-orientation="vertical"
                  aria-label="Resize the side discussion"
                  aria-valuemin={THREAD_MIN}
                  aria-valuemax={maxThreadWidth()}
                  aria-valuenow={threadWidth}
                  tabIndex={0}
                  onKeyDown={onSplitKeyDown}
                  onPointerDown={onSplitPointerDown}
                  onPointerMove={onSplitPointerMove}
                  onPointerUp={onSplitPointerUp}
                  onPointerCancel={onSplitPointerUp}
                  className="w-1.5 shrink-0 cursor-col-resize touch-none bg-shodh-border-subtle hover:bg-shodh-border-strong focus-visible:outline-none focus-visible:bg-shodh-accent transition-colors duration-micro"
                />
                <div id={`${descriptionId}-thread`} className="shrink-0 min-h-0" style={{ width: threadWidth }}>
                  <SideThread
                    open={open}
                    thread={thread}
                    extras={extras}
                    onClearSelection={clearSelection}
                    onOpenCitation={setPeek}
                    onDone={onClose}
                  />
                </div>
              </>
            )}
          </div>

          {!maximized && (
            <button
              type="button"
              aria-label="Resize the focus view. Use the arrow keys."
              title="Drag to resize"
              onPointerDown={onResizePointerDown}
              onPointerMove={onResizePointerMove}
              onPointerUp={onResizePointerUp}
              onPointerCancel={onResizePointerUp}
              onKeyDown={onResizeKeyDown}
              className={cn(
                'absolute right-0 bottom-0 z-20 w-4 h-4 cursor-nwse-resize touch-none rounded-tl-md text-shodh-text-faint',
                "after:absolute after:right-1 after:bottom-1 after:w-2 after:h-2 after:border-r-2 after:border-b-2 after:border-current after:content-['']",
                FOCUS_RING,
              )}
            />
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

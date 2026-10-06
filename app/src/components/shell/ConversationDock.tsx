import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Maximize2, Minus, Pin, PinOff, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import type { ViewTab } from '../../lib/viewTabs';
import {
  DOCK_MIN_WIDTH,
  DOCK_STORAGE_KEY,
  clampDockWidth,
  maxDockWidth,
  parseDockPrefs,
  resolveDockLayout,
  serializeDockPrefs,
  viewPrefs,
  widthForDrag,
  widthForKey,
  withViewPrefs,
} from '../../lib/dockMode';
import type { DockLayout, DockPrefs, DockViewPrefs } from '../../lib/dockMode';
import { AgentComposer } from '../../features/agent/AgentComposer';
import type { AgentComposerHandle } from '../../features/agent/AgentComposer';
import { SnippetDropZone } from '../../features/research/SnippetDropZone';
import { appendToDraft } from '../../features/research/snippetModel';
import { passageHits } from '../../features/agent/citations';
import { currentStep, isLive, pendingApproval } from '../../features/agent/reducer';
import { StatusLine } from '../../features/agent/StatusLine';
import { Transcript } from '../../features/agent/Transcript';
import { useChatSession } from '../../features/ask/ChatSessionContext';
import type { ChatMessage } from '../../features/ask/types';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** Pre-v2 record (a single global 'hidden' | 'open' | 'minimized'); superseded by per-view prefs. */
const LEGACY_STORAGE_KEY = 'shodh.conversationDock';

/**
 * Height kept clear below the top of the page content for the overlay. Every
 * non-Ask view puts its primary actions at the top of the page (Library's
 * "Add folder" header, the Tasks layout switch and list toolbar, the Activity
 * and Settings headers) and none has a bottom action bar, so bounding the
 * overlay's top below them keeps those actions uncovered.
 */
const OVERLAY_TOOLBAR_RESERVE = 112;
const OVERLAY_HEIGHT = 470;
const OVERLAY_EDGE = 16;

/** Elements whose own interactions must not collapse the overlay (portaled dialogs, menus, toasts, drag sources). */
const OUTSIDE_EXEMPT =
  '[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"], [data-sonner-toaster], [draggable="true"]';

function readPrefs(): DockPrefs {
  try {
    window.localStorage.removeItem(LEGACY_STORAGE_KEY);
    return parseDockPrefs(window.localStorage.getItem(DOCK_STORAGE_KEY));
  } catch {
    return {};
  }
}

function storePrefs(prefs: DockPrefs) {
  try {
    window.localStorage.setItem(DOCK_STORAGE_KEY, serializeDockPrefs(prefs));
  } catch {
    // Storage unavailable (private mode); the dock still works for this session.
  }
}

function useWindowWidth(): number {
  const [width, setWidth] = useState(() => window.innerWidth);
  useEffect(() => {
    const onResize = () => setWidth(window.innerWidth);
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);
  return width;
}

/** Grows with every new message, step and text segment: the unread measure. */
function activitySize(messages: readonly ChatMessage[]): number {
  let size = messages.length;
  for (const m of messages) size += m.transcript?.blocks.length ?? 0;
  return size;
}

interface ConversationDockProps {
  activeTab: ViewTab;
  /** Return to the Ask view (full conversation). */
  onExpand: () => void;
}

/**
 * The active conversation while another view is open.
 *
 * Minimised it is a pill (bottom-right) with a working indicator and an unread
 * count. Expanded (pill, Ctrl+J) it is a resizable side panel that the page
 * reflows around, so nothing is covered; pinning makes the panel that view's
 * default. When the window is too narrow for a panel next to a usable page it
 * floats instead, kept below the page's toolbar, and collapses back to the
 * pill on Esc or a click outside. Layout rules live in `lib/dockMode`.
 */
export function ConversationDock({ activeTab, onExpand }: ConversationDockProps) {
  const session = useChatSession();
  const { messages, isStreaming, navigation, steer, send, cancel, approve, setRuntimeInstalled, streamingConversationId, sideRun } = session;
  const [prefs, setPrefs] = useState<DockPrefs>(readPrefs);
  /** Expanded or collapsed by the person on a view; only applies while that view stays open. */
  const [override, setOverride] = useState<{ tab: ViewTab; open: boolean } | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [dragWidth, setDragWidth] = useState<number | null>(null);
  const [overlayTop, setOverlayTop] = useState(OVERLAY_TOOLBAR_RESERVE);
  const [draft, setDraft] = useState('');
  const [unreadBase, setUnreadBase] = useState<number | null>(null);
  const panelRef = useRef<HTMLElement | null>(null);
  const pillRef = useRef<HTMLButtonElement>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const composerRef = useRef<AgentComposerHandle>(null);
  const dragRef = useRef<{ pointerId: number; startX: number; startWidth: number } | null>(null);
  const focusComposerRef = useRef(false);
  const focusPillRef = useRef(false);
  const windowWidth = useWindowWidth();
  const offAsk = activeTab !== 'ask';
  const current: DockViewPrefs = viewPrefs(prefs, activeTab);

  const updatePrefs = useCallback((patch: Partial<DockViewPrefs>) => {
    setPrefs(prev => {
      const next = withViewPrefs(prev, activeTab, patch);
      storePrefs(next);
      return next;
    });
  }, [activeTab]);

  const expand = useCallback(() => {
    focusComposerRef.current = true;
    setDismissed(false);
    setOverride({ tab: activeTab, open: true });
  }, [activeTab]);
  const collapse = useCallback((returnFocus: boolean) => {
    focusPillRef.current = returnFocus;
    setOverride({ tab: activeTab, open: false });
  }, [activeTab]);

  // The agent opened another view, or an answer is running off Ask: bring the
  // dock back if it was closed. It shows in the view's own form (a pill unless
  // pinned) so the page is never shifted or covered unasked.
  const lastNavSeq = useRef(navigation?.seq ?? 0);
  useEffect(() => {
    if (!navigation || navigation.seq === lastNavSeq.current) return;
    lastNavSeq.current = navigation.seq;
    setDismissed(false);
  }, [navigation]);
  useEffect(() => {
    if (offAsk && isStreaming) setDismissed(false);
  }, [offAsk, isStreaming]);

  const lastUser = useMemo(() => [...messages].reverse().find(m => m.role === 'user') ?? null, [messages]);
  const lastAnswer = useMemo(() => [...messages].reverse().find(m => m.role === 'assistant' && m.transcript) ?? null, [messages]);
  const transcript = lastAnswer?.transcript ?? null;
  const hits = useMemo(() => passageHits(transcript?.passages ?? []), [transcript?.passages]);
  const live = transcript !== null && isLive(transcript);
  const waiting = live && transcript ? pendingApproval(transcript) : null;
  const working = live && transcript ? currentStep(transcript) : null;
  const visible = offAsk && !dismissed && messages.length > 0;
  // Each visit starts from the view's own preference; an expand or collapse
  // made on another view does not carry over.
  const openOverride = override !== null && override.tab === activeTab ? override.open : null;
  const open = openOverride ?? current.pinned;
  const layout: DockLayout | null = visible ? resolveDockLayout(open, windowWidth, openOverride === true) : null;
  const panelWidth = clampDockWidth(dragWidth ?? current.width, windowWidth);

  // Ctrl+J expands or minimises the dock (outside Ask, where the conversation is already shown).
  useEffect(() => {
    if (!offAsk) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey || e.key.toLowerCase() !== 'j') return;
      e.preventDefault();
      if (layout === 'pill' || layout === null) expand();
      else collapse(true);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [offAsk, layout, expand, collapse]);

  // Unread count while minimised.
  const size = activitySize(messages);
  useEffect(() => {
    if (layout === 'pill') setUnreadBase(base => (base === null ? size : base));
    else setUnreadBase(null);
  }, [layout, size]);
  const unread = unreadBase === null ? 0 : Math.max(0, size - unreadBase);

  // Focus follows explicit actions only: a pinned panel restored on a view
  // switch must not take focus from the page.
  useEffect(() => {
    if (layout === 'panel' || layout === 'overlay') {
      if (focusComposerRef.current) composerRef.current?.focus();
      focusComposerRef.current = false;
    } else if (layout === 'pill') {
      if (focusPillRef.current) pillRef.current?.focus();
      focusPillRef.current = false;
    }
  }, [layout]);
  useLayoutEffect(() => {
    const el = scrollerRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, layout]);

  // Overlay: keep its top below the page's toolbar, measured from the content area.
  useLayoutEffect(() => {
    if (layout !== 'overlay') return;
    const main = document.getElementById('main-content');
    const top = main ? main.getBoundingClientRect().top : 0;
    setOverlayTop(Math.max(0, Math.round(top)) + OVERLAY_TOOLBAR_RESERVE);
  }, [layout, windowWidth]);

  // Overlay: a press outside collapses it to the pill.
  useEffect(() => {
    if (layout !== 'overlay') return;
    const onPointerDown = (e: PointerEvent) => {
      const target = e.target;
      if (!(target instanceof Element)) return;
      if (panelRef.current?.contains(target) || target.closest(OUTSIDE_EXEMPT)) return;
      collapse(false);
    };
    document.addEventListener('pointerdown', onPointerDown, true);
    return () => document.removeEventListener('pointerdown', onPointerDown, true);
  }, [layout, collapse]);

  // Keyboard inside the dock: Esc denies a pending approval or interrupts the
  // answer; with nothing running, Esc collapses the overlay. Tab stays within
  // the overlay (it floats over the page); the side panel is part of the page
  // and Tab moves freely in and out of it.
  const onPanelKeyDown = (e: React.KeyboardEvent<HTMLElement>) => {
    if (e.key === 'Escape') {
      if (live) {
        e.preventDefault();
        if (waiting) approve(waiting.id, false);
        else cancel();
        return;
      }
      if (layout === 'overlay') {
        e.preventDefault();
        collapse(true);
      }
      return;
    }
    if (layout !== 'overlay' || e.key !== 'Tab' || !panelRef.current) return;
    const items = Array.from(panelRef.current.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(el => el.offsetParent !== null);
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  };

  // Resizing the side panel from its left edge.
  const endDrag = (e: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== e.pointerId) return;
    dragRef.current = null;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
    const finalWidth = widthForDrag(drag.startWidth, drag.startX, e.clientX, window.innerWidth);
    setDragWidth(null);
    if (finalWidth !== drag.startWidth) updatePrefs({ width: finalWidth });
  };
  const resizeHandlers = {
    onPointerDown: (e: React.PointerEvent<HTMLDivElement>) => {
      if (e.button !== 0) return;
      e.preventDefault();
      e.currentTarget.setPointerCapture(e.pointerId);
      dragRef.current = { pointerId: e.pointerId, startX: e.clientX, startWidth: panelWidth };
      setDragWidth(panelWidth);
    },
    onPointerMove: (e: React.PointerEvent<HTMLDivElement>) => {
      const drag = dragRef.current;
      if (!drag || drag.pointerId !== e.pointerId) return;
      setDragWidth(widthForDrag(drag.startWidth, drag.startX, e.clientX, window.innerWidth));
    },
    onPointerUp: endDrag,
    onPointerCancel: endDrag,
    onLostPointerCapture: (e: React.PointerEvent<HTMLDivElement>) => {
      if (dragRef.current?.pointerId !== e.pointerId) return;
      dragRef.current = null;
      setDragWidth(null);
    },
    onKeyDown: (e: React.KeyboardEvent<HTMLDivElement>) => {
      const next = widthForKey(e.key, panelWidth, windowWidth, e.shiftKey);
      if (next === null) return;
      e.preventDefault();
      if (next !== current.width) updatePrefs({ width: next });
    },
  };

  const busyElsewhere = (streamingConversationId !== null && !isStreaming) || sideRun !== null;

  const submit = () => {
    const text = draft.trim();
    if (!text || isStreaming || busyElsewhere) return;
    setDraft('');
    void send(text, { spaceId: null, spaceName: null });
  };
  const submitSteer = () => {
    const text = draft.trim();
    if (!text || !isStreaming) return;
    setDraft('');
    steer(text);
  };

  if (layout === null) return null;

  if (layout === 'pill') {
    return (
      <button
        ref={pillRef}
        type="button"
        onClick={expand}
        aria-label={`Open the conversation${waiting ? ', needs your approval' : working ? `, working: ${working.label}` : live ? ', working' : ''}${unread > 0 ? `, ${unread} new` : ''} (Ctrl+J)`}
        title="Open the conversation (Ctrl+J)"
        className={cn(
          'dock-pop fixed bottom-4 right-4 z-40 inline-flex items-center gap-2 h-10 max-w-[320px] pl-3 pr-3.5 rounded-full border border-shodh-border-strong bg-shodh-surface shadow-[0_8px_28px_rgba(0,0,0,0.3)] text-[12.5px] text-shodh-text hover:bg-shodh-raised transition-colors duration-micro',
          FOCUS_RING,
        )}
      >
        <span
          className={cn('w-2 h-2 rounded-full shrink-0', live ? (waiting ? 'bg-shodh-warning ask-breathe' : 'bg-shodh-accent-text ask-breathe') : 'bg-shodh-text-faint')}
          aria-hidden="true"
        />
        <span className="font-semibold shrink-0">Assistant</span>
        {live && (
          <span className="min-w-0 truncate text-shodh-text-muted">
            {waiting ? 'Needs your approval' : working ? working.label : 'Working…'}
          </span>
        )}
        {unread > 0 && (
          <span className="shrink-0 min-w-[18px] h-[18px] px-1 rounded-full bg-shodh-accent text-shodh-on-accent text-[10.5px] font-bold leading-[18px] text-center tabular-nums" aria-hidden="true">
            {unread > 99 ? '99+' : unread}
          </span>
        )}
      </button>
    );
  }

  const pinned = current.pinned;
  const controls = [
    { label: 'Open in Ask', icon: Maximize2, onClick: onExpand },
    pinned
      ? { label: 'Unpin side panel', icon: PinOff, onClick: () => { focusPillRef.current = true; updatePrefs({ pinned: false }); setOverride(null); } }
      : {
          label: layout === 'overlay' ? 'Pin as side panel (shown when the window is wider)' : 'Pin as side panel',
          icon: Pin,
          onClick: () => updatePrefs({ pinned: true }),
        },
    { label: 'Minimise (Ctrl+J)', icon: Minus, onClick: () => collapse(true) },
    { label: 'Close', icon: X, onClick: () => setDismissed(true) },
  ];

  const body = (
    <>
      <header className="shrink-0 flex items-center gap-2 h-11 pl-3.5 pr-1.5 border-b border-shodh-border bg-shodh-surface">
        <span
          className={cn('w-2 h-2 rounded-full shrink-0', live ? 'bg-shodh-accent-text ask-breathe' : 'bg-shodh-success')}
          aria-hidden="true"
        />
        <span className="text-[13px] font-semibold text-shodh-text shrink-0">Assistant</span>
        <span className="min-w-0 truncate text-[12px] text-shodh-text-muted">
          {session.activeConversation?.title ?? ''}
        </span>
        <div className="ml-auto flex items-center">
          {controls.map(({ label, icon: Icon, onClick }) => (
            <button
              key={label}
              type="button"
              onClick={onClick}
              aria-label={label}
              title={label}
              className={cn(
                'w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
                FOCUS_RING,
              )}
            >
              <Icon className="w-3.5 h-3.5" aria-hidden="true" />
            </button>
          ))}
        </div>
      </header>

      <div ref={scrollerRef} className="flex-1 min-h-0 overflow-y-auto scrollbar-thin px-3.5 py-3 flex flex-col gap-3" role="log" aria-label="Latest exchange">
        {lastUser && (
          <div className="self-end max-w-[85%] px-3 py-2 rounded-[14px] bg-shodh-raised-2 text-[13px] leading-[1.5] text-shodh-text whitespace-pre-wrap break-words">
            {lastUser.content}
          </div>
        )}
        {transcript && (
          <Transcript
            transcript={transcript}
            hits={hits}
            onOpenCitation={() => onExpand()}
            onDecide={approve}
            onRuntimeInstalled={() => setRuntimeInstalled(true)}
            onOpenSettings={() => window.dispatchEvent(new CustomEvent('switchTab', { detail: 'settings' }))}
            compact
          />
        )}
      </div>

      <div className="shrink-0 border-t border-shodh-border bg-shodh-surface px-2.5 pt-2 pb-2.5 flex flex-col gap-1.5">
        {transcript && (
          <div className="px-1">
            <StatusLine transcript={transcript} fallbackModel={null} compact />
          </div>
        )}
        <SnippetDropZone onInsert={text => setDraft(prev => appendToDraft(prev, text))}>
          <AgentComposer
            id="dock-composer"
            ref={composerRef}
            value={draft}
            onChange={setDraft}
            onSubmit={submit}
            onSteer={submitSteer}
            onStop={cancel}
            onApprove={waiting?.tier === 'write' ? () => approve(waiting.id, true) : undefined}
            running={isStreaming}
            canSteer={transcript?.status === 'running'}
            approvalPending={waiting !== null}
            blockedReason={sideRun ? `Answering your side question about “${sideRun.label}”.` : busyElsewhere ? 'An answer is running in another conversation.' : null}
            placeholder="Reply…"
            compact
          />
        </SnippetDropZone>
      </div>
    </>
  );

  if (layout === 'panel') {
    const dragging = dragWidth !== null;
    return (
      <aside
        ref={el => { panelRef.current = el; }}
        id="conversation-dock"
        aria-label="Conversation"
        onKeyDown={onPanelKeyDown}
        className={cn('relative h-full shrink-0 flex flex-col border-l border-shodh-border bg-shodh-ground', dragging && 'select-none')}
        style={{ width: panelWidth }}
      >
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize conversation panel"
          aria-controls="conversation-dock"
          aria-valuenow={panelWidth}
          aria-valuemin={DOCK_MIN_WIDTH}
          aria-valuemax={maxDockWidth(windowWidth)}
          aria-valuetext={`${panelWidth} pixels wide`}
          tabIndex={0}
          title="Drag or use the arrow keys to resize"
          {...resizeHandlers}
          className={cn(
            'group absolute inset-y-0 left-0 z-10 w-2 cursor-col-resize touch-none flex justify-start',
            'focus-visible:outline-none',
          )}
        >
          <span
            className={cn(
              'h-full w-0.5 transition-colors duration-micro',
              dragging ? 'bg-shodh-accent' : 'bg-transparent group-hover:bg-shodh-border-strong group-focus-visible:bg-shodh-accent',
            )}
            aria-hidden="true"
          />
        </div>
        {body}
      </aside>
    );
  }

  return (
    <div
      ref={el => { panelRef.current = el; }}
      role="dialog"
      aria-modal="false"
      aria-label="Conversation"
      onKeyDown={onPanelKeyDown}
      className="dock-pop fixed bottom-4 right-4 z-40 flex flex-col rounded-2xl border border-shodh-border-strong bg-shodh-ground shadow-[0_16px_48px_rgba(0,0,0,0.38)] overflow-hidden"
      style={{
        width: `min(380px, calc(100vw - ${OVERLAY_EDGE * 2}px))`,
        height: OVERLAY_HEIGHT,
        maxHeight: `calc(100vh - ${overlayTop + OVERLAY_EDGE}px)`,
      }}
    >
      {body}
    </div>
  );
}

export default ConversationDock;

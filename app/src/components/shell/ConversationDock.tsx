import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Maximize2, Minus, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import type { ViewTab } from '../../lib/viewTabs';
import { AgentComposer } from '../../features/agent/AgentComposer';
import type { AgentComposerHandle } from '../../features/agent/AgentComposer';
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

const STORAGE_KEY = 'shodh.conversationDock';

export type DockMode = 'hidden' | 'open' | 'minimized';

function readStoredMode(): DockMode {
  try {
    const value = window.localStorage.getItem(STORAGE_KEY);
    return value === 'open' || value === 'minimized' ? value : 'hidden';
  } catch {
    return 'hidden';
  }
}

function storeMode(mode: DockMode) {
  try {
    window.localStorage.setItem(STORAGE_KEY, mode);
  } catch {
    // Storage unavailable (private mode); the dock still works for this session.
  }
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
 * The active conversation, docked bottom-right while another view is open.
 * Opens when the agent navigates; Ctrl+J toggles it; minimised it is a pill
 * with a working indicator and an unread count.
 */
export function ConversationDock({ activeTab, onExpand }: ConversationDockProps) {
  const session = useChatSession();
  const { messages, isStreaming, navigation, steer, send, cancel, approve, setRuntimeInstalled, streamingConversationId } = session;
  const [mode, setMode] = useState<DockMode>(readStoredMode);
  const [draft, setDraft] = useState('');
  const [unreadBase, setUnreadBase] = useState<number | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const composerRef = useRef<AgentComposerHandle>(null);
  const offAsk = activeTab !== 'ask';

  const changeMode = useCallback((next: DockMode) => {
    setMode(next);
    storeMode(next);
  }, []);

  // The agent opened another view: follow the conversation in the dock.
  const lastNavSeq = useRef(navigation?.seq ?? 0);
  useEffect(() => {
    if (!navigation || navigation.seq === lastNavSeq.current) return;
    lastNavSeq.current = navigation.seq;
    changeMode('open');
  }, [navigation, changeMode]);

  // Leaving Ask while an answer runs keeps it visible as a pill.
  useEffect(() => {
    if (offAsk && isStreaming && mode === 'hidden') changeMode('minimized');
  }, [offAsk, isStreaming, mode, changeMode]);

  // Ctrl+J toggles the dock (outside Ask, where the conversation is already shown).
  useEffect(() => {
    if (!offAsk) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey || e.key.toLowerCase() !== 'j') return;
      e.preventDefault();
      changeMode(mode === 'open' ? 'minimized' : 'open');
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [offAsk, mode, changeMode]);

  const lastUser = useMemo(() => [...messages].reverse().find(m => m.role === 'user') ?? null, [messages]);
  const lastAnswer = useMemo(() => [...messages].reverse().find(m => m.role === 'assistant' && m.transcript) ?? null, [messages]);
  const transcript = lastAnswer?.transcript ?? null;
  const hits = useMemo(() => passageHits(transcript?.passages ?? []), [transcript?.passages]);
  const live = transcript !== null && isLive(transcript);
  const waiting = live && transcript ? pendingApproval(transcript) : null;
  const working = live && transcript ? currentStep(transcript) : null;
  const visible = offAsk && mode !== 'hidden' && messages.length > 0;
  const shownMode: DockMode = visible ? mode : 'hidden';

  // Unread count while minimised.
  const size = activitySize(messages);
  useEffect(() => {
    if (shownMode === 'minimized') setUnreadBase(base => (base === null ? size : base));
    else setUnreadBase(null);
  }, [shownMode, size]);
  const unread = unreadBase === null ? 0 : Math.max(0, size - unreadBase);

  // Focus the reply box when the dock opens; keep the newest content in view.
  useEffect(() => {
    if (shownMode === 'open') composerRef.current?.focus();
  }, [shownMode]);
  useLayoutEffect(() => {
    const el = scrollerRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, shownMode]);

  // Keyboard inside the open dock: Tab stays within it, Esc denies a pending
  // approval or interrupts the answer.
  const onPanelKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape' && live) {
      e.preventDefault();
      if (waiting) approve(waiting.id, false);
      else cancel();
      return;
    }
    if (e.key !== 'Tab' || !panelRef.current) return;
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

  const busyElsewhere = streamingConversationId !== null && !isStreaming;

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

  if (shownMode === 'hidden') return null;

  if (shownMode === 'minimized') {
    return (
      <button
        type="button"
        onClick={() => changeMode('open')}
        aria-label={`Open the conversation${working ? `, working: ${working.label}` : ''}${unread > 0 ? `, ${unread} new` : ''} (Ctrl+J)`}
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

  return (
    <div
      ref={panelRef}
      role="dialog"
      aria-modal="false"
      aria-label="Conversation"
      onKeyDown={onPanelKeyDown}
      className="dock-pop fixed bottom-4 right-4 z-40 w-[380px] h-[470px] max-h-[calc(100vh-32px)] flex flex-col rounded-2xl border border-shodh-border-strong bg-shodh-ground shadow-[0_16px_48px_rgba(0,0,0,0.38)] overflow-hidden"
    >
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
          {[
            { label: 'Open in Ask', icon: Maximize2, onClick: onExpand },
            { label: 'Minimise (Ctrl+J)', icon: Minus, onClick: () => changeMode('minimized') },
            { label: 'Close', icon: X, onClick: () => changeMode('hidden') },
          ].map(({ label, icon: Icon, onClick }) => (
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
          blockedReason={busyElsewhere ? 'An answer is running in another conversation.' : null}
          placeholder="Reply…"
          compact
        />
      </div>
    </div>
  );
}

export default ConversationDock;

import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { connectorName } from '../../features/connectors/conversations';
import { Maximize2, Minus, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import type { ViewTab } from '../../lib/viewTabs';
import { RETIRED_DOCK_STORAGE_KEYS, dockLayout } from '../../lib/dockMode';
import type { DockLayout } from '../../lib/dockMode';
import { AgentComposer } from '../../features/agent/AgentComposer';
import type { AgentComposerHandle } from '../../features/agent/AgentComposer';
import { SnippetDropZone } from '../../features/research/SnippetDropZone';
import { appendToDraft } from '../../features/research/snippetModel';
import { passageHits } from '../../features/agent/citations';
import { currentStep, isLive, pendingApproval } from '../../features/agent/reducer';
import { StatusLine } from '../../features/agent/StatusLine';
import { Transcript } from '../../features/agent/Transcript';
import { SetupCard } from '../../features/setup/SetupCard';
import { Kbd, IS_MAC } from '../ui/Kbd';
import { ariaKeyShortcuts, matchesShortcut, withShortcut } from '../../lib/shortcuts';
import { useQueuedQuestion } from '../../features/setup/useQueuedQuestion';
import { useChatSession } from '../../features/ask/ChatSessionContext';
import { useDraft } from '../../features/ask/useDraft';
import { composerDrafts } from '../../features/ask/draftStore';
import { useSendOptions } from '../../features/ask/useSendOptions';
import { AttachmentChips, FileRefChip, useChatFileScope, useImageAttachments } from '../../features/ask/AttachmentChips';
import type { ConversationFile } from '../../features/ask/fileQuestion';
import { attachedContextOf, composerAttachments } from '../../features/ask/imageAttachments';
import { composerFiles, limitedToFiles } from '../../features/ask/fileQuestion';
import { missingIndexedFiles, reportMissingFiles } from '../../features/ask/fileQuestionApi';
import type { OptionSource } from '../../features/ask/sendOptions';
import type { ChatMessage } from '../../features/ask/types';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

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

function removeRetiredRecords() {
  try {
    RETIRED_DOCK_STORAGE_KEYS.forEach(key => window.localStorage.removeItem(key));
  } catch {
    // Storage unavailable (private mode): nothing was stored either.
  }
}

/** The window's size, so the floating chat stays inside it as it is resized. */
function useWindowSize(): { width: number; height: number } {
  const [size, setSize] = useState(() => ({ width: window.innerWidth, height: window.innerHeight }));
  useEffect(() => {
    const onResize = () => setSize({ width: window.innerWidth, height: window.innerHeight });
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);
  return size;
}

/** Grows with every new message, step and text segment: the unread measure. */
function activitySize(messages: readonly ChatMessage[]): number {
  let size = messages.length;
  for (const m of messages) size += m.transcript?.blocks.length ?? 0;
  return size;
}

interface ConversationDockProps {
  activeTab: ViewTab;
  /** The Library's sources: replies search the included ones, as in Ask. */
  sources: readonly OptionSource[];
  /** Return to the Ask view (full conversation). */
  onExpand: () => void;
}

/**
 * The active conversation while another view is open.
 *
 * Minimised it is a pill (bottom-right) with a working indicator and an unread
 * count. Expanded (pill, Ctrl+J, or the assistant opening a view for the user
 * to watch) it floats over the page in that corner, kept below the page's
 * toolbar, at every window size: it never takes a column. Working on the page
 * leaves it open; Esc in it or Minimise collapses it. Layout rules live in
 * `lib/dockMode`.
 */
export function ConversationDock({ activeTab, sources, onExpand }: ConversationDockProps) {
  const session = useChatSession();
  const { messages, isStreaming, navigation, steer, send, cancel, approve, setRuntimeInstalled, streamingConversationId, sideRun } = session;
  /** Expanded or collapsed on a view; only applies while that view stays open. */
  const [override, setOverride] = useState<{ tab: ViewTab; open: boolean } | null>(null);
  const [dismissed, setDismissed] = useState(false);
  useEffect(removeRetiredRecords, []);
  const [overlayTop, setOverlayTop] = useState(OVERLAY_TOOLBAR_RESERVE);
  // The same draft as the Ask composer: "Open in Ask" keeps what was typed here.
  const [draft, setDraft] = useDraft(session.activeConversationId);
  const sendOptions = useSendOptions(sources);
  // Image text attached in Ask goes out with a reply sent from here too.
  const attachments = useImageAttachments(session.activeConversationId);
  // As in Ask: an "About <file>" chat stays limited to that file until its chip is removed.
  const saveAboutFile = useCallback((file: ConversationFile | null) => {
    const id = session.activeConversationId;
    if (id) session.updateConversationMeta(id, { aboutFile: file ?? undefined });
  }, [session.activeConversationId, session.updateConversationMeta]);
  const fileScope = useChatFileScope(session.activeConversationId, session.activeConversation?.aboutFile, saveAboutFile);
  const aboutFile = fileScope.file;
  // Read after the file check: a conversation switch meanwhile cancels the send.
  const shownConversationRef = useRef(session.activeConversationId);
  shownConversationRef.current = session.activeConversationId;
  const checkingFileRef = useRef(false);
  const [unreadBase, setUnreadBase] = useState<number | null>(null);
  const panelRef = useRef<HTMLElement | null>(null);
  const pillRef = useRef<HTMLButtonElement>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const composerRef = useRef<AgentComposerHandle>(null);
  const focusComposerRef = useRef(false);
  const focusPillRef = useRef(false);
  const windowSize = useWindowSize();
  const offAsk = activeTab !== 'ask';

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
  // dock back if it was closed, as a pill, so the page is not covered unasked.
  const lastNavSeq = useRef(navigation?.seq ?? 0);
  useEffect(() => {
    if (!navigation || navigation.seq === lastNavSeq.current) return;
    lastNavSeq.current = navigation.seq;
    setDismissed(false);
    // A Creator build the user just started opened Artifacts: the chat opens
    // alongside it (for this visit), so they can steer while they watch.
    if (navigation.openDock) setOverride({ tab: navigation.view, open: true });
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
  // Each visit starts minimised; an expand or collapse made on another view
  // does not carry over.
  const open = override !== null && override.tab === activeTab && override.open;
  const layout: DockLayout | null = visible ? dockLayout(open) : null;

  // Ctrl+J expands or minimises the dock (outside Ask, where the conversation is already shown).
  useEffect(() => {
    if (!offAsk) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (!matchesShortcut(e, 'toggleDock')) return;
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

  // Focus follows explicit actions only: the chat the assistant opens beside a
  // view it navigated to must not take focus from the page.
  useEffect(() => {
    if (layout === 'overlay') {
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
  }, [layout, windowSize]);

  // Keyboard inside the dock: Esc denies a pending approval or interrupts the
  // answer; with nothing running, Esc collapses it. It does not hold focus: the
  // page stays usable while it floats, so Tab moves in and out of it freely.
  const onPanelKeyDown = (e: React.KeyboardEvent<HTMLElement>) => {
    if (e.key === 'Escape') {
      if (live) {
        e.preventDefault();
        if (waiting) approve(waiting.id, false);
        else cancel();
        return;
      }
      e.preventDefault();
      collapse(true);
    }
  };

  const busyElsewhere = (streamingConversationId !== null && !isStreaming) || sideRun !== null;
  // A connector's conversation is read-only here too.
  const readOnlySource = session.activeConversation?.source ?? null;

  // As in Ask: a question asked before Shodh is ready waits for setup.
  const queued = useQueuedQuestion(session.activeConversationId);

  const submit = () => {
    const text = draft.trim();
    if (!text || isStreaming || busyElsewhere || checkingFileRef.current) return;
    // As in Ask: the attached image text goes beside the typed words, and both are
    // cleared only once the question is accepted.
    const conversationId = session.activeConversationId;
    const options = sendOptions;
    const sendComposer = (): boolean => {
      const current = composerDrafts.get(conversationId).trim();
      if (!current) return true;
      const queuedFile = composerFiles.get(conversationId);
      const attached = composerAttachments.get(conversationId);
      if (!send(current, limitedToFiles(options, queuedFile ? [queuedFile.path] : []), { attachedContext: attachedContextOf(attached) })) return false;
      composerDrafts.set(conversationId, '');
      composerAttachments.consume(conversationId, attached);
      if (queuedFile) fileScope.keep(queuedFile);
      return true;
    };
    // As in Ask: before Shodh is ready the question waits for setup.
    if (queued.hold(sendComposer)) return;
    // The file the question is about ("Ask about this file") goes with it as its search limit.
    const file = composerFiles.get(conversationId);
    const go = () => {
      sendComposer();
    };
    if (!file) {
      go();
      return;
    }
    // As in Ask: a file removed or renamed since it was chosen is said plainly.
    checkingFileRef.current = true;
    void missingIndexedFiles([file.path])
      .then(missing => {
        if (shownConversationRef.current !== conversationId) return;
        if (missing.length === 0) go();
        else reportMissingFiles(missing, false, fileScope.remove);
      })
      .finally(() => {
        checkingFileRef.current = false;
      });
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
        aria-label={`Open the conversation${waiting ? ', needs your approval' : working ? `, working: ${working.label}` : live ? ', working' : ''}${unread > 0 ? `, ${unread} new` : ''}`}
        aria-keyshortcuts={ariaKeyShortcuts('toggleDock', IS_MAC)}
        title={withShortcut('Open the conversation', 'toggleDock', IS_MAC)}
        className={cn(
          'dock-pop fixed bottom-4 right-4 z-40 inline-flex items-center gap-2 h-10 max-w-[320px] pl-3 pr-3.5 rounded-full border border-shodh-border-strong bg-shodh-surface shadow-[0_8px_28px_rgba(0,0,0,0.3)] text-small text-shodh-text hover:bg-shodh-raised transition-colors duration-micro',
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
          <span className="shrink-0 min-w-[18px] h-[18px] px-1 rounded-full bg-shodh-accent text-shodh-on-accent text-caption font-bold leading-[18px] text-center tabular-nums" aria-hidden="true">
            {unread > 99 ? '99+' : unread}
          </span>
        )}
        <Kbd shortcut="toggleDock" className="ml-0.5" />
      </button>
    );
  }

  const controls = [
    { label: 'Open in Ask', icon: Maximize2, onClick: onExpand },
    { label: withShortcut('Minimise', 'toggleDock', IS_MAC), icon: Minus, onClick: () => collapse(true) },
    { label: 'Close', icon: X, onClick: () => setDismissed(true) },
  ];

  const body = (
    <>
      <header className="shrink-0 flex items-center gap-2 h-11 pl-3.5 pr-1.5 border-b border-shodh-border bg-shodh-surface">
        <span
          className={cn('w-2 h-2 rounded-full shrink-0', live ? 'bg-shodh-accent-text ask-breathe' : 'bg-shodh-success')}
          aria-hidden="true"
        />
        <span className="text-small font-semibold text-shodh-text shrink-0">Assistant</span>
        <span className="min-w-0 truncate text-caption text-shodh-text-muted">
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
          <div className="self-end max-w-[85%] px-3 py-2 rounded-[14px] bg-shodh-raised-2 text-small leading-[1.5] text-shodh-text whitespace-pre-wrap break-words">
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
        <SetupCard compact waiting={queued.waiting} onCancelWaiting={queued.cancel} />
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
            blockedReason={readOnlySource
              ? `This conversation happened in ${connectorName(readOnlySource.connector)}. Open it in Ask and choose Continue in Ask to reply.`
              : queued.waiting ? 'Your question is sent as soon as Shodh is ready.'
              : sideRun ? `Answering your side question about “${sideRun.label}”.` : busyElsewhere ? 'An answer is running in another conversation.' : null}
            placeholder="Reply…"
            attachments={(
              <>
                {aboutFile && <FileRefChip conversationId={session.activeConversationId} file={aboutFile} compact onRemove={fileScope.remove} />}
                <AttachmentChips conversationId={session.activeConversationId} attachments={attachments} compact />
              </>
            )}
            compact
          />
        </SnippetDropZone>
      </div>
    </>
  );

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

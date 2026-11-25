import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { ViewTab } from '../../lib/viewTabs';
import { RETIRED_DOCK_STORAGE_KEYS, dockLayout } from '../../lib/dockMode';
import type { DockLayout } from '../../lib/dockMode';
import type { AgentComposerHandle } from '../../features/agent/AgentComposer';
import { SnippetDropZone } from '../../features/research/SnippetDropZone';
import { appendToDraft } from '../../features/research/snippetModel';
import { SetupCard } from '../../features/setup/SetupCard';
import { matchesShortcut } from '../../lib/shortcuts';
import { runtimeSetupState, useRuntimeSetup } from '../../features/setup/runtimeSetupHost';
import { useChatSession } from '../../features/ask/ChatSessionContext';
import { useDraft } from '../../features/ask/useDraft';
import { AttachmentChips, FileRefChip } from '../../features/ask/AttachmentChips';
import { useDraftSubmit } from '../../features/dock/useDraftSubmit';
import type { OptionSource } from '../../features/ask/sendOptions';
import type { ChatMessage } from '../../features/ask/types';
import { useTheme } from '../../contexts/ThemeContext';
import { snapshotOf } from '../../lib/dockProtocol';
import type { DockAction } from '../../lib/dockProtocol';
import { DockPanel, blockedReasonOf } from './DockPanel';
import { useConversationModes } from '../../features/ask/useConversationModes';
import { ModelChip } from '../../features/modelPicker/ModelChip';
import { useModelLabel } from '../../features/modelPicker/useModelLabel';
import { useSendKey } from '../../hooks/useSendKey';
import { RUN_LOCKED_REASON } from '../../features/agent/composerRow';
import type { DockModel } from './DockPanel';

/**
 * Height kept clear below the top of the page content for the overlay. Every
 * non-Ask view puts its primary actions at the top of the page (Library's
 * "Add folder" header, the Tasks layout switch and list toolbar, the Activity
 * and Settings headers) and none has a bottom action bar, so bounding the
 * overlay's top below them keeps those actions uncovered.
 */
const OVERLAY_TOOLBAR_RESERVE = 112;
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
 * `lib/dockMode`; the markup, shared with the floating chat over the embedded
 * preview, in `DockPanel`.
 */
export function ConversationDock({ activeTab, sources, onExpand }: ConversationDockProps) {
  const session = useChatSession();
  const { messages, isStreaming, navigation, steer, cancel, approve, setRuntimeInstalled, streamingConversationId, sideRun } = session;
  const { theme } = useTheme();
  const runtime = useRuntimeSetup();
  /** Expanded or collapsed on a view; only applies while that view stays open. */
  const [override, setOverride] = useState<{ tab: ViewTab; open: boolean } | null>(null);
  const [dismissed, setDismissed] = useState(false);
  useEffect(removeRetiredRecords, []);
  const [overlayTop, setOverlayTop] = useState(OVERLAY_TOOLBAR_RESERVE);
  // The same draft as the Ask composer: "Open in Ask" keeps what was typed here.
  const [draft, setDraft] = useDraft(session.activeConversationId);
  // Sending, with Ask's rules (attachments, the file limit, setup), as the floating chat does.
  const { submit: submitDraft, attachments, fileScope, queued } = useDraftSubmit(sources);
  const aboutFile = fileScope.file;
  const [unreadBase, setUnreadBase] = useState<number | null>(null);
  const pillRef = useRef<HTMLButtonElement>(null);
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

  const visible = offAsk && !dismissed && messages.length > 0;
  // The composer's mode menu and model, as Ask shows them.
  const modes = useConversationModes();
  const modelLabel = useModelLabel(visible);
  const { sendKey } = useSendKey();
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

  // Overlay: keep its top below the page's toolbar, measured from the content area.
  useLayoutEffect(() => {
    if (layout !== 'overlay') return;
    const main = document.getElementById('main-content');
    const top = main ? main.getBoundingClientRect().top : 0;
    setOverlayTop(Math.max(0, Math.round(top)) + OVERLAY_TOOLBAR_RESERVE);
  }, [layout, windowSize]);

  // A connector's conversation is read-only here too.
  const readOnlySource = session.activeConversation?.source ?? null;

  const submitSteer = () => {
    const text = draft.trim();
    if (!text || !isStreaming) return;
    setDraft('');
    steer(text);
  };

  // The in-page chat reads main's state whole (no relay, nothing left out).
  const snapshot = useMemo(() => snapshotOf({
    conversationId: session.activeConversationId,
    title: session.activeConversation?.title ?? '',
    messages,
    isStreaming,
    streamingConversationId,
    sideRun,
    readOnlyConnector: readOnlySource?.connector ?? null,
    draft,
    dictating: false,
    // The in-page chat offers no microphone (Ask does).
    voiceSupported: false,
    open,
    unread,
    theme,
    runtime: runtimeSetupState(runtime),
    work: modes.work,
    permission: modes.permission,
    codeAvailable: modes.codeAvailable,
    model: modelLabel,
    sendKey,
  }, false), [session.activeConversationId, session.activeConversation?.title, messages, isStreaming, streamingConversationId, sideRun, readOnlySource?.connector, draft, open, unread, theme, runtime, modes.work, modes.permission, modes.codeAvailable, modelLabel, sendKey]);

  const onAction = (action: DockAction) => {
    switch (action.kind) {
      case 'send': submitDraft(draft); break;
      case 'steer': submitSteer(); break;
      case 'cancel': cancel(); break;
      case 'approve': approve(action.id, action.allow, action.forConversation, action.reviewToken); break;
      case 'draft': setDraft(action.text); break;
      case 'open':
        if (action.open) expand();
        else collapse(true);
        break;
      case 'close': setDismissed(true); break;
      case 'openInAsk': onExpand(); break;
      case 'runtimeInstalled': setRuntimeInstalled(action.installed); break;
      case 'openSettings': window.dispatchEvent(new CustomEvent('switchTab', { detail: 'settings' })); break;
      case 'setWork': modes.pickWork(action.work); break;
      case 'setPermission': modes.changeAgentMode(action.mode); break;
      // The in-page chat's runtime card, review button, web sources and commands act in
      // this window directly; only the floating chat sends these.
      default: break;
    }
  };

  if (layout === null) return null;

  const model: DockModel = {
    snapshot,
    onAction,
    blockedReason: blockedReasonOf(snapshot, queued.waiting),
    setupCard: <SetupCard compact waiting={queued.waiting} onCancelWaiting={queued.cancel} />,
    composerExtras: (
      <>
        {aboutFile && <FileRefChip conversationId={session.activeConversationId} file={aboutFile} compact onRemove={fileScope.remove} />}
        <AttachmentChips conversationId={session.activeConversationId} attachments={attachments} compact />
      </>
    ),
    // The model menu itself, as in Ask (this chat runs in the main window).
    modelControl: (
      <ModelChip
        answerRunning={isStreaming}
        takesPickerRequests={false}
        lockedReason={isStreaming ? RUN_LOCKED_REASON : null}
        onOpenSettings={() => window.dispatchEvent(new CustomEvent('switchTab', { detail: 'settings' }))}
      />
    ),
    wrapComposer: composer => (
      <SnippetDropZone onInsert={text => setDraft(prev => appendToDraft(prev, text))}>{composer}</SnippetDropZone>
    ),
  };

  return (
    <DockPanel
      model={model}
      onExpandAsk={onExpand}
      fixed
      maxHeight={`calc(100vh - ${overlayTop + OVERLAY_EDGE}px)`}
      pillRef={pillRef}
      composerRef={composerRef}
    />
  );
}

export default ConversationDock;

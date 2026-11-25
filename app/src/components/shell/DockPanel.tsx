import React, { useLayoutEffect, useMemo, useRef } from 'react';
import { History, Maximize2, Minus, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { connectorName } from '../../features/connectors/conversations';
import { AgentComposer } from '../../features/agent/AgentComposer';
import type { AgentComposerHandle } from '../../features/agent/AgentComposer';
import { passageHits } from '../../features/agent/citations';
import { currentStep, isLive, pendingApproval } from '../../features/agent/reducer';
import { StatusLine } from '../../features/agent/StatusLine';
import { ModePicker } from '../../features/agent/ModePicker';
import { Transcript } from '../../features/agent/Transcript';
import { Kbd, IS_MAC } from '../ui/Kbd';
import { ariaKeyShortcuts, withShortcut } from '../../lib/shortcuts';
import type { DockAction, DockSnapshot } from '../../lib/dockProtocol';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

/** In the page, pill and panel sit in the window's bottom-right corner, over the page. */
const CORNER = 'fixed bottom-4 right-4 z-40';

/** The open panel's size in the page (the floating chat's webview is sized to the same). */
export const PANEL_WIDTH = 380;
export const PANEL_HEIGHT = 470;
export const PANEL_EDGE = 16;

/**
 * What the chat shows and does, from either host: the in-page chat builds it from main's own
 * session, the floating chat over the preview (`features/dock/DockView`) from main's snapshot,
 * sending every action back to main.
 */
export interface DockModel {
  snapshot: DockSnapshot;
  onAction: (action: DockAction) => void;
  /** Why sending is blocked now, if it is (`blockedReasonOf`). */
  blockedReason: string | null;
  /** Shown in the composer above the input (the in-page chat's attachment and file chips). */
  composerExtras?: React.ReactNode;
  /** Shown above the status line (the in-page chat's setup card). */
  setupCard?: React.ReactNode;
  /** Wraps the composer (the in-page chat's snippet drop zone). */
  wrapComposer?: (composer: React.ReactNode) => React.ReactNode;
  /**
   * The composer row's model control: the in-page chat's own model menu, or the floating
   * chat's trigger, which asks main to show Ask with its model menu open.
   */
  modelControl: React.ReactNode;
}

/** Why the chat cannot send now, as the in-page chat has always said it. */
export function blockedReasonOf(snapshot: DockSnapshot, queuedWaiting: boolean): string | null {
  if (snapshot.readOnlyConnector) {
    return `From ${connectorName(snapshot.readOnlyConnector)}: reply with Continue in Ask.`;
  }
  if (queuedWaiting) return 'Sends when Shodh is ready.';
  if (snapshot.sideRunLabel) return `Answering your side question about “${snapshot.sideRunLabel}”.`;
  if (snapshot.busyElsewhere) return 'Answering in another conversation.';
  return null;
}

interface DockPanelProps {
  model: DockModel;
  /** "Open in Ask", and a citation or the earlier work of a long answer. */
  onExpandAsk: () => void;
  /** In the page (fixed in the corner), or filling the floating chat's own webview. */
  fixed: boolean;
  /** The panel's height cap in the page (kept below the page's toolbar). */
  maxHeight?: string;
  pillRef?: React.Ref<HTMLButtonElement>;
  panelRef?: React.Ref<HTMLDivElement>;
  composerRef?: React.Ref<AgentComposerHandle>;
}

/**
 * The conversation's last exchange, as a pill (minimised) or a floating panel (open). One
 * markup for the in-page chat and the floating chat over the embedded preview, so the two
 * never show it differently.
 */
export function DockPanel({ model, onExpandAsk, fixed, maxHeight, pillRef, panelRef, composerRef }: DockPanelProps) {
  const { snapshot, onAction } = model;
  const transcript = snapshot.transcript;
  const scrollerRef = useRef<HTMLDivElement>(null);
  const hits = useMemo(() => passageHits(transcript?.passages ?? []), [transcript?.passages]);
  const live = transcript !== null && isLive(transcript);
  const waiting = live && transcript ? pendingApproval(transcript) : null;
  const working = live && transcript ? currentStep(transcript) : null;

  useLayoutEffect(() => {
    const el = scrollerRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [transcript, snapshot.lastUser, snapshot.open]);

  if (!snapshot.open) {
    const unread = snapshot.unread;
    return (
      <button
        ref={pillRef}
        type="button"
        onClick={() => onAction({ kind: 'open', open: true })}
        aria-label={`Open the conversation${waiting ? ', needs your approval' : working ? `, working: ${working.label}` : live ? ', working' : ''}${unread > 0 ? `, ${unread} new` : ''}`}
        aria-keyshortcuts={ariaKeyShortcuts('toggleDock', IS_MAC)}
        title={withShortcut('Open the conversation', 'toggleDock', IS_MAC)}
        data-dock-box=""
        className={cn(
          'dock-pop inline-flex items-center gap-2 h-10 max-w-[320px] pl-3 pr-3.5 rounded-full border border-shodh-border-strong bg-shodh-surface text-small text-shodh-text hover:bg-shodh-raised transition-colors duration-micro',
          fixed ? cn(CORNER, 'shadow-[0_8px_28px_rgba(0,0,0,0.3)]') : 'whitespace-nowrap',
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
    { label: 'Open in Ask', icon: Maximize2, onClick: onExpandAsk },
    { label: withShortcut('Minimise', 'toggleDock', IS_MAC), icon: Minus, onClick: () => onAction({ kind: 'open', open: false }) },
    { label: 'Close', icon: X, onClick: () => onAction({ kind: 'close' }) },
  ];

  // Keyboard inside the chat: Esc denies a pending approval or interrupts the answer; with
  // nothing running, Esc minimises it. It does not hold focus: Tab moves in and out freely.
  const onPanelKeyDown = (e: React.KeyboardEvent<HTMLElement>) => {
    if (e.key !== 'Escape') return;
    e.preventDefault();
    if (live) {
      if (waiting) onAction({ kind: 'approve', id: waiting.id, allow: false });
      else onAction({ kind: 'cancel' });
      return;
    }
    onAction({ kind: 'open', open: false });
  };

  const decide = (stepId: string, approved: boolean, forConversation?: boolean, reviewToken?: string) => {
    onAction({
      kind: 'approve',
      id: stepId,
      allow: approved,
      ...(forConversation !== undefined ? { forConversation } : {}),
      ...(reviewToken !== undefined ? { reviewToken } : {}),
    });
  };

  const composer = (
    <AgentComposer
      id="dock-composer"
      ref={composerRef}
      value={snapshot.draft}
      onChange={text => onAction({ kind: 'draft', text })}
      onSubmit={() => onAction({ kind: 'send' })}
      onSteer={() => onAction({ kind: 'steer' })}
      onStop={() => onAction({ kind: 'cancel' })}
      onApprove={waiting?.tier === 'write' ? () => onAction({ kind: 'approve', id: waiting.id, allow: true }) : undefined}
      running={snapshot.isStreaming}
      canSteer={transcript?.status === 'running'}
      approvalPending={waiting !== null}
      blockedReason={model.blockedReason}
      placeholder="Reply…"
      attachments={model.composerExtras}
      voice={snapshot.voiceSupported
        ? { supported: true, listening: snapshot.dictating, toggle: () => onAction({ kind: 'dictate', on: !snapshot.dictating }) }
        : undefined}
      modeControl={(
        <ModePicker
          work={snapshot.work}
          permission={snapshot.permission}
          codeAvailable={snapshot.codeAvailable}
          onPickWork={work => onAction({ kind: 'setWork', work })}
          onPickPermission={mode => onAction({ kind: 'setPermission', mode })}
          answerRunning={snapshot.isStreaming}
        />
      )}
      modelControl={model.modelControl}
      compact
    />
  );

  return (
    <div
      ref={panelRef}
      role="dialog"
      aria-modal="false"
      aria-label="Conversation"
      onKeyDown={onPanelKeyDown}
      data-dock-box=""
      className={cn(
        'dock-pop flex flex-col rounded-2xl border border-shodh-border-strong bg-shodh-ground overflow-hidden',
        fixed ? cn(CORNER, 'shadow-[0_16px_48px_rgba(0,0,0,0.38)]') : 'w-screen h-screen',
      )}
      style={fixed
        ? { width: `min(${PANEL_WIDTH}px, calc(100vw - ${PANEL_EDGE * 2}px))`, height: PANEL_HEIGHT, maxHeight }
        : undefined}
    >
      <header className="shrink-0 flex items-center gap-2 h-11 pl-3.5 pr-1.5 border-b border-shodh-border bg-shodh-surface">
        <span
          className={cn('w-2 h-2 rounded-full shrink-0', live ? 'bg-shodh-accent-text ask-breathe' : 'bg-shodh-success')}
          aria-hidden="true"
        />
        <span className="text-small font-semibold text-shodh-text shrink-0">Assistant</span>
        <span className="min-w-0 truncate text-caption text-shodh-text-muted">{snapshot.title}</span>
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
        {snapshot.trimmed && (
          <button
            type="button"
            onClick={onExpandAsk}
            title="This answer is too long to show whole here"
            className={cn(
              'self-stretch inline-flex items-center gap-2 px-3 py-2 rounded-lg border border-dashed border-shodh-border text-caption text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
              FOCUS_RING,
            )}
          >
            <History className="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
            <span className="min-w-0 text-left">Earlier steps · Show in Ask</span>
          </button>
        )}
        {snapshot.lastUser && (
          <div className="self-end max-w-[85%] px-3 py-2 rounded-[14px] bg-shodh-raised-2 text-small leading-[1.5] text-shodh-text whitespace-pre-wrap break-words">
            {snapshot.lastUser.content}
          </div>
        )}
        {transcript && (
          <Transcript
            transcript={transcript}
            hits={hits}
            onOpenCitation={() => onExpandAsk()}
            onDecide={decide}
            onRuntimeInstalled={() => onAction({ kind: 'runtimeInstalled', installed: true })}
            onOpenSettings={() => onAction({ kind: 'openSettings' })}
            compact
          />
        )}
        {transcript && (
          <div data-testid="answer-status">
            <StatusLine transcript={transcript} fallbackModel={null} compact />
          </div>
        )}
      </div>

      <div className="shrink-0 border-t border-shodh-border bg-shodh-surface px-2.5 pt-2 pb-2.5 flex flex-col gap-1.5">
        {model.setupCard}
        {model.wrapComposer ? model.wrapComposer(composer) : composer}
      </div>
    </div>
  );
}

import React, { useEffect, useRef, useState } from 'react';
import { ArrowLeft, CheckCircle2, ChevronRight, MoreHorizontal, Unplug } from 'lucide-react';
import { cn } from '../../lib/utils';
import { slackApi, slackError } from './api';
import { notify } from '../../lib/notify';
import { SLACK_TABS, SLACK_TAB_LABELS, moveTab } from './navigation';
import { slackPill, slackTabBadges, stalePill, type SlackStatus, type SlackTabId } from './slackModel';
import { StaleNotice } from '../../components/ui/StaleNotice';
import { SlackMark } from './SlackLogo';
import { SlackSetup, type WizardState } from './SlackSetup';
import { AnsweringTab, IndexingTab, MemoriesTab, OverviewTab, SettingsTab, useWorkspaceListings } from './SlackTabs';
import { Callout, DANGER_BUTTON, OUTLINE_BUTTON, PRIMARY_BUTTON, QUIET_BUTTON, Skeleton, StatusPill, TABLIST, tabClass } from './ui';
import type { SlackStatusState } from './useSlackStatus';

/** "Disconnect Acme?": keep what was indexed, or forget all of it. */
function DisconnectConfirm({
  status,
  busy,
  onDone,
  run,
}: {
  status: SlackStatus;
  busy: boolean;
  onDone: () => void;
  run: (forget: boolean) => void;
}) {
  const channels = status.index.channels.length;
  const memories = status.index.memories;
  return (
    <div
      role="alertdialog"
      aria-label={`Disconnect ${status.installation?.teamName ?? 'Slack'}`}
      onKeyDown={e => { if (e.key === 'Escape') { e.preventDefault(); onDone(); } }}
      className="flex flex-col gap-3 rounded-xl border border-shodh-error/40 bg-shodh-surface p-4"
    >
      <div>
        <p className="m-0 text-body font-semibold text-shodh-text">Disconnect {status.installation?.teamName ?? 'Slack'}?</p>
        <p className="m-0 mt-0.5 text-small text-shodh-text-secondary max-w-[68ch]">
          The tokens are deleted from this computer and the bot stops answering. Keep what was indexed from Slack (
          {channels} {channels === 1 ? 'channel' : 'channels'}, {memories} {memories === 1 ? 'memory' : 'memories'}), or forget all of it?
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <button type="button" className={OUTLINE_BUTTON} disabled={busy} autoFocus onClick={() => run(false)}>
          Disconnect and keep it
        </button>
        <button type="button" className={DANGER_BUTTON} disabled={busy} onClick={() => run(true)}>
          Disconnect and forget everything from Slack
        </button>
        <button type="button" className={QUIET_BUTTON} disabled={busy} onClick={onDone}>Cancel</button>
      </div>
    </div>
  );
}

/** The overflow menu of the page header. */
function HeaderMenu({ onDisconnect }: { onDisconnect: () => void }) {
  const [open, setOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  return (
    <div ref={menuRef} className="relative shrink-0">
      <button
        ref={buttonRef}
        type="button"
        className={cn(QUIET_BUTTON, 'w-9 h-9 justify-center px-0')}
        aria-label="Slack options"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen(o => !o)}
      >
        <MoreHorizontal className="w-4 h-4" aria-hidden="true" />
      </button>
      {open && (
        <div
          role="menu"
          aria-label="Slack options"
          className="shell-pop absolute right-0 top-10 z-50 min-w-[200px] py-1 rounded-lg border border-shodh-border-strong bg-shodh-raised shadow-lg"
          onKeyDown={e => {
            if (e.key === 'Escape') {
              e.preventDefault();
              setOpen(false);
              buttonRef.current?.focus();
            } else if (e.key === 'Tab') {
              setOpen(false);
            }
          }}
        >
          <button
            type="button"
            role="menuitem"
            autoFocus
            className="w-full flex items-center gap-2 px-3 py-1.5 text-small text-left text-shodh-error hover:bg-shodh-raised-2 focus-visible:bg-shodh-raised-2 focus-visible:outline-none"
            onClick={() => {
              setOpen(false);
              onDisconnect();
            }}
          >
            <Unplug className="w-3.5 h-3.5" aria-hidden="true" /> Disconnect…
          </button>
        </div>
      )}
    </div>
  );
}

function Breadcrumb({ onBack }: { onBack: () => void }) {
  return (
    <nav aria-label="Breadcrumb" className="flex items-center gap-1 text-small">
      <button type="button" className={cn(QUIET_BUTTON, '-ml-2.5')} onClick={onBack} aria-label="Back to all connectors">
        <ArrowLeft className="w-3.5 h-3.5" aria-hidden="true" />
        Connectors
      </button>
      <ChevronRight className="w-3.5 h-3.5 text-shodh-text-faint" aria-hidden="true" />
      <span aria-current="page" className="text-shodh-text font-medium">Slack</span>
    </nav>
  );
}

/** The page while the status loads: its shape, not a spinner. */
function PageSkeleton() {
  return (
    <div className="flex flex-col gap-5" aria-busy="true" aria-label="Loading Slack">
      <div className="flex items-center gap-3">
        <SlackMark size="lg" />
        <div className="flex flex-col gap-2 flex-1">
          <Skeleton className="h-5 w-24" />
          <Skeleton className="h-3.5 w-48" />
        </div>
      </div>
      <Skeleton className="h-9 w-full max-w-xl" />
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
        <Skeleton className="h-24" />
        <Skeleton className="h-24" />
      </div>
    </div>
  );
}

/**
 * The Slack page: a guided setup until Slack is connected, then its sections in tabs.
 * Disconnect lives in the header's menu and on the Settings tab, with the same confirm.
 */
export function SlackPage({
  slack,
  wizard,
  onWizard,
  tab,
  onTab,
  onBack,
}: {
  slack: SlackStatusState;
  wizard: WizardState;
  onWizard: (next: WizardState) => void;
  tab: SlackTabId;
  onTab: (tab: SlackTabId) => void;
  onBack: () => void;
}) {
  const { status, loadError, setStatus, retry, now } = slack;
  const [busy, setBusy] = useState(false);
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);
  /** Set by a successful Connect, until the user moves on. */
  const [justConnected, setJustConnected] = useState(false);
  const tabRefs = useRef<Partial<Record<SlackTabId, HTMLButtonElement | null>>>({});
  const confirmRef = useRef<HTMLDivElement>(null);
  const workspaces = useWorkspaceListings();

  useEffect(() => {
    if (confirmDisconnect) confirmRef.current?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }, [confirmDisconnect]);

  const disconnect = async (forget: boolean) => {
    setBusy(true);
    try {
      setStatus(await slackApi.disconnect(forget));
      setConfirmDisconnect(false);
      setJustConnected(false);
      notify.success(forget ? 'Slack disconnected; everything from it is forgotten' : 'Slack disconnected');
    } catch (err) {
      notify.error('Slack could not be disconnected', { description: slackError(err) });
    } finally {
      setBusy(false);
    }
  };

  if (!status) {
    return (
      <div className="flex flex-col gap-5">
        <Breadcrumb onBack={onBack} />
        {loadError ? (
          <Callout tone="error" role="alert">
            <p className="m-0">The Slack connector could not be loaded: {loadError}</p>
            <button type="button" className={cn(OUTLINE_BUTTON, 'mt-2')} onClick={retry}>Try again</button>
          </Callout>
        ) : (
          <PageSkeleton />
        )}
      </div>
    );
  }

  // A refresh failed: the status shown is the last known one, not current.
  const pill = loadError ? stalePill(slackPill(status)) : slackPill(status);
  const connected = status.installation !== null;
  const badges = slackTabBadges(status, now);
  const tabProps = { status, onStatus: setStatus, busy, setBusy, now };

  return (
    <div className="flex flex-col gap-5">
      <Breadcrumb onBack={onBack} />
      {loadError && <StaleNotice error={loadError} onRetry={retry} />}

      <header className="flex items-start gap-3.5">
        <SlackMark size="lg" />
        <div className="flex-1 min-w-0">
          <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
            <h2 className="m-0 text-[20px] font-semibold text-shodh-text">Slack</h2>
            <span role="status" aria-live="polite">
              <StatusPill pill={pill} />
            </span>
          </div>
          <p className="m-0 mt-0.5 text-small text-shodh-text-secondary truncate">
            {status.installation
              ? `${status.installation.teamName} · @${status.installation.botName}`
              : 'Your team asks @shodh in Slack and gets cited answers from one workspace you choose.'}
          </p>
        </div>
        {connected && <HeaderMenu onDisconnect={() => setConfirmDisconnect(true)} />}
      </header>

      {confirmDisconnect && connected && (
        <div ref={confirmRef}>
          <DisconnectConfirm status={status} busy={busy} onDone={() => setConfirmDisconnect(false)} run={forget => void disconnect(forget)} />
        </div>
      )}

      {!connected ? (
        <SlackSetup
          status={status}
          wizard={wizard}
          onWizard={onWizard}
          onConnected={next => {
            setStatus(next);
            setJustConnected(true);
          }}
        />
      ) : justConnected ? (
        <section aria-labelledby="slack-connected" className="flex flex-col items-start gap-3 rounded-xl border border-shodh-border bg-shodh-surface p-6">
          <CheckCircle2 className="w-7 h-7 text-shodh-success" aria-hidden="true" />
          <div>
            <h3 id="slack-connected" className="m-0 text-[16px] font-semibold text-shodh-text">
              Connected to {status.installation?.teamName} as @{status.installation?.botName}
            </h3>
            <p className="m-0 mt-1 text-small text-shodh-text-secondary max-w-[60ch]">
              Next, pick the workspace Slack answers from and the channels it answers in. Until then the bot replies that it
              isn’t set up yet.
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              className={PRIMARY_BUTTON}
              autoFocus
              onClick={() => {
                setJustConnected(false);
                onTab('answering');
              }}
            >
              Choose what Slack can answer from
            </button>
            <button
              type="button"
              className={QUIET_BUTTON}
              onClick={() => {
                setJustConnected(false);
                onTab('overview');
              }}
            >
              Go to the overview
            </button>
          </div>
        </section>
      ) : (
        <>
          <div role="tablist" aria-label="Slack sections" className={TABLIST}>
            {SLACK_TABS.map(t => {
              const badge = badges[t];
              return (
                <button
                  key={t}
                  ref={el => { tabRefs.current[t] = el; }}
                  type="button"
                  role="tab"
                  id={`slack-tab-${t}`}
                  aria-selected={tab === t}
                  aria-controls={`slack-panel-${t}`}
                  tabIndex={tab === t ? 0 : -1}
                  onClick={() => onTab(t)}
                  onKeyDown={e => {
                    const next = moveTab(t, e.key);
                    if (!next) return;
                    e.preventDefault();
                    onTab(next);
                    tabRefs.current[next]?.focus();
                  }}
                  className={tabClass(tab === t)}
                >
                  {SLACK_TAB_LABELS[t]}
                  {badge && (
                    <span
                      className={cn(
                        'min-w-[18px] h-[18px] px-1 inline-flex items-center justify-center rounded-full text-caption font-semibold tabular-nums',
                        badge.tone === 'warning' ? 'bg-shodh-warning-soft text-shodh-warning' : 'bg-shodh-raised text-shodh-text-muted',
                      )}
                    >
                      {badge.count}
                      <span className="sr-only">{badge.tone === 'warning' ? ' need attention' : ''}</span>
                    </span>
                  )}
                </button>
              );
            })}
          </div>

          <div role="tabpanel" id={`slack-panel-${tab}`} aria-labelledby={`slack-tab-${tab}`} tabIndex={0} className="focus:outline-none">
            {tab === 'overview' && <OverviewTab {...tabProps} onTab={onTab} />}
            {tab === 'answering' && <AnsweringTab {...tabProps} workspaces={workspaces} />}
            {tab === 'indexing' && <IndexingTab {...tabProps} workspaces={workspaces} />}
            {tab === 'memories' && <MemoriesTab {...tabProps} />}
            {tab === 'settings' && <SettingsTab {...tabProps} onDisconnect={() => setConfirmDisconnect(true)} />}
          </div>
        </>
      )}
    </div>
  );
}

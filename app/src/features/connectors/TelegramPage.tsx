import React, { useEffect, useId, useRef, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { ArrowLeft, CheckCircle2, ChevronRight, ExternalLink, MoreHorizontal, Unplug, UserPlus, Users } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { SwitchRow } from '../../components/PrivacySettings';
import type { WorkspaceListing } from '../workspaces/types';
import { slackError } from './api';
import { TELEGRAM_TABS, TELEGRAM_TAB_LABELS, moveTab } from './navigation';
import { capShare, stalePill, validCap } from './slackModel';
import { StaleNotice } from '../../components/ui/StaleNotice';
import { telegramApi } from './telegramApi';
import {
  TELEGRAM_MODEL_NOTE,
  TELEGRAM_RUNNING_HINT,
  groupPrivacyLine,
  guestSourceLine,
  lastUpdateLine,
  ownerLine,
  telegramNextSteps,
  telegramPill,
  telegramStateLine,
  telegramTabBadges,
  telegramUsageLine,
  waitingGroups,
  type TelegramStatus,
  type TelegramTabId,
} from './telegramModel';
import { LinkCard } from './TelegramLink';
import { TelegramMark } from './TelegramLogo';
import { TelegramSetup, type TelegramWizardState } from './TelegramSetup';
import { useWorkspaceListings } from './SlackTabs';
import {
  Callout,
  DANGER_BUTTON,
  INPUT,
  LearnMore,
  OUTLINE_BUTTON,
  PRIMARY_BUTTON,
  QUIET_BUTTON,
  Section,
  Skeleton,
  StatusPill,
  TABLIST,
  TONE_DOT,
  TONE_TEXT,
  tabClass,
} from './ui';
import type { TelegramStatusState } from './useTelegramStatus';

interface TabProps {
  status: TelegramStatus;
  onStatus: (status: TelegramStatus) => void;
  busy: boolean;
  setBusy: (busy: boolean) => void;
  now: number;
}

/** Runs a command that returns the new status; a failure is a toast. */
function useRun({ onStatus, setBusy }: Pick<TabProps, 'onStatus' | 'setBusy'>) {
  return async (action: () => Promise<TelegramStatus>, failure: string): Promise<boolean> => {
    setBusy(true);
    try {
      onStatus(await action());
      return true;
    } catch (err) {
      notify.error(failure, { description: slackError(err) });
      return false;
    } finally {
      setBusy(false);
    }
  };
}

function Meter({ share, label }: { share: number; label: string }) {
  const percent = Math.round(share * 100);
  return (
    <div role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent} className="h-1.5 w-full rounded-full bg-shodh-raised-2 overflow-hidden">
      <div
        className={cn('h-full rounded-full', share >= 1 ? 'bg-shodh-error' : share >= 0.8 ? 'bg-shodh-warning' : 'bg-shodh-accent')}
        style={{ width: `${percent}%` }}
      />
    </div>
  );
}

function Panel({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5 rounded-lg border border-shodh-border-subtle bg-shodh-surface p-4 min-w-0">
      <p className="m-0 text-[12px] font-medium text-shodh-text-muted">{title}</p>
      {children}
    </div>
  );
}

// ── Overview ────────────────────────────────────────────────────────

function OverviewTab({ status, now, onTab }: TabProps & { onTab: (tab: TelegramTabId) => void }) {
  const line = telegramStateLine(status, now);
  const steps = telegramNextSteps(status);
  const on = status.state !== 'off' && status.state !== 'local_only';
  return (
    <div className="flex flex-col gap-6">
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
        <Panel title="Connection">
          <p className="m-0 flex items-center gap-2 text-[13.5px] font-semibold text-shodh-text">
            <span aria-hidden="true" className={cn('w-2 h-2 rounded-full shrink-0', TONE_DOT[line.tone])} />
            <span className="min-w-0 break-words">{line.title}</span>
          </p>
          {line.detail && <p className={cn('m-0 text-[12.5px] break-words', TONE_TEXT[line.tone])}>{line.detail}</p>}
          <p className="m-0 text-[12px] text-shodh-text-muted">{lastUpdateLine(status, now)}</p>
          {on && <p className="m-0 text-[12px] text-shodh-text-muted">{TELEGRAM_RUNNING_HINT}</p>}
        </Panel>
        <Panel title="Your account">
          <p className="m-0 text-[13.5px] font-semibold text-shodh-text">{ownerLine(status)}</p>
          <p className="m-0 text-[12px] text-shodh-text-muted">
            {status.owner
              ? 'Your chat answers from all your workspaces and memories. It can’t change anything; open Shodh for that.'
              : 'Link it to ask Shodh from your phone.'}
          </p>
        </Panel>
        <Panel title="Answers to others today">
          <p className="m-0 text-[13.5px] font-semibold text-shodh-text tabular-nums">
            {status.answersToday} of {status.dailyCap}
          </p>
          <Meter share={capShare(status)} label="Answers to others used today" />
          <p className="m-0 text-[12px] text-shodh-text-muted">{telegramUsageLine(status, now)}</p>
        </Panel>
        <Panel title="People & groups">
          <p className="m-0 text-[13px] text-shodh-text">
            {status.people.length} {status.people.length === 1 ? 'person' : 'people'}, {status.groups.filter(g => g.allowed).length} allowed{' '}
            {status.groups.filter(g => g.allowed).length === 1 ? 'group' : 'groups'}
            {waitingGroups(status).length > 0 && `, ${waitingGroups(status).length} waiting`}
          </p>
          <button type="button" className={cn(QUIET_BUTTON, 'self-start -ml-2.5')} onClick={() => onTab('people')}>
            Open People & groups
          </button>
        </Panel>
      </div>

      {status.lastSendFailedAt && (
        <Callout tone="warning" role="alert">The last answer could not be sent to Telegram. The details are in the app log.</Callout>
      )}

      {steps.length > 0 ? (
        <Section title="Next steps">
          <ul className="m-0 p-0 list-none flex flex-col divide-y divide-shodh-border-subtle rounded-lg border border-shodh-border-subtle bg-shodh-surface">
            {steps.map(step => (
              <li key={step.id} className="flex flex-wrap sm:flex-nowrap items-start gap-3 px-4 py-3">
                <span aria-hidden="true" className={cn('mt-1.5 w-2 h-2 rounded-full shrink-0', TONE_DOT[step.tone])} />
                <div className="flex-1 min-w-0">
                  <p className="m-0 text-[13px] font-semibold text-shodh-text">{step.title}</p>
                  <p className="m-0 mt-0.5 text-[12.5px] text-shodh-text-muted break-words">{step.detail}</p>
                </div>
                {step.tab && step.action && (
                  <button type="button" className={cn(OUTLINE_BUTTON, 'shrink-0')} onClick={() => onTab(step.tab as TelegramTabId)}>
                    {step.action}
                  </button>
                )}
              </li>
            ))}
          </ul>
        </Section>
      ) : (
        <Callout tone="ok" role="status">Everything is set up. Ask Shodh from your Telegram chat.</Callout>
      )}
    </div>
  );
}

// ── People & groups ─────────────────────────────────────────────────

function OwnerSection(props: TabProps) {
  const { status, busy } = props;
  const run = useRun(props);
  const [confirm, setConfirm] = useState(false);
  if (!status.bot) return null;
  return (
    <Section
      title="You"
      description="Your own chat with the bot is your Shodh: all your workspaces and memories, the model you chose, with sources. It can’t change anything."
    >
      {status.owner ? (
        <div className="flex flex-wrap items-center gap-3 rounded-lg border border-shodh-border-subtle bg-shodh-surface px-4 py-3">
          <CheckCircle2 className="w-4 h-4 text-shodh-success shrink-0" aria-hidden="true" />
          <span className="text-[13px] font-medium text-shodh-text">{ownerLine(status)}</span>
          {confirm ? (
            <span role="alertdialog" aria-label="Unlink your account" className="ml-auto flex items-center gap-2">
              <span className="text-[12.5px] text-shodh-text-secondary">The bot will refuse this account.</span>
              <button
                type="button"
                className={DANGER_BUTTON}
                disabled={busy}
                autoFocus
                onClick={() => {
                  setConfirm(false);
                  void run(() => telegramApi.unlinkOwner(), 'Your account could not be unlinked');
                }}
              >
                Unlink
              </button>
              <button type="button" className={QUIET_BUTTON} onClick={() => setConfirm(false)}>Cancel</button>
            </span>
          ) : (
            <button type="button" className={cn(QUIET_BUTTON, 'ml-auto')} disabled={busy} onClick={() => setConfirm(true)}>
              Unlink
            </button>
          )}
        </div>
      ) : (
        <LinkCard kind="owner" botUsername={status.bot.username} disabled={status.state === 'local_only'} />
      )}
    </Section>
  );
}

function WorkspaceSection(props: TabProps & { workspaces: WorkspaceListing[] | null }) {
  const { status, busy, workspaces } = props;
  const run = useRun(props);
  const [pending, setPending] = useState<{ id: string; name: string } | null>(null);
  const pickerId = useId();
  const source = guestSourceLine(status);
  const chosen = status.answerWorkspace?.id ?? '';
  const chosenListed = workspaces?.some(w => w.id === chosen) ?? true;
  return (
    <Section title="Others are answered from" description="One workspace, for every person and group you allow. Never your memories.">
      <label htmlFor={pickerId} className="sr-only">Workspace others are answered from</label>
      {workspaces === null ? (
        <Skeleton className="h-9 max-w-sm" />
      ) : (
        <select
          id={pickerId}
          value={pending?.id ?? chosen}
          disabled={busy}
          onChange={e => {
            const id = e.target.value;
            if (!id) {
              setPending(null);
              void run(() => telegramApi.setAnswerWorkspace(null), 'The workspace could not be chosen');
              return;
            }
            setPending({ id, name: workspaces.find(w => w.id === id)?.name ?? 'that workspace' });
          }}
          className={cn(INPUT, 'max-w-sm')}
        >
          <option value="">Choose a workspace…</option>
          {!chosenListed && chosen && <option value={chosen}>{status.answerWorkspace?.name || 'Unavailable workspace'}</option>}
          {workspaces.map(w => (
            <option key={w.id} value={w.id}>{w.name}</option>
          ))}
        </select>
      )}
      {pending ? (
        <div role="alertdialog" aria-label="Confirm the workspace others are answered from" className="flex flex-col gap-2 rounded-lg border border-shodh-warning/40 bg-shodh-warning-soft px-3.5 py-3">
          <p className="m-0 text-[12.5px] text-shodh-text-secondary">
            Everyone you invite, and every group you allow, will get answers from the documents of “{pending.name}”.
          </p>
          <div className="flex items-center gap-2">
            <button
              type="button"
              className={PRIMARY_BUTTON}
              disabled={busy}
              autoFocus
              onClick={() => {
                const id = pending.id;
                setPending(null);
                void run(() => telegramApi.setAnswerWorkspace(id), 'The workspace could not be chosen');
              }}
            >
              Answer from this workspace
            </button>
            <button
              type="button"
              className={QUIET_BUTTON}
              onClick={() => setPending(null)}
              onKeyDown={e => { if (e.key === 'Escape') { e.preventDefault(); setPending(null); } }}
            >
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <p className={cn('m-0 text-[12.5px] max-w-[68ch]', TONE_TEXT[source.tone])}>{source.text}</p>
      )}
    </Section>
  );
}

function PeopleSection(props: TabProps) {
  const { status, busy } = props;
  const run = useRun(props);
  const [inviting, setInviting] = useState(false);
  if (!status.bot) return null;
  return (
    <Section
      title="People"
      description="Someone you invite asks in their own chat with the bot."
      actions={
        <button type="button" className={OUTLINE_BUTTON} disabled={busy || status.state === 'local_only'} onClick={() => setInviting(v => !v)} aria-expanded={inviting}>
          <UserPlus className="w-3.5 h-3.5" aria-hidden="true" />
          {inviting ? 'Hide the invite' : 'Invite someone'}
        </button>
      }
    >
      {inviting && <LinkCard kind="invite" botUsername={status.bot.username} />}
      {status.people.length === 0 ? (
        <p className="m-0 text-[12.5px] text-shodh-text-muted">Nobody else can ask. Invite someone to let them.</p>
      ) : (
        <ul className="m-0 p-0 list-none flex flex-col divide-y divide-shodh-border-subtle rounded-lg border border-shodh-border-subtle bg-shodh-surface">
          {status.people.map(p => (
            <li key={p.id} className="flex items-center gap-3 px-4 py-2.5">
              <span className="min-w-0 flex-1">
                <span className="block text-[13px] text-shodh-text truncate">{p.name}</span>
                {p.username && <span className="block text-[12px] text-shodh-text-muted truncate">@{p.username}</span>}
              </span>
              <button
                type="button"
                className={QUIET_BUTTON}
                disabled={busy}
                aria-label={`Remove ${p.name}`}
                onClick={() => void run(() => telegramApi.removePerson(p.id), `${p.name} could not be removed`)}
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
      )}
    </Section>
  );
}

function GroupsSection(props: TabProps) {
  const { status, busy } = props;
  const run = useRun(props);
  const waiting = waitingGroups(status);
  const allowed = status.groups.filter(g => g.allowed);
  return (
    <Section title="Groups" description={groupPrivacyLine(status)}>
      {status.groups.length === 0 ? (
        <p className="m-0 text-[12.5px] text-shodh-text-muted">
          Add the bot to a Telegram group and it appears here, waiting for you. Nobody there gets an answer until you allow it.
        </p>
      ) : (
        <ul className="m-0 p-0 list-none flex flex-col divide-y divide-shodh-border-subtle rounded-lg border border-shodh-border-subtle bg-shodh-surface">
          {[...waiting, ...allowed].map(g => (
            <li key={g.id} className="flex flex-wrap items-center gap-3 px-4 py-2.5">
              <Users className="w-4 h-4 shrink-0 text-shodh-text-muted" aria-hidden="true" />
              <span className="min-w-0 flex-1">
                <span className="block text-[13px] text-shodh-text truncate">{g.title}</span>
                <span className={cn('block text-[12px]', g.allowed ? 'text-shodh-text-muted' : 'text-shodh-warning')}>
                  {g.allowed ? 'Allowed' : `Waiting for you${g.addedBy ? `: added by ${g.addedBy}` : ''}`}
                </span>
              </span>
              {!g.allowed && (
                <button
                  type="button"
                  className={PRIMARY_BUTTON}
                  disabled={busy}
                  onClick={() => void run(() => telegramApi.setGroup(g.id, true), `“${g.title}” could not be allowed`)}
                >
                  Allow
                </button>
              )}
              <button
                type="button"
                className={QUIET_BUTTON}
                disabled={busy}
                aria-label={`Remove ${g.title}`}
                onClick={() => void run(() => telegramApi.setGroup(g.id, false), `“${g.title}” could not be removed`)}
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
      )}
    </Section>
  );
}

function CapSection(props: TabProps) {
  const { status, busy, now } = props;
  const run = useRun(props);
  const [capInput, setCapInput] = useState(String(status.dailyCap));
  const capId = useId();
  const helpId = useId();
  useEffect(() => setCapInput(String(status.dailyCap)), [status.dailyCap]);
  const cap = validCap(capInput, status);
  return (
    <Section
      title="Answers to others per day"
      description={<span id={helpId}>Each answer to someone else is one model call. At the limit the bot says so and makes no call. Your own chat has no limit.</span>}
    >
      <div className="flex flex-col gap-2 max-w-sm">
        <Meter share={capShare(status)} label="Answers to others used today" />
        <p className="m-0 text-[12px] text-shodh-text-muted">{telegramUsageLine(status, now)}</p>
      </div>
      <form
        className="flex items-center gap-2"
        onSubmit={e => {
          e.preventDefault();
          if (cap !== null && cap !== status.dailyCap) void run(() => telegramApi.setDailyCap(cap), 'The daily limit was not saved');
        }}
      >
        <label htmlFor={capId} className="text-[12.5px] text-shodh-text-secondary">Daily limit</label>
        <input
          id={capId}
          type="number"
          inputMode="numeric"
          min={status.minDailyCap}
          max={status.maxDailyCap}
          step={1}
          value={capInput}
          disabled={busy}
          aria-describedby={helpId}
          aria-invalid={cap === null}
          onChange={e => setCapInput(e.target.value)}
          className={cn(INPUT, 'w-24 tabular-nums')}
        />
        <button type="submit" className={OUTLINE_BUTTON} disabled={busy || cap === null || cap === status.dailyCap}>Save</button>
      </form>
      {cap === null && capInput.trim() !== '' && (
        <p role="alert" className="m-0 text-[12.5px] text-shodh-error">
          Enter a whole number from {status.minDailyCap} to {status.maxDailyCap}.
        </p>
      )}
    </Section>
  );
}

function PeopleTab(props: TabProps & { workspaces: WorkspaceListing[] | null }) {
  return (
    <div className="flex flex-col gap-8">
      <OwnerSection {...props} />
      <PeopleSection {...props} />
      <GroupsSection {...props} />
      <WorkspaceSection {...props} />
      <CapSection {...props} />
    </div>
  );
}

// ── Settings ────────────────────────────────────────────────────────

function SettingsTab(props: TabProps & { onDisconnect: () => void }) {
  const { status, busy, onDisconnect } = props;
  const run = useRun(props);
  const on = status.state !== 'off' && status.state !== 'local_only';
  return (
    <div className="flex flex-col gap-8">
      <SwitchRow
        label="Answer in Telegram"
        description="Shodh reads the bot’s messages from this computer and answers you, and anyone you allowed."
        checked={on}
        disabled={busy || status.state === 'local_only'}
        onChange={next => void run(() => telegramApi.setEnabled(next), 'The Telegram connector could not be switched')}
      />

      <Section title="Privacy">
        <div className="flex flex-col gap-3">
          <LearnMore summary="Telegram bot chats are not end-to-end encrypted.">{status.privacyNote}</LearnMore>
          <LearnMore summary="Messages go to your model provider.">{TELEGRAM_MODEL_NOTE}</LearnMore>
          <p className="m-0 text-[12.5px] text-shodh-text-muted">{TELEGRAM_RUNNING_HINT}</p>
        </div>
      </Section>

      <Section title="Disconnect" description="Deletes the bot token from this computer and stops the bot. You choose whether to keep the Telegram conversations in Shodh.">
        <div>
          <button type="button" className={DANGER_BUTTON} disabled={busy} onClick={onDisconnect}>
            Disconnect @{status.bot?.username ?? 'bot'}…
          </button>
        </div>
      </Section>
    </div>
  );
}

// ── The page ────────────────────────────────────────────────────────

function DisconnectConfirm({ status, busy, onDone, run }: { status: TelegramStatus; busy: boolean; onDone: () => void; run: (forget: boolean) => void }) {
  const bot = status.bot ? `@${status.bot.username}` : 'the bot';
  return (
    <div
      role="alertdialog"
      aria-label={`Disconnect ${bot}`}
      onKeyDown={e => { if (e.key === 'Escape') { e.preventDefault(); onDone(); } }}
      className="flex flex-col gap-3 rounded-xl border border-shodh-error/40 bg-shodh-surface p-4"
    >
      <div>
        <p className="m-0 text-[13.5px] font-semibold text-shodh-text">Disconnect {bot}?</p>
        <p className="m-0 mt-0.5 text-[12.5px] text-shodh-text-secondary max-w-[68ch]">
          The token is deleted from this computer and the bot stops answering, you and everyone you allowed. Keep the Telegram
          conversations in Shodh’s sidebar, or remove them too? Messages already in Telegram stay there.
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <button type="button" className={OUTLINE_BUTTON} disabled={busy} autoFocus onClick={() => run(false)}>
          Disconnect and keep conversations
        </button>
        <button type="button" className={DANGER_BUTTON} disabled={busy} onClick={() => run(true)}>
          Disconnect and remove them
        </button>
        <button type="button" className={QUIET_BUTTON} disabled={busy} onClick={onDone}>Cancel</button>
      </div>
    </div>
  );
}

function HeaderMenu({ onDisconnect, link }: { onDisconnect: () => void; link: string | null }) {
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
  const item = 'w-full flex items-center gap-2 px-3 py-1.5 text-[12.5px] text-left hover:bg-shodh-raised-2 focus-visible:bg-shodh-raised-2 focus-visible:outline-none';
  return (
    <div ref={menuRef} className="relative shrink-0">
      <button ref={buttonRef} type="button" className={cn(QUIET_BUTTON, 'w-9 h-9 justify-center px-0')} aria-label="Telegram options" aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen(o => !o)}>
        <MoreHorizontal className="w-4 h-4" aria-hidden="true" />
      </button>
      {open && (
        <div
          role="menu"
          aria-label="Telegram options"
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
          {link && (
            <button
              type="button"
              role="menuitem"
              autoFocus
              className={cn(item, 'text-shodh-text-secondary')}
              onClick={() => {
                setOpen(false);
                void openUrl(link).catch(err => notify.error('Telegram could not be opened', { description: slackError(err) }));
              }}
            >
              <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" /> Open the bot in Telegram
            </button>
          )}
          <button
            type="button"
            role="menuitem"
            autoFocus={!link}
            className={cn(item, 'text-shodh-error')}
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
    <nav aria-label="Breadcrumb" className="flex items-center gap-1 text-[12.5px]">
      <button type="button" className={cn(QUIET_BUTTON, '-ml-2.5')} onClick={onBack} aria-label="Back to all connectors">
        <ArrowLeft className="w-3.5 h-3.5" aria-hidden="true" />
        Connectors
      </button>
      <ChevronRight className="w-3.5 h-3.5 text-shodh-text-faint" aria-hidden="true" />
      <span aria-current="page" className="text-shodh-text font-medium">Telegram</span>
    </nav>
  );
}

/**
 * The Telegram page: a guided setup (make a bot, paste its token, link your account), then
 * its sections in tabs. Disconnect lives in the header's menu and on Settings.
 */
export function TelegramPage({
  telegram,
  wizard,
  onWizard,
  tab,
  onTab,
  onBack,
}: {
  telegram: TelegramStatusState;
  wizard: TelegramWizardState;
  onWizard: (next: TelegramWizardState) => void;
  tab: TelegramTabId;
  onTab: (tab: TelegramTabId) => void;
  onBack: () => void;
}) {
  const { status, loadError, setStatus, retry, now } = telegram;
  const [busy, setBusy] = useState(false);
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);
  /** The user chose to link their account later. */
  const [linkLater, setLinkLater] = useState(false);
  /** The account was linked while the setup showed: say so before the tabs. */
  const [justLinked, setJustLinked] = useState(false);
  const linking = useRef(false);
  const tabRefs = useRef<Partial<Record<TelegramTabId, HTMLButtonElement | null>>>({});
  const workspaces = useWorkspaceListings();

  const inSetup = status !== null && (!status.bot || (!status.owner && !linkLater));
  useEffect(() => {
    if (!status) return;
    if (inSetup && status.bot && !status.owner) linking.current = true;
    if (linking.current && status.owner) {
      linking.current = false;
      setJustLinked(true);
    }
  }, [status, inSetup]);

  const disconnect = async (forget: boolean) => {
    setBusy(true);
    try {
      setStatus(await telegramApi.disconnect(forget));
      setConfirmDisconnect(false);
      setJustLinked(false);
      setLinkLater(false);
      notify.success(forget ? 'Telegram disconnected; its conversations are removed' : 'Telegram disconnected');
    } catch (err) {
      notify.error('Telegram could not be disconnected', { description: slackError(err) });
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
            <p className="m-0">The Telegram connector could not be loaded: {loadError}</p>
            <button type="button" className={cn(OUTLINE_BUTTON, 'mt-2')} onClick={retry}>Try again</button>
          </Callout>
        ) : (
          <div className="flex flex-col gap-5" aria-busy="true" aria-label="Loading Telegram">
            <div className="flex items-center gap-3">
              <TelegramMark size="lg" />
              <div className="flex flex-col gap-2 flex-1">
                <Skeleton className="h-5 w-28" />
                <Skeleton className="h-3.5 w-52" />
              </div>
            </div>
            <Skeleton className="h-9 w-full max-w-xl" />
            <Skeleton className="h-48 w-full" />
          </div>
        )}
      </div>
    );
  }

  // A refresh failed: the status shown is the last known one, not current.
  const pill = loadError ? stalePill(telegramPill(status)) : telegramPill(status);
  const badges = telegramTabBadges(status);
  const tabProps = { status, onStatus: setStatus, busy, setBusy, now };

  return (
    <div className="flex flex-col gap-5">
      <Breadcrumb onBack={onBack} />
      {loadError && <StaleNotice error={loadError} onRetry={retry} />}

      <header className="flex items-start gap-3.5">
        <TelegramMark size="lg" />
        <div className="flex-1 min-w-0">
          <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
            <h2 className="m-0 text-[20px] font-semibold text-shodh-text">Telegram</h2>
            <span role="status" aria-live="polite">
              <StatusPill pill={pill} />
            </span>
          </div>
          <p className="m-0 mt-0.5 text-[13px] text-shodh-text-secondary truncate">
            {status.bot ? `@${status.bot.username} · ${ownerLine(status)}` : 'Ask Shodh from your phone, with your whole library and memories.'}
          </p>
        </div>
        {status.bot && <HeaderMenu link={status.bot.link} onDisconnect={() => setConfirmDisconnect(true)} />}
      </header>

      {confirmDisconnect && status.bot && (
        <DisconnectConfirm status={status} busy={busy} onDone={() => setConfirmDisconnect(false)} run={forget => void disconnect(forget)} />
      )}

      {inSetup ? (
        <TelegramSetup
          status={status}
          wizard={wizard}
          onWizard={onWizard}
          onStatus={setStatus}
          onLater={() => {
            setLinkLater(true);
            onTab('overview');
          }}
        />
      ) : justLinked ? (
        <section aria-labelledby="telegram-linked" className="flex flex-col items-start gap-3 rounded-xl border border-shodh-border bg-shodh-surface p-6">
          <CheckCircle2 className="w-7 h-7 text-shodh-success" aria-hidden="true" />
          <div>
            <h3 id="telegram-linked" className="m-0 text-[16px] font-semibold text-shodh-text">{ownerLine(status)}</h3>
            <p className="m-0 mt-1 text-[13px] text-shodh-text-secondary max-w-[60ch]">
              Ask anything in that chat: Shodh answers from all your workspaces and memories, with sources. Send /new for a fresh
              conversation. Your Telegram conversations appear in Shodh’s sidebar too.
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <button type="button" className={PRIMARY_BUTTON} autoFocus onClick={() => { setJustLinked(false); onTab('overview'); }}>
              Done
            </button>
            <button type="button" className={QUIET_BUTTON} onClick={() => { setJustLinked(false); onTab('people'); }}>
              Invite someone else
            </button>
          </div>
        </section>
      ) : (
        <>
          <div role="tablist" aria-label="Telegram sections" className={TABLIST}>
            {TELEGRAM_TABS.map(t => {
              const badge = badges[t];
              return (
                <button
                  key={t}
                  ref={el => { tabRefs.current[t] = el; }}
                  type="button"
                  role="tab"
                  id={`telegram-tab-${t}`}
                  aria-selected={tab === t}
                  aria-controls={`telegram-panel-${t}`}
                  tabIndex={tab === t ? 0 : -1}
                  onClick={() => onTab(t)}
                  onKeyDown={e => {
                    const next = moveTab(t, e.key, TELEGRAM_TABS);
                    if (!next) return;
                    e.preventDefault();
                    onTab(next);
                    tabRefs.current[next]?.focus();
                  }}
                  className={tabClass(tab === t)}
                >
                  {TELEGRAM_TAB_LABELS[t]}
                  {badge && (
                    <span className="min-w-[18px] h-[18px] px-1 inline-flex items-center justify-center rounded-full text-[10.5px] font-semibold tabular-nums bg-shodh-warning-soft text-shodh-warning">
                      {badge.count}
                      <span className="sr-only"> waiting</span>
                    </span>
                  )}
                </button>
              );
            })}
          </div>

          <div role="tabpanel" id={`telegram-panel-${tab}`} aria-labelledby={`telegram-tab-${tab}`} tabIndex={0} className="focus:outline-none">
            {tab === 'overview' && <OverviewTab {...tabProps} onTab={onTab} />}
            {tab === 'people' && <PeopleTab {...tabProps} workspaces={workspaces} />}
            {tab === 'settings' && <SettingsTab {...tabProps} onDisconnect={() => setConfirmDisconnect(true)} />}
          </div>
        </>
      )}
    </div>
  );
}

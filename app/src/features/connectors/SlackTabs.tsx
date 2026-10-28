import React, { useEffect, useId, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Check, Copy, ExternalLink, Hash, Loader2, Lock, RefreshCw, Users } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { SwitchRow } from '../../components/PrivacySettings';
import { listen } from '@tauri-apps/api/event';
import { workspacesApi, WORKSPACES_CHANGED } from '../workspaces/api';
import type { WorkspaceListing } from '../workspaces/types';
import { slackApi, slackError } from './api';
import {
  INDEX_PRIVACY_NOTE,
  MODEL_PRIVACY_NOTE,
  RUNNING_HINT,
  UPDATE_APP_FOR_INDEXING_LINE,
  UPDATE_APP_STEPS,
  answerPlacesLine,
  appManifestUrl,
  answerSourceLine,
  capShare,
  channelLabel,
  channelNeedsAttention,
  exposureLine,
  indexChannelLine,
  indexSummary,
  lastEventLine,
  memoriesCountLine,
  memoryPlace,
  memoryProvenance,
  manifestFromCreateUrl,
  nextSteps,
  overviewWarnings,
  ownerLine,
  scopeWords,
  sharedIndexLine,
  stateLine,
  stillMissingLine,
  usageLine,
  validCap,
  validWindow,
  type ChannelChoice,
  type IndexChannelView,
  type Person,
  type SlackMemoryView,
  type SlackStatus,
  type SlackTabId,
} from './slackModel';
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
  TONE_DOT,
  TONE_TEXT,
} from './ui';

/** What every tab of the connected page gets. */
export interface TabProps {
  status: SlackStatus;
  onStatus: (status: SlackStatus) => void;
  busy: boolean;
  setBusy: (busy: boolean) => void;
  now: number;
}

/** Runs a command that returns the new status; a failure is a toast. */
function useRun({ onStatus, setBusy }: Pick<TabProps, 'onStatus' | 'setBusy'>) {
  return async (action: () => Promise<SlackStatus>, failure: string): Promise<boolean> => {
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

function openInSlack(url: string) {
  void openUrl(url).catch(err => notify.error('Slack could not be opened', { description: slackError(err) }));
}

function Meter({ share, label }: { share: number; label: string }) {
  const percent = Math.round(share * 100);
  return (
    <div
      role="meter"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent}
      className="h-1.5 w-full rounded-full bg-shodh-raised-2 overflow-hidden"
    >
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

/**
 * How to give the user's Slack app the permissions it lacks: open its App Manifest page,
 * paste Shodh's current manifest, reinstall, then check again (Shodh asks Slack which
 * permissions the bot has now). The permission names are behind a disclosure.
 */
function UpdateSlackApp({ status, permissions, onStatus, busy, setBusy }: TabProps & { permissions: string[] }) {
  const [copied, setCopied] = useState(false);
  const [checking, setChecking] = useState(false);
  const manifest = manifestFromCreateUrl(status.createAppUrl);
  const stepsId = useId();

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 2000);
    return () => window.clearTimeout(timer);
  }, [copied]);

  const copy = async () => {
    if (!manifest) return;
    try {
      await navigator.clipboard.writeText(manifest);
      setCopied(true);
    } catch (err) {
      notify.error('The manifest could not be copied', { description: slackError(err) });
    }
  };

  // Shodh asks Slack which permissions the bot has now (the stored ones are from when it
  // connected); the step goes away once none is missing.
  const checkAgain = async () => {
    setChecking(true);
    setBusy(true);
    try {
      const next = await slackApi.checkPermissions();
      onStatus(next);
      const still = stillMissingLine(next);
      if (still) notify.error('The Slack app still lacks permissions', { description: still });
      else notify.success('The Slack app has every permission Shodh needs');
    } catch (err) {
      notify.error('Slack could not be asked', { description: slackError(err) });
    } finally {
      setChecking(false);
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <button type="button" className={PRIMARY_BUTTON} disabled={busy} onClick={() => openInSlack(appManifestUrl(status.installation?.appId))}>
          <ExternalLink className="w-4 h-4" aria-hidden="true" /> Open your Slack app
        </button>
        <button type="button" className={OUTLINE_BUTTON} disabled={busy || !manifest} onClick={() => void copy()}>
          {copied ? <Check className="w-3.5 h-3.5 text-shodh-success" aria-hidden="true" /> : <Copy className="w-3.5 h-3.5" aria-hidden="true" />}
          {copied ? 'Manifest copied' : 'Copy updated manifest'}
        </button>
        <span aria-live="polite" className="sr-only">{copied ? 'The updated manifest was copied.' : ''}</span>
      </div>
      <ol id={stepsId} className="m-0 pl-5 list-decimal flex flex-col gap-1 text-[12.5px] text-shodh-text-secondary max-w-[64ch]">
        {UPDATE_APP_STEPS.map(step => (
          <li key={step}>{step}</li>
        ))}
      </ol>
      <div>
        <button type="button" className={OUTLINE_BUTTON} disabled={busy} aria-describedby={stepsId} onClick={() => void checkAgain()}>
          {checking ? (
            <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />
          ) : (
            <RefreshCw className="w-3.5 h-3.5" aria-hidden="true" />
          )}
          {checking ? 'Asking Slack…' : 'Check again'}
        </button>
      </div>
      <LearnMore label="Show permission names">
        <ul className="m-0 p-0 list-none flex flex-col gap-1">
          {permissions.map(scope => (
            <li key={scope}>
              <code className="font-mono text-[12px] text-shodh-text">{scope}</code>
              {scopeWords(scope) !== scope && <span className="text-shodh-text-muted">{`: ${scopeWords(scope)}`}</span>}
            </li>
          ))}
        </ul>
      </LearnMore>
    </div>
  );
}

export function OverviewTab(props: TabProps & { onTab: (tab: SlackTabId) => void }) {
  const { status, now, onTab } = props;
  const line = stateLine(status, now);
  const warnings = overviewWarnings(status, now);
  const steps = nextSteps(status);
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
          <p className="m-0 text-[12px] text-shodh-text-muted">{lastEventLine(status, now)}</p>
          {on && <p className="m-0 text-[12px] text-shodh-text-muted">{RUNNING_HINT}</p>}
        </Panel>
        <Panel title="Answers today">
          <p className="m-0 text-[13.5px] font-semibold text-shodh-text tabular-nums">
            {status.answersToday} of {status.dailyCap}
          </p>
          <Meter share={capShare(status)} label="Slack answers used today" />
          <p className="m-0 text-[12px] text-shodh-text-muted">{usageLine(status, now)}</p>
        </Panel>
        <Panel title="Indexing">
          <p className="m-0 text-[13px] text-shodh-text">{indexSummary(status.index)}</p>
          <button type="button" className={cn(OUTLINE_BUTTON, 'self-start mt-1.5')} onClick={() => onTab('indexing')}>
            Open Indexing
          </button>
        </Panel>
        <Panel title="Memories from Slack">
          <p className="m-0 text-[13px] text-shodh-text">{memoriesCountLine(status.index)}</p>
          <button type="button" className={cn(OUTLINE_BUTTON, 'self-start mt-1.5')} onClick={() => onTab('memories')}>
            Open Memories
          </button>
        </Panel>
      </div>

      {warnings.length > 0 && (
        <div role="alert" className="flex flex-col gap-2">
          {warnings.map(w => (
            <Callout key={w} tone="warning">{w}</Callout>
          ))}
        </div>
      )}

      {steps.length > 0 ? (
        <Section title="Next steps">
          <ul className="m-0 p-0 list-none flex flex-col divide-y divide-shodh-border-subtle rounded-lg border border-shodh-border-subtle bg-shodh-surface">
            {steps.map(step => (
              <li key={step.id} className="flex flex-wrap sm:flex-nowrap items-start gap-3 px-4 py-3">
                <span aria-hidden="true" className={cn('mt-1.5 w-2 h-2 rounded-full shrink-0', TONE_DOT[step.tone])} />
                <div className="flex-1 min-w-0">
                  <p className="m-0 text-[13px] font-semibold text-shodh-text">{step.title}</p>
                  <p className="m-0 mt-0.5 text-[12.5px] text-shodh-text-muted break-words max-w-[72ch]">{step.detail}</p>
                  {step.permissions && step.permissions.length > 0 && (
                    <div className="mt-3">
                      <UpdateSlackApp {...props} permissions={step.permissions} />
                    </div>
                  )}
                </div>
                {step.tab && step.action && (
                  <button type="button" className={cn(OUTLINE_BUTTON, 'shrink-0')} onClick={() => onTab(step.tab as SlackTabId)}>
                    {step.action}
                  </button>
                )}
              </li>
            ))}
          </ul>
        </Section>
      ) : (
        <Callout tone="ok" role="status">Everything is set up. Shodh answers in the channels you chose and in direct messages.</Callout>
      )}
    </div>
  );
}

// ── Answering ───────────────────────────────────────────────────────

/** The channels Shodh answers in: loaded from Slack on request, saved together. */
function ChannelPicker({ status, onStatus, busy, setBusy }: TabProps) {
  const [choices, setChoices] = useState<ChannelChoice[] | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const places = answerPlacesLine(status);

  const load = async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await slackApi.listChannels();
      setChoices(list);
      // A channel that became shared since it was chosen must be chosen again.
      const nowShared = new Set(status.answerChannels.filter(c => c.nowShared).map(c => c.id));
      setPicked(new Set(list.filter(c => c.selected && !nowShared.has(c.id)).map(c => c.id)));
    } catch (err) {
      setError(slackError(err));
    } finally {
      setLoading(false);
    }
  };

  const save = async () => {
    setBusy(true);
    try {
      onStatus(await slackApi.setAnswerChannels([...picked]));
      setChoices(null);
    } catch (err) {
      notify.error('The channels were not saved', { description: slackError(err) });
    } finally {
      setBusy(false);
    }
  };

  const toggle = (id: string) =>
    setPicked(prev => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <Section
      title="Channels Shodh answers in"
      description={<LearnMore summary={status.answerChannels.length === 0 ? 'None yet: a mention anywhere gets a polite no.' : `${status.answerChannels.length} ${status.answerChannels.length === 1 ? 'channel' : 'channels'}, plus direct messages from your members.`}>{places.text}</LearnMore>}
      actions={
        choices === null ? (
          <button type="button" className={OUTLINE_BUTTON} disabled={busy || loading || status.state === 'local_only'} onClick={() => void load()}>
            {loading && <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
            {loading ? 'Asking Slack…' : 'Choose channels'}
          </button>
        ) : undefined
      }
    >
      {choices === null && status.answerChannels.length > 0 && (
        <ul className="m-0 p-0 list-none flex flex-wrap gap-1.5">
          {status.answerChannels.map(c => (
            <li
              key={c.id}
              className={cn(
                'inline-flex items-center gap-1 h-7 px-2.5 rounded-full border text-[12px]',
                c.nowShared || c.shared ? 'border-shodh-warning/50 text-shodh-warning' : 'border-shodh-border text-shodh-text-secondary',
              )}
            >
              <Hash className="w-3 h-3" aria-hidden="true" />
              {c.name}
              {c.shared && <span className="sr-only">, shared with other organizations</span>}
              {c.nowShared && <span> · shared now, choose again</span>}
            </li>
          ))}
        </ul>
      )}
      {choices !== null && (choices.length === 0 ? (
        <Callout tone="neutral">
          The bot is in no channel yet. In Slack, type /invite @{status.installation?.botName ?? 'shodh'} in a channel, then choose again.
          <div className="mt-2">
            <button type="button" className={OUTLINE_BUTTON} onClick={() => setChoices(null)}>Close</button>
          </div>
        </Callout>
      ) : (
        <div className="flex flex-col gap-3 rounded-lg border border-shodh-border-subtle bg-shodh-surface p-3">
          <ul aria-label="Channels the bot is in" className="m-0 p-0 list-none grid grid-cols-1 sm:grid-cols-2 gap-1 max-h-60 overflow-y-auto scrollbar-thin">
            {choices.map(c => (
              <li key={c.id}>
                <label className="flex items-center gap-2 px-2 py-1.5 rounded-md text-[12.5px] text-shodh-text-secondary hover:bg-shodh-raised cursor-pointer">
                  <input type="checkbox" checked={picked.has(c.id)} disabled={busy} onChange={() => toggle(c.id)} />
                  <span className={c.shared ? 'text-shodh-warning' : undefined}>{channelLabel(c)}</span>
                </label>
              </li>
            ))}
          </ul>
          <div className="flex items-center gap-2">
            <button type="button" className={PRIMARY_BUTTON} disabled={busy} onClick={() => void save()}>Save channels</button>
            <button type="button" className={QUIET_BUTTON} disabled={busy} onClick={() => setChoices(null)}>Cancel</button>
          </div>
        </div>
      ))}
      {error && <p role="alert" className="m-0 text-[12.5px] text-shodh-error">{error}</p>}
    </Section>
  );
}

export function AnsweringTab(props: TabProps & { workspaces: WorkspaceListing[] | null }) {
  const { status, busy, setBusy, now, workspaces } = props;
  const run = useRun(props);
  const [capInput, setCapInput] = useState(String(status.dailyCap));
  /** A workspace picked but not confirmed yet, with what choosing it exposes. */
  const [pending, setPending] = useState<{ id: string; line: string } | null>(null);
  const pickerId = useId();
  const capId = useId();
  const capHelpId = useId();

  useEffect(() => setCapInput(String(status.dailyCap)), [status.dailyCap]);

  const pick = async (id: string) => {
    if (!id) {
      setPending(null);
      void run(() => slackApi.setAnswerWorkspace(null), 'The workspace could not be chosen');
      return;
    }
    setBusy(true);
    try {
      const detail = await workspacesApi.get(id);
      setPending({ id, line: exposureLine(status.installation?.teamName ?? 'Slack', detail.name, detail.sources) });
    } catch (err) {
      notify.error('The workspace could not be read', { description: slackError(err) });
    } finally {
      setBusy(false);
    }
  };

  const on = status.state !== 'off' && status.state !== 'local_only';
  const source = answerSourceLine(status);
  const cap = validCap(capInput, status);
  const chosen = status.answerWorkspace?.id ?? '';
  const chosenListed = workspaces?.some(w => w.id === chosen) ?? true;

  return (
    <div className="flex flex-col gap-8">
      <SwitchRow
        label="Answer in Slack"
        description="Shodh keeps a connection to Slack and answers mentions in the channels below, and direct messages from your members."
        checked={on}
        disabled={busy || status.state === 'local_only'}
        onChange={next => void run(() => slackApi.setEnabled(next), 'The Slack connector could not be switched')}
      />

      <Section title="Slack answers from" description="One Shodh workspace. Nothing else in Shodh is used.">
        <label htmlFor={pickerId} className="sr-only">Workspace Slack answers from</label>
        {workspaces === null ? (
          <Skeleton className="h-9 max-w-sm" />
        ) : (
          <select
            id={pickerId}
            value={pending?.id ?? chosen}
            disabled={busy}
            onChange={e => void pick(e.target.value)}
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
          <div role="alertdialog" aria-label="Confirm the workspace Slack answers from" className="flex flex-col gap-2 rounded-lg border border-shodh-warning/40 bg-shodh-warning-soft px-3.5 py-3">
            <p className="m-0 text-[12.5px] text-shodh-text-secondary">{pending.line}</p>
            <div className="flex items-center gap-2">
              <button
                type="button"
                className={PRIMARY_BUTTON}
                disabled={busy}
                autoFocus
                onClick={() => {
                  const id = pending.id;
                  setPending(null);
                  void run(() => slackApi.setAnswerWorkspace(id), 'The workspace could not be chosen');
                }}
              >
                Answer from this workspace
              </button>
              <button
                type="button"
                className={QUIET_BUTTON}
                disabled={busy}
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

      <ChannelPicker {...props} />

      <Section
        title="Answers per day"
        description={<span id={capHelpId}>Each answer is one model call. At the limit the bot says so and makes no call.</span>}
      >
        <div className="flex flex-col gap-2 max-w-sm">
          <Meter share={capShare(status)} label="Slack answers used today" />
          <p className="m-0 text-[12px] text-shodh-text-muted">{usageLine(status, now)}</p>
        </div>
        <form
          className="flex items-center gap-2"
          onSubmit={e => {
            e.preventDefault();
            if (cap !== null && cap !== status.dailyCap) void run(() => slackApi.setDailyCap(cap), 'The daily limit was not saved');
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
            aria-describedby={capHelpId}
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
    </div>
  );
}

// ── Indexing ────────────────────────────────────────────────────────

function ChannelBadges({ channel }: { channel: { private?: boolean; shared: boolean } }) {
  return (
    <>
      {channel.private && (
        <span className="inline-flex items-center gap-1 h-5 px-1.5 rounded-full bg-shodh-raised text-[11px] text-shodh-text-secondary">
          <Lock className="w-3 h-3" aria-hidden="true" /> Private
        </span>
      )}
      {channel.shared && (
        <span className="inline-flex items-center gap-1 h-5 px-1.5 rounded-full bg-shodh-warning-soft text-[11px] text-shodh-warning">
          <Users className="w-3 h-3" aria-hidden="true" /> Shared
        </span>
      )}
    </>
  );
}

function ChannelRow({
  channel,
  now,
  busy,
  confirming,
  onConfirm,
  onForget,
}: {
  channel: IndexChannelView;
  now: number;
  busy: boolean;
  confirming: boolean;
  onConfirm: (on: boolean) => void;
  onForget: () => void;
}) {
  const line = indexChannelLine(channel, now);
  const progress =
    channel.state === 'backfilling' && channel.estimatedThreads !== null && channel.estimatedThreads > 0
      ? Math.min(1, channel.threads / channel.estimatedThreads)
      : null;
  return (
    <li className={cn('px-4 py-3', channelNeedsAttention(channel) && 'bg-shodh-warning-soft/40')}>
      <div className="grid grid-cols-1 md:grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)_minmax(0,1.6fr)_4.5rem_7.5rem] gap-x-4 gap-y-1.5 items-start">
        <div className="min-w-0 flex flex-wrap items-center gap-1.5">
          <span className="text-[13px] font-semibold text-shodh-text truncate">#{channel.name}</span>
          <ChannelBadges channel={channel} />
        </div>
        <p className="m-0 text-[12.5px] text-shodh-text-secondary truncate">
          <span className="md:sr-only">Into </span>
          {channel.workspace.name || 'a deleted workspace'}
        </p>
        <div className="min-w-0 flex flex-col gap-1">
          <p className="m-0 flex items-center gap-1.5 text-[12.5px] text-shodh-text">
            <span aria-hidden="true" className={cn('w-1.5 h-1.5 rounded-full shrink-0', TONE_DOT[line.tone])} />
            {line.title}
          </p>
          {progress !== null && (
            <div className="h-1 rounded-full bg-shodh-raised-2 overflow-hidden" aria-hidden="true">
              <div className="h-full bg-shodh-accent" style={{ width: `${Math.round(progress * 100)}%` }} />
            </div>
          )}
          {line.detail && <p className={cn('m-0 text-[12px] break-words', TONE_TEXT[line.tone])}>{line.detail}</p>}
        </div>
        <p className="m-0 text-[12.5px] text-shodh-text-secondary tabular-nums md:text-right">
          {channel.threads}
          <span className="md:sr-only"> {channel.threads === 1 ? 'thread' : 'threads'}</span>
        </p>
        <div className="md:justify-self-end">
          {!confirming && (
            <button
              type="button"
              className={QUIET_BUTTON}
              disabled={busy || channel.state === 'forgetting'}
              onClick={() => onConfirm(true)}
              aria-label={`Stop indexing #${channel.name} and forget it`}
            >
              Stop and forget
            </button>
          )}
        </div>
      </div>
      {confirming && (
        <div
          role="alertdialog"
          aria-label={`Forget #${channel.name}`}
          onKeyDown={e => { if (e.key === 'Escape') { e.preventDefault(); onConfirm(false); } }}
          className="mt-3 flex flex-col gap-2 rounded-lg border border-shodh-error/40 bg-shodh-surface-2 px-3.5 py-3"
        >
          <p className="m-0 text-[12.5px] text-shodh-text-secondary">
            Stop indexing #{channel.name} and forget its {channel.threads} {channel.threads === 1 ? 'thread' : 'threads'}, its sync
            state and the memories saved there?
          </p>
          <div className="flex items-center gap-2">
            <button type="button" className={DANGER_BUTTON} disabled={busy} autoFocus onClick={onForget}>Forget</button>
            <button type="button" className={QUIET_BUTTON} disabled={busy} onClick={() => onConfirm(false)}>Cancel</button>
          </div>
        </div>
      )}
    </li>
  );
}

export function IndexingTab(props: TabProps & { workspaces: WorkspaceListing[] | null }) {
  const { status, busy, now, workspaces } = props;
  const run = useRun(props);
  const index = status.index;
  const channelPickerId = useId();
  const workspacePickerId = useId();
  const windowId = useId();
  const [choices, setChoices] = useState<ChannelChoice[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [channelId, setChannelId] = useState('');
  const [workspaceId, setWorkspaceId] = useState('');
  const [sharedConfirmed, setSharedConfirmed] = useState(false);
  const [forgetting, setForgetting] = useState<string | null>(null);
  const [windowInput, setWindowInput] = useState(String(index.windowDays));

  useEffect(() => setWindowInput(String(index.windowDays)), [index.windowDays]);

  const indexed = new Set(index.channels.map(c => c.id));
  const offered = (choices ?? []).filter(c => !indexed.has(c.id));
  const chosen = offered.find(c => c.id === channelId) ?? null;
  const windowDays = validWindow(windowInput, index);
  const blocked = !index.available || index.missingScopes.length > 0 || status.state === 'local_only';

  const load = async () => {
    setLoading(true);
    setError(null);
    try {
      setChoices(await slackApi.listChannels());
      setChannelId('');
      setSharedConfirmed(false);
    } catch (err) {
      setError(slackError(err));
    } finally {
      setLoading(false);
    }
  };

  const add = async () => {
    if (!chosen || !workspaceId) return;
    const ok = await run(
      () => slackApi.indexChannel(chosen.id, workspaceId, chosen.shared && sharedConfirmed),
      'The channel could not be indexed',
    );
    if (ok) setChoices(null);
  };

  return (
    <div className="flex flex-col gap-8">
      <Section
        title="Indexed channels"
        description={
          <LearnMore summary={`Threads of these channels can be searched and cited in Ask, like documents. ${index.costNote}`}>
            <p className="m-0">This list is separate from where Shodh answers.</p>
            <p className="m-0 mt-1.5">{INDEX_PRIVACY_NOTE}</p>
          </LearnMore>
        }
        actions={
          choices === null ? (
            <button type="button" className={PRIMARY_BUTTON} disabled={busy || loading || blocked} onClick={() => void load()}>
              {loading && <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
              {loading ? 'Asking Slack…' : 'Index a channel'}
            </button>
          ) : undefined
        }
      >
        {!index.available && (
          <Callout tone="warning" role="alert">Indexing needs the app database, which could not be opened. The audit page says why.</Callout>
        )}
        {index.missingScopes.length > 0 && (
          <Callout tone="warning" role="alert">
            <p className="m-0 mb-3 max-w-[72ch]">{UPDATE_APP_FOR_INDEXING_LINE}</p>
            <UpdateSlackApp {...props} permissions={index.missingScopes} />
          </Callout>
        )}

        {choices !== null && (offered.length === 0 ? (
          <Callout tone="neutral">
            No other channel to index. In Slack, type /invite @{status.installation?.botName ?? 'shodh'} in a channel, then try again.
            <div className="mt-2">
              <button type="button" className={OUTLINE_BUTTON} onClick={() => setChoices(null)}>Close</button>
            </div>
          </Callout>
        ) : (
          <div className="flex flex-col gap-3 rounded-lg border border-shodh-border-subtle bg-shodh-surface p-4">
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
              <div className="flex flex-col gap-1">
                <label htmlFor={channelPickerId} className="text-[12.5px] font-semibold text-shodh-text">Channel</label>
                <select
                  id={channelPickerId}
                  value={channelId}
                  disabled={busy}
                  onChange={e => { setChannelId(e.target.value); setSharedConfirmed(false); }}
                  className={INPUT}
                >
                  <option value="">Choose a channel…</option>
                  {offered.map(c => (
                    <option key={c.id} value={c.id}>{channelLabel(c)}</option>
                  ))}
                </select>
              </div>
              <div className="flex flex-col gap-1">
                <label htmlFor={workspacePickerId} className="text-[12.5px] font-semibold text-shodh-text">Into workspace</label>
                <select
                  id={workspacePickerId}
                  value={workspaceId}
                  disabled={busy || workspaces === null}
                  onChange={e => setWorkspaceId(e.target.value)}
                  className={INPUT}
                >
                  <option value="">Choose a workspace…</option>
                  {(workspaces ?? []).map(w => (
                    <option key={w.id} value={w.id}>{w.name}</option>
                  ))}
                </select>
              </div>
            </div>
            {chosen?.shared && (
              <label className="flex items-start gap-2 text-[12.5px] text-shodh-warning">
                <input type="checkbox" className="mt-0.5" checked={sharedConfirmed} disabled={busy} onChange={e => setSharedConfirmed(e.target.checked)} />
                <span>{sharedIndexLine(chosen)}</span>
              </label>
            )}
            {error && <p role="alert" className="m-0 text-[12.5px] text-shodh-error">{error}</p>}
            <div className="flex items-center gap-2">
              <button
                type="button"
                className={PRIMARY_BUTTON}
                disabled={busy || !chosen || !workspaceId || (chosen.shared && !sharedConfirmed)}
                onClick={() => void add()}
              >
                Index this channel
              </button>
              <button type="button" className={QUIET_BUTTON} disabled={busy} onClick={() => setChoices(null)}>Cancel</button>
            </div>
          </div>
        ))}
        {choices === null && error && <p role="alert" className="m-0 text-[12.5px] text-shodh-error">{error}</p>}

        {index.channels.length === 0 ? (
          <div className="flex flex-col items-start gap-1 rounded-lg border border-dashed border-shodh-border px-4 py-6">
            <p className="m-0 text-[13px] font-medium text-shodh-text">No channel indexed yet</p>
            <p className="m-0 text-[12.5px] text-shodh-text-muted max-w-[60ch]">
              Index a channel your team discusses work in, and its threads show up in Ask with links back to Slack.
            </p>
          </div>
        ) : (
          <div className="rounded-lg border border-shodh-border-subtle bg-shodh-surface overflow-hidden">
            <div aria-hidden="true" className="hidden md:grid grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)_minmax(0,1.6fr)_4.5rem_7.5rem] gap-x-4 px-4 py-2 border-b border-shodh-border-subtle text-[11.5px] font-medium text-shodh-text-muted">
              <span>Channel</span>
              <span>Workspace</span>
              <span>Status</span>
              <span className="text-right">Threads</span>
              <span />
            </div>
            <ul aria-label="Indexed channels" className="m-0 p-0 list-none divide-y divide-shodh-border-subtle">
              {index.channels.map(c => (
                <ChannelRow
                  key={c.id}
                  channel={c}
                  now={now}
                  busy={busy}
                  confirming={forgetting === c.id}
                  onConfirm={on => setForgetting(on ? c.id : null)}
                  onForget={() => {
                    setForgetting(null);
                    void run(() => slackApi.forgetChannel(c.id), 'The channel could not be forgotten');
                  }}
                />
              ))}
            </ul>
          </div>
        )}
      </Section>

      <Section title="History" description="How far back a newly indexed channel is read.">
        <form
          className="flex items-center gap-2"
          onSubmit={e => {
            e.preventDefault();
            if (windowDays !== null && windowDays !== index.windowDays) {
              void run(() => slackApi.setIndexWindow(windowDays), 'The history window was not saved');
            }
          }}
        >
          <label htmlFor={windowId} className="text-[12.5px] text-shodh-text-secondary">Read back</label>
          <input
            id={windowId}
            type="number"
            inputMode="numeric"
            min={index.minWindowDays}
            max={index.maxWindowDays}
            step={1}
            value={windowInput}
            disabled={busy}
            aria-invalid={windowDays === null}
            onChange={e => setWindowInput(e.target.value)}
            className={cn(INPUT, 'w-24 tabular-nums')}
          />
          <span className="text-[12.5px] text-shodh-text-secondary">days</span>
          <button type="submit" className={OUTLINE_BUTTON} disabled={busy || windowDays === null || windowDays === index.windowDays}>Save</button>
        </form>
        {windowDays === null && windowInput.trim() !== '' && (
          <p role="alert" className="m-0 text-[12.5px] text-shodh-error">
            Enter a whole number of days from {index.minWindowDays} to {index.maxWindowDays}.
          </p>
        )}
      </Section>
    </div>
  );
}

// ── Memories ────────────────────────────────────────────────────────

/** "You in Slack": the Slack member whose `remember:` goes into the user's memories. */
function OwnerSection({ status, onStatus, busy, setBusy }: TabProps) {
  const [people, setPeople] = useState<Person[] | null>(null);
  const [picked, setPicked] = useState('');
  const [filter, setFilter] = useState('');
  const [error, setError] = useState<string | null>(null);
  const filterId = useId();

  const load = async () => {
    setError(null);
    setBusy(true);
    try {
      const list = await slackApi.listPeople();
      setPeople(list);
      setPicked(status.index.owner?.userId ?? '');
    } catch (err) {
      setError(slackError(err));
    } finally {
      setBusy(false);
    }
  };

  const save = async (userId: string | null) => {
    setBusy(true);
    try {
      onStatus(await slackApi.setOwner(userId));
      setPeople(null);
    } catch (err) {
      notify.error('The choice was not saved', { description: slackError(err) });
    } finally {
      setBusy(false);
    }
  };

  const shown = (people ?? []).filter(p => p.name.toLowerCase().includes(filter.trim().toLowerCase()));

  return (
    <Section
      title="You in Slack"
      description={ownerLine(status.index)}
      actions={
        people === null ? (
          <>
            {status.index.owner && (
              <button type="button" className={QUIET_BUTTON} disabled={busy} onClick={() => void save(null)}>Clear</button>
            )}
            <button type="button" className={OUTLINE_BUTTON} disabled={busy || status.state === 'local_only'} onClick={() => void load()}>
              {status.index.owner ? 'Change' : 'This is me…'}
            </button>
          </>
        ) : undefined
      }
    >
      {people !== null && (
        <div className="flex flex-col gap-2 rounded-lg border border-shodh-border-subtle bg-shodh-surface p-4">
          <label htmlFor={filterId} className="text-[12.5px] font-semibold text-shodh-text">Find yourself</label>
          <input id={filterId} value={filter} autoFocus onChange={e => setFilter(e.target.value)} className={cn(INPUT, 'max-w-xs')} />
          <ul aria-label="Members of the Slack workspace" className="m-0 p-0 list-none grid grid-cols-1 sm:grid-cols-2 gap-1 max-h-56 overflow-y-auto scrollbar-thin">
            {shown.map(p => (
              <li key={p.id}>
                <label className="flex items-center gap-2 px-2 py-1.5 rounded-md text-[12.5px] text-shodh-text-secondary hover:bg-shodh-raised cursor-pointer">
                  <input type="radio" name="slack-owner" checked={picked === p.id} disabled={busy} onChange={() => setPicked(p.id)} />
                  {p.name}
                </label>
              </li>
            ))}
          </ul>
          {shown.length === 0 && <p className="m-0 text-[12.5px] text-shodh-text-muted">No member matches “{filter.trim()}”.</p>}
          <div className="flex items-center gap-2">
            <button type="button" className={PRIMARY_BUTTON} disabled={busy || !picked} onClick={() => void save(picked)}>This is me</button>
            <button type="button" className={QUIET_BUTTON} disabled={busy} onClick={() => setPeople(null)}>Cancel</button>
          </div>
        </div>
      )}
      {error && <p role="alert" className="m-0 text-[12.5px] text-shodh-error">{error}</p>}
    </Section>
  );
}

/** "From Slack": every memory saved from Slack, to keep, delete or open in Slack. */
function MemoriesList({ status, busy, setBusy }: TabProps) {
  const [memories, setMemories] = useState<SlackMemoryView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const count = status.index.memories;
  const available = status.index.available;

  // Read again whenever the count changes (someone saved or a channel was forgotten).
  useEffect(() => {
    if (!available) return;
    let cancelled = false;
    slackApi
      .memories()
      .then(list => { if (!cancelled) { setMemories(list); setError(null); } })
      .catch(err => { if (!cancelled) setError(slackError(err)); });
    return () => {
      cancelled = true;
    };
  }, [available, count]);

  const act = async (action: () => Promise<SlackMemoryView[]>, failure: string) => {
    setBusy(true);
    try {
      setMemories(await action());
    } catch (err) {
      notify.error(failure, { description: slackError(err) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Section
      title="From Slack"
      description="“@shodh remember: …” saves at once: yours into your memories, anyone else’s for their channel only, used only for Slack answers there."
    >
      {!available ? (
        <Callout tone="warning" role="alert">Memories from Slack need the app database, which could not be opened. The audit page says why.</Callout>
      ) : error ? (
        <Callout tone="error" role="alert">The memories could not be read: {error}</Callout>
      ) : memories === null ? (
        <div className="flex flex-col gap-2" aria-busy="true" aria-label="Loading memories">
          {[0, 1].map(i => (
            <div key={i} className="flex flex-col gap-2 rounded-lg border border-shodh-border-subtle bg-shodh-surface px-4 py-3">
              <Skeleton className="h-3.5 w-3/4" />
              <Skeleton className="h-3 w-1/3" />
            </div>
          ))}
        </div>
      ) : memories.length === 0 ? (
        <div className="flex flex-col items-start gap-1 rounded-lg border border-dashed border-shodh-border px-4 py-6">
          <p className="m-0 text-[13px] font-medium text-shodh-text">Nothing saved from Slack yet</p>
          <p className="m-0 text-[12.5px] text-shodh-text-muted max-w-[60ch]">
            In a channel Shodh answers in, anyone can write “@shodh remember: the launch moved to May”. It shows up here,
            with who saved it and where.
          </p>
        </div>
      ) : (
        <ul aria-label="Memories from Slack" className="m-0 p-0 list-none flex flex-col divide-y divide-shodh-border-subtle rounded-lg border border-shodh-border-subtle bg-shodh-surface">
          {memories.map(m => (
            <li key={m.id} className="flex flex-col gap-1.5 px-4 py-3">
              <p className="m-0 text-[13px] text-shodh-text whitespace-pre-wrap break-words">{m.text}</p>
              <p className="m-0 text-[12px] text-shodh-text-muted">
                {memoryProvenance(m)} · {memoryPlace(m)}
              </p>
              <div className="flex flex-wrap items-center gap-1 -ml-2.5">
                {!m.inMemories && (
                  <button type="button" className={QUIET_BUTTON} disabled={busy} onClick={() => void act(() => slackApi.promoteMemory(m.id), 'The memory could not be added')}>
                    Add to my memories
                  </button>
                )}
                {m.permalink && (
                  <button type="button" className={QUIET_BUTTON} onClick={() => openInSlack(m.permalink as string)}>
                    <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" /> Open in Slack
                  </button>
                )}
                <button
                  type="button"
                  className={cn(QUIET_BUTTON, 'text-shodh-error hover:text-shodh-error')}
                  disabled={busy}
                  onClick={() => void act(() => slackApi.deleteMemory(m.id), 'The memory could not be deleted')}
                >
                  Delete
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
    </Section>
  );
}

export function MemoriesTab(props: TabProps) {
  return (
    <div className="flex flex-col gap-8">
      <OwnerSection {...props} />
      <MemoriesList {...props} />
    </div>
  );
}

// ── Settings ────────────────────────────────────────────────────────

export function SettingsTab({ status, busy, onDisconnect }: TabProps & { onDisconnect: () => void }) {
  const source = answerSourceLine(status);
  const places = answerPlacesLine(status);
  return (
    <div className="flex flex-col gap-8">
      <Section title="What Slack can see">
        <ul className="m-0 pl-4 list-disc flex flex-col gap-1.5 text-[12.5px] text-shodh-text-secondary max-w-[68ch]">
          <li className={cn(source.tone === 'warning' && 'text-shodh-warning')}>{source.text}</li>
          <li className={cn(places.tone === 'warning' && 'text-shodh-warning')}>{places.text}</li>
        </ul>
      </Section>

      <Section title="Privacy">
        <div className="flex flex-col gap-3">
          <LearnMore summary="Questions from Slack go to your model provider.">{MODEL_PRIVACY_NOTE}</LearnMore>
          <LearnMore summary="Indexed threads are stored on this computer; use your disk’s encryption.">{INDEX_PRIVACY_NOTE}</LearnMore>
          <p className="m-0 text-[12.5px] text-shodh-text-muted">{RUNNING_HINT}</p>
        </div>
      </Section>

      <Section
        title="Disconnect"
        description="Deletes the tokens from this computer and stops the bot. You choose whether to keep what was indexed."
      >
        <div>
          <button type="button" className={DANGER_BUTTON} disabled={busy} onClick={onDisconnect}>
            Disconnect {status.installation?.teamName ?? 'Slack'}…
          </button>
        </div>
      </Section>
    </div>
  );
}

/** The workspaces the pickers offer, kept current. */
export function useWorkspaceListings(): WorkspaceListing[] | null {
  const [workspaces, setWorkspaces] = useState<WorkspaceListing[] | null>(null);
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    const load = () => {
      workspacesApi
        .list(false)
        .then(list => { if (!cancelled) setWorkspaces(list); })
        .catch(err => { if (!cancelled) notify.error('Workspaces could not be loaded', { description: slackError(err) }); });
    };
    load();
    listen(WORKSPACES_CHANGED, load)
      .then(fn => { if (cancelled) fn(); else unlisten = fn; })
      .catch(err => console.error(`Failed to listen for ${WORKSPACES_CHANGED}:`, err));
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);
  return workspaces;
}

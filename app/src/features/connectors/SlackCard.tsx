import React, { useCallback, useEffect, useId, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { ExternalLink, Loader2, MessagesSquare, Unplug } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { SwitchRow } from '../../components/PrivacySettings';
import { workspacesApi, WORKSPACES_CHANGED } from '../workspaces/api';
import type { WorkspaceListing } from '../workspaces/types';
import { listen } from '@tauri-apps/api/event';
import { onSlackStatus, slackApi, slackError } from './api';
import {
  INDEX_PRIVACY_NOTE,
  MODEL_PRIVACY_NOTE,
  RUNNING_HINT,
  answerPlacesLine,
  answerSourceLine,
  canConnect,
  channelLabel,
  exposureLine,
  slackWarnings,
  checkAppToken,
  checkBotToken,
  lastEventLine,
  setupSteps,
  stateLine,
  tokenHint,
  usageLine,
  validCap,
  indexChannelLine,
  memoriesCountLine,
  memoryPlace,
  memoryProvenance,
  ownerLine,
  sharedIndexLine,
  validWindow,
  type ChannelChoice,
  type Person,
  type SlackMemoryView,
  type SlackStatus,
  type Tone,
} from './slackModel';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const BUTTON = cn(
  'inline-flex items-center gap-1.5 h-8 px-3 rounded-lg border border-shodh-border text-[12.5px] text-shodh-text-secondary hover:bg-shodh-raised-2 disabled:opacity-50 disabled:cursor-not-allowed',
  FOCUS_RING,
);

const PRIMARY_BUTTON = cn(
  'inline-flex items-center gap-1.5 h-8 px-3 rounded-lg bg-shodh-accent text-shodh-on-accent text-[12.5px] font-medium hover:opacity-90 disabled:opacity-50 disabled:cursor-not-allowed',
  FOCUS_RING,
);

const INPUT = cn(
  'h-8 rounded-lg border border-shodh-border bg-shodh-ground px-2 text-[12.5px] text-shodh-text disabled:opacity-50',
  FOCUS_RING,
);

const TONE_DOT: Record<Tone, string> = {
  neutral: 'bg-shodh-text-faint',
  active: 'bg-shodh-warning animate-pulse motion-reduce:animate-none',
  ok: 'bg-shodh-success',
  warning: 'bg-shodh-warning',
  error: 'bg-shodh-error',
};

const TONE_TEXT: Record<Tone, string> = {
  neutral: 'text-shodh-text-muted',
  active: 'text-shodh-text-muted',
  ok: 'text-shodh-text-muted',
  warning: 'text-shodh-warning',
  error: 'text-shodh-error',
};

/** How often times on the card ("tomorrow") are refreshed. */
const CLOCK_TICK_MS = 30_000;

function TokenField({
  label,
  value,
  onChange,
  check,
  placeholder,
  disabled,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  check: typeof checkAppToken;
  placeholder: string;
  disabled: boolean;
}) {
  const id = useId();
  const hintId = useId();
  const hint = tokenHint(value, check);
  return (
    <div className="flex flex-col gap-1">
      <label htmlFor={id} className="text-[12.5px] font-semibold text-shodh-text">{label}</label>
      <input
        id={id}
        type="password"
        autoComplete="off"
        spellCheck={false}
        value={value}
        placeholder={placeholder}
        disabled={disabled}
        aria-invalid={hint !== null}
        aria-describedby={hint ? hintId : undefined}
        onChange={e => onChange(e.target.value)}
        className={cn(INPUT, 'w-full font-mono')}
      />
      {hint && <p id={hintId} role="alert" className="m-0 text-[12px] text-shodh-error">{hint}</p>}
    </div>
  );
}

function Setup({ status, onStatus }: { status: SlackStatus; onStatus: (s: SlackStatus) => void }) {
  const [appToken, setAppToken] = useState('');
  const [botToken, setBotToken] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const steps = setupSteps(status);
  const localOnly = status.state === 'local_only';

  const connect = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!canConnect(appToken, botToken)) return;
    setBusy(true);
    setError(null);
    try {
      const next = await slackApi.connect(appToken, botToken);
      setAppToken('');
      setBotToken('');
      onStatus(next);
      notify.success('Slack connected', { description: next.installation ? `Shodh is in ${next.installation.teamName} as @${next.installation.botName}.` : undefined });
    } catch (err) {
      setError(slackError(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={e => void connect(e)} className="flex flex-col gap-4">
      <ol className="m-0 pl-0 list-none flex flex-col gap-3">
        {steps.map((step, i) => (
          <li key={step.title} className="flex gap-3">
            <span aria-hidden="true" className="shrink-0 w-6 h-6 rounded-full bg-shodh-raised-2 text-[12px] font-semibold text-shodh-text-secondary flex items-center justify-center">
              {i + 1}
            </span>
            <div className="min-w-0 flex flex-col gap-1">
              <p className="m-0 text-[13px] font-semibold text-shodh-text">{step.title}</p>
              <p className="m-0 text-[12.5px] text-shodh-text-muted">{step.detail}</p>
              {i === 0 && (
                <div>
                  <button
                    type="button"
                    className={BUTTON}
                    disabled={localOnly}
                    onClick={() => void openUrl(status.createAppUrl).catch(err => notify.error('Slack could not be opened', { description: slackError(err) }))}
                  >
                    <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" />
                    Create the Slack app
                  </button>
                </div>
              )}
            </div>
          </li>
        ))}
      </ol>
      <div className="grid gap-3 sm:grid-cols-2">
        <TokenField label="App-level token" value={appToken} onChange={v => { setAppToken(v); setError(null); }} check={checkAppToken} placeholder="xapp-…" disabled={busy || localOnly} />
        <TokenField label="Bot token" value={botToken} onChange={v => { setBotToken(v); setError(null); }} check={checkBotToken} placeholder="xoxb-…" disabled={busy || localOnly} />
      </div>
      {error && <p role="alert" className="m-0 text-[12.5px] text-shodh-error">{error}</p>}
      <div className="flex items-center gap-3">
        <button type="submit" className={PRIMARY_BUTTON} disabled={busy || localOnly || !canConnect(appToken, botToken)}>
          {busy && <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
          {busy ? 'Checking with Slack…' : 'Connect'}
        </button>
        <p className="m-0 text-[12px] text-shodh-text-muted">{MODEL_PRIVACY_NOTE}</p>
      </div>
    </form>
  );
}

/** The channels Shodh answers in: loaded from Slack on request, saved together. */
function ChannelPicker({
  status,
  onStatus,
  busy,
  setBusy,
}: {
  status: SlackStatus;
  onStatus: (s: SlackStatus) => void;
  busy: boolean;
  setBusy: (busy: boolean) => void;
}) {
  const [choices, setChoices] = useState<ChannelChoice[] | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const headingId = useId();
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
    <div className="flex flex-col gap-1.5" role="group" aria-labelledby={headingId}>
      <p id={headingId} className="m-0 text-[13.5px] font-semibold text-shodh-text">Answer in these channels</p>
      <p className={cn('m-0 text-[12.5px]', TONE_TEXT[places.tone])}>{places.text}</p>
      {choices === null ? (
        <div>
          <button type="button" className={BUTTON} disabled={busy || loading || status.state === 'local_only'} onClick={() => void load()}>
            {loading && <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
            {loading ? 'Asking Slack…' : 'Choose channels…'}
          </button>
        </div>
      ) : choices.length === 0 ? (
        <p className="m-0 text-[12.5px] text-shodh-text-muted">
          The bot is in no channel yet. Invite it in Slack with /invite @{status.installation?.botName ?? 'shodh'}, then choose again.
        </p>
      ) : (
        <div className="flex flex-col gap-2">
          <ul className="m-0 p-0 list-none flex flex-col gap-1 max-h-56 overflow-y-auto scrollbar-thin">
            {choices.map(c => (
              <li key={c.id}>
                <label className="inline-flex items-center gap-2 text-[12.5px] text-shodh-text-secondary">
                  <input type="checkbox" checked={picked.has(c.id)} disabled={busy} onChange={() => toggle(c.id)} />
                  <span className={c.shared ? 'text-shodh-warning' : undefined}>{channelLabel(c)}</span>
                </label>
              </li>
            ))}
          </ul>
          <div className="flex items-center gap-2">
            <button type="button" className={PRIMARY_BUTTON} disabled={busy} onClick={() => void save()}>Save channels</button>
            <button type="button" className={BUTTON} disabled={busy} onClick={() => setChoices(null)}>Cancel</button>
          </div>
        </div>
      )}
      {error && <p role="alert" className="m-0 text-[12.5px] text-shodh-error">{error}</p>}
    </div>
  );
}

/** Channels indexed into Shodh workspaces: their sync, adding one, forgetting one. */
function IndexSection({
  status,
  onStatus,
  busy,
  setBusy,
  workspaces,
  now,
}: {
  status: SlackStatus;
  onStatus: (s: SlackStatus) => void;
  busy: boolean;
  setBusy: (busy: boolean) => void;
  workspaces: WorkspaceListing[] | null;
  now: number;
}) {
  const index = status.index;
  const headingId = useId();
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

  const act = async (action: () => Promise<SlackStatus>, failure: string): Promise<boolean> => {
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

  const add = async () => {
    if (!chosen || !workspaceId) return;
    const ok = await act(
      () => slackApi.indexChannel(chosen.id, workspaceId, chosen.shared && sharedConfirmed),
      'The channel could not be indexed',
    );
    if (ok) setChoices(null);
  };

  return (
    <div className="flex flex-col gap-2" role="group" aria-labelledby={headingId}>
      <p id={headingId} className="m-0 text-[13.5px] font-semibold text-shodh-text">Index these channels</p>
      <p className="m-0 text-[12.5px] text-shodh-text-muted">
        Shodh reads the channels you choose here into a workspace, so their threads can be searched and cited like its
        documents. This list is separate from where Shodh answers. {index.costNote}
      </p>
      <p className="m-0 text-[12.5px] text-shodh-text-muted">{INDEX_PRIVACY_NOTE}</p>
      {!index.available && (
        <p role="alert" className="m-0 text-[12.5px] text-shodh-warning">
          Indexing needs the app database, which could not be opened; see the audit page.
        </p>
      )}
      {index.missingScopesMessage && (
        <p role="alert" className="m-0 text-[12.5px] text-shodh-warning">{index.missingScopesMessage}</p>
      )}

      {index.channels.length > 0 && (
        <ul className="m-0 p-0 list-none flex flex-col gap-2">
          {index.channels.map(c => {
            const line = indexChannelLine(c, now);
            return (
              <li key={c.id} className="flex flex-col gap-1 rounded-lg bg-shodh-surface-2 px-3.5 py-2.5">
                <div className="flex items-start gap-2.5">
                  <span aria-hidden="true" className={cn('mt-1.5 inline-block w-2 h-2 rounded-full shrink-0', TONE_DOT[line.tone])} />
                  <div className="min-w-0 flex-1">
                    <p className="m-0 text-[12.5px] font-semibold text-shodh-text">
                      {channelLabel(c)} → {c.workspace.name || 'a deleted workspace'}
                    </p>
                    <p className="m-0 text-[12.5px] text-shodh-text-secondary">{line.title}</p>
                    {line.detail && <p className={cn('m-0 text-[12px] break-words', TONE_TEXT[line.tone])}>{line.detail}</p>}
                  </div>
                </div>
                {forgetting === c.id ? (
                  <div role="alertdialog" aria-label={`Forget #${c.name}`} className="flex flex-wrap items-center gap-2">
                    <p className="m-0 text-[12px] text-shodh-text-secondary">
                      Stop indexing #{c.name} and forget its {c.threads} threads, its sync state and the memories saved there?
                    </p>
                    <button
                      type="button"
                      className={cn(BUTTON, 'text-shodh-error')}
                      disabled={busy}
                      onClick={() => {
                        setForgetting(null);
                        void act(() => slackApi.forgetChannel(c.id), 'The channel could not be forgotten');
                      }}
                    >
                      Forget
                    </button>
                    <button type="button" className={BUTTON} disabled={busy} onClick={() => setForgetting(null)}>Cancel</button>
                  </div>
                ) : (
                  <div>
                    <button type="button" className={BUTTON} disabled={busy || c.state === 'forgetting'} onClick={() => setForgetting(c.id)}>
                      Stop indexing and forget
                    </button>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}

      {choices === null ? (
        <div>
          <button type="button" className={BUTTON} disabled={busy || loading || blocked} onClick={() => void load()}>
            {loading && <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
            {loading ? 'Asking Slack…' : 'Index a channel…'}
          </button>
        </div>
      ) : offered.length === 0 ? (
        <div className="flex items-center gap-2">
          <p className="m-0 text-[12.5px] text-shodh-text-muted">
            No other channel to index. Invite the bot in Slack with /invite @{status.installation?.botName ?? 'shodh'}, then try again.
          </p>
          <button type="button" className={BUTTON} onClick={() => setChoices(null)}>Close</button>
        </div>
      ) : (
        <div className="flex flex-col gap-2 rounded-lg bg-shodh-surface-2 px-3.5 py-3">
          <div className="flex flex-wrap items-end gap-3">
            <div className="flex flex-col gap-1">
              <label htmlFor={channelPickerId} className="text-[12.5px] font-semibold text-shodh-text">Channel</label>
              <select
                id={channelPickerId}
                value={channelId}
                disabled={busy}
                onChange={e => { setChannelId(e.target.value); setSharedConfirmed(false); }}
                className={cn(INPUT, 'max-w-xs')}
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
                className={cn(INPUT, 'max-w-xs')}
              >
                <option value="">Choose a workspace…</option>
                {(workspaces ?? []).map(w => (
                  <option key={w.id} value={w.id}>{w.name}</option>
                ))}
              </select>
            </div>
          </div>
          {chosen?.shared && (
            <label className="inline-flex items-start gap-2 text-[12.5px] text-shodh-warning">
              <input type="checkbox" checked={sharedConfirmed} disabled={busy} onChange={e => setSharedConfirmed(e.target.checked)} />
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
            <button type="button" className={BUTTON} disabled={busy} onClick={() => setChoices(null)}>Cancel</button>
          </div>
        </div>
      )}
      {choices === null && error && <p role="alert" className="m-0 text-[12.5px] text-shodh-error">{error}</p>}

      <form
        className="flex items-center gap-2"
        onSubmit={e => {
          e.preventDefault();
          if (windowDays !== null && windowDays !== index.windowDays) {
            void act(() => slackApi.setIndexWindow(windowDays), 'The history window was not saved');
          }
        }}
      >
        <label htmlFor={windowId} className="text-[12.5px] text-shodh-text-secondary">
          A newly indexed channel is read back
        </label>
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
          className={cn(INPUT, 'w-20 tabular-nums')}
        />
        <span className="text-[12.5px] text-shodh-text-secondary">days</span>
        <button type="submit" className={BUTTON} disabled={busy || windowDays === null || windowDays === index.windowDays}>Save</button>
      </form>
    </div>
  );
}

/** "This is me": the Slack member whose `remember:` goes into the user's memories. */
function OwnerSection({
  status,
  onStatus,
  busy,
  setBusy,
}: {
  status: SlackStatus;
  onStatus: (s: SlackStatus) => void;
  busy: boolean;
  setBusy: (busy: boolean) => void;
}) {
  const [people, setPeople] = useState<Person[] | null>(null);
  const [picked, setPicked] = useState('');
  const [filter, setFilter] = useState('');
  const [error, setError] = useState<string | null>(null);
  const headingId = useId();
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
    <div className="flex flex-col gap-1.5" role="group" aria-labelledby={headingId}>
      <p id={headingId} className="m-0 text-[13.5px] font-semibold text-shodh-text">You in Slack</p>
      <p className="m-0 text-[12.5px] text-shodh-text-muted">{ownerLine(status.index)}</p>
      {people === null ? (
        <div className="flex items-center gap-2">
          <button type="button" className={BUTTON} disabled={busy || status.state === 'local_only'} onClick={() => void load()}>
            {status.index.owner ? 'Change…' : 'This is me…'}
          </button>
          {status.index.owner && (
            <button type="button" className={BUTTON} disabled={busy} onClick={() => void save(null)}>Clear</button>
          )}
        </div>
      ) : (
        <div className="flex flex-col gap-2 rounded-lg bg-shodh-surface-2 px-3.5 py-3">
          <label htmlFor={filterId} className="text-[12.5px] font-semibold text-shodh-text">Find yourself</label>
          <input id={filterId} value={filter} onChange={e => setFilter(e.target.value)} className={cn(INPUT, 'max-w-xs')} />
          <ul className="m-0 p-0 list-none flex flex-col gap-1 max-h-48 overflow-y-auto scrollbar-thin">
            {shown.map(p => (
              <li key={p.id}>
                <label className="inline-flex items-center gap-2 text-[12.5px] text-shodh-text-secondary">
                  <input type="radio" name="slack-owner" checked={picked === p.id} disabled={busy} onChange={() => setPicked(p.id)} />
                  {p.name}
                </label>
              </li>
            ))}
          </ul>
          <div className="flex items-center gap-2">
            <button type="button" className={PRIMARY_BUTTON} disabled={busy || !picked} onClick={() => void save(picked)}>This is me</button>
            <button type="button" className={BUTTON} disabled={busy} onClick={() => setPeople(null)}>Cancel</button>
          </div>
        </div>
      )}
      {error && <p role="alert" className="m-0 text-[12.5px] text-shodh-error">{error}</p>}
    </div>
  );
}

/** "From Slack": every memory saved from Slack, to promote or delete. */
function MemoriesSection({ status, busy, setBusy }: { status: SlackStatus; busy: boolean; setBusy: (busy: boolean) => void }) {
  const [memories, setMemories] = useState<SlackMemoryView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const headingId = useId();

  const act = async (action: () => Promise<SlackMemoryView[]>, failure: string) => {
    setBusy(true);
    setError(null);
    try {
      setMemories(await action());
    } catch (err) {
      if (failure) notify.error(failure, { description: slackError(err) });
      else setError(slackError(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-1.5" role="group" aria-labelledby={headingId}>
      <p id={headingId} className="m-0 text-[13.5px] font-semibold text-shodh-text">From Slack</p>
      <p className="m-0 text-[12.5px] text-shodh-text-muted">
        {memoriesCountLine(status.index)} “@shodh remember: …” saves at once: yours into your memories, anyone else’s for
        their channel only (used only for Slack answers there, never in Ask).
      </p>
      {memories === null ? (
        <div>
          <button
            type="button"
            className={BUTTON}
            disabled={busy || !status.index.available || status.index.memories === 0}
            onClick={() => void act(slackApi.memories, '')}
          >
            Show memories from Slack
          </button>
        </div>
      ) : (
        <div className="flex flex-col gap-2">
          <ul className="m-0 p-0 list-none flex flex-col gap-2 max-h-72 overflow-y-auto scrollbar-thin">
            {memories.map(m => (
              <li key={m.id} className="flex flex-col gap-1 rounded-lg bg-shodh-surface-2 px-3.5 py-2.5">
                <p className="m-0 text-[12.5px] text-shodh-text whitespace-pre-wrap break-words">{m.text}</p>
                <p className="m-0 text-[12px] text-shodh-text-muted">
                  {memoryProvenance(m)} · {memoryPlace(m)}
                </p>
                <div className="flex flex-wrap items-center gap-2">
                  {!m.inMemories && (
                    <button type="button" className={BUTTON} disabled={busy} onClick={() => void act(() => slackApi.promoteMemory(m.id), 'The memory could not be added')}>
                      Add to my memories
                    </button>
                  )}
                  <button type="button" className={cn(BUTTON, 'text-shodh-error')} disabled={busy} onClick={() => void act(() => slackApi.deleteMemory(m.id), 'The memory could not be deleted')}>
                    Delete
                  </button>
                  {m.permalink && (
                    <button
                      type="button"
                      className={BUTTON}
                      onClick={() => void openUrl(m.permalink as string).catch(err => notify.error('Slack could not be opened', { description: slackError(err) }))}
                    >
                      <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" />
                      Open in Slack
                    </button>
                  )}
                </div>
              </li>
            ))}
          </ul>
          <div>
            <button type="button" className={BUTTON} disabled={busy} onClick={() => setMemories(null)}>Hide</button>
          </div>
        </div>
      )}
      {error && <p role="alert" className="m-0 text-[12.5px] text-shodh-error">{error}</p>}
    </div>
  );
}

function Connected({
  status,
  onStatus,
  now,
}: {
  status: SlackStatus;
  onStatus: (s: SlackStatus) => void;
  now: number;
}) {
  const [workspaces, setWorkspaces] = useState<WorkspaceListing[] | null>(null);
  const [capInput, setCapInput] = useState(String(status.dailyCap));
  const [busy, setBusy] = useState(false);
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);
  /** A workspace picked but not confirmed yet, with what choosing it exposes. */
  const [pending, setPending] = useState<{ id: string; line: string } | null>(null);
  const pickerId = useId();
  const capId = useId();
  const capHelpId = useId();

  useEffect(() => setCapInput(String(status.dailyCap)), [status.dailyCap]);

  useEffect(() => {
    let cancelled = false;
    const load = () => {
      workspacesApi
        .list(false)
        .then(list => { if (!cancelled) setWorkspaces(list); })
        .catch(err => { if (!cancelled) notify.error('Workspaces could not be loaded', { description: slackError(err) }); });
    };
    load();
    let unlisten: (() => void) | null = null;
    listen(WORKSPACES_CHANGED, load)
      .then(fn => { if (cancelled) fn(); else unlisten = fn; })
      .catch(err => console.error(`Failed to listen for ${WORKSPACES_CHANGED}:`, err));
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  const run = async (action: () => Promise<SlackStatus>, failure: string) => {
    setBusy(true);
    try {
      onStatus(await action());
    } catch (err) {
      notify.error(failure, { description: slackError(err) });
    } finally {
      setBusy(false);
    }
  };

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
  const warnings = slackWarnings(status, now);
  const source = answerSourceLine(status);
  const cap = validCap(capInput, status);
  const chosen = status.answerWorkspace?.id ?? '';
  const chosenListed = workspaces?.some(w => w.id === chosen) ?? true;

  return (
    <div className="flex flex-col gap-5">
      {warnings.length > 0 && (
        <ul role="alert" className="m-0 pl-0 list-none flex flex-col gap-1.5 rounded-lg border border-shodh-border bg-shodh-surface-2 px-3.5 py-3">
          {warnings.map(w => (
            <li key={w} className="text-[12.5px] text-shodh-warning">{w}</li>
          ))}
        </ul>
      )}
      <SwitchRow
        label="Answer in Slack"
        description="When on, Shodh keeps a connection to Slack and answers mentions of the bot in the channels chosen below, and direct messages from members of your Slack workspace."
        checked={on}
        disabled={busy || status.state === 'local_only'}
        onChange={next => void run(() => slackApi.setEnabled(next), 'The Slack connector could not be switched')}
      />

      <div className="flex flex-col gap-1.5">
        <label htmlFor={pickerId} className="text-[13.5px] font-semibold text-shodh-text">Slack answers from</label>
        <select
          id={pickerId}
          value={pending?.id ?? chosen}
          disabled={busy || workspaces === null}
          onChange={e => void pick(e.target.value)}
          className={cn(INPUT, 'max-w-sm')}
        >
          <option value="">Choose a workspace…</option>
          {!chosenListed && chosen && <option value={chosen}>{status.answerWorkspace?.name || 'Unavailable workspace'}</option>}
          {(workspaces ?? []).map(w => (
            <option key={w.id} value={w.id}>{w.name}</option>
          ))}
        </select>
        {pending ? (
          <div role="alertdialog" aria-label="Confirm the workspace Slack answers from" className="flex flex-col gap-2 rounded-lg bg-shodh-surface-2 px-3.5 py-3">
            <p className="m-0 text-[12.5px] text-shodh-text-secondary">{pending.line}</p>
            <div className="flex items-center gap-2">
              <button
                type="button"
                className={PRIMARY_BUTTON}
                disabled={busy}
                onClick={() => {
                  const id = pending.id;
                  setPending(null);
                  void run(() => slackApi.setAnswerWorkspace(id), 'The workspace could not be chosen');
                }}
              >
                Answer from this workspace
              </button>
              <button type="button" className={BUTTON} disabled={busy} onClick={() => setPending(null)}>Cancel</button>
            </div>
          </div>
        ) : (
          <p className={cn('m-0 text-[12.5px]', TONE_TEXT[source.tone])}>{source.text}</p>
        )}
      </div>

      <ChannelPicker status={status} onStatus={onStatus} busy={busy} setBusy={setBusy} />

      <IndexSection status={status} onStatus={onStatus} busy={busy} setBusy={setBusy} workspaces={workspaces} now={now} />

      <OwnerSection status={status} onStatus={onStatus} busy={busy} setBusy={setBusy} />

      <MemoriesSection status={status} busy={busy} setBusy={setBusy} />

      <form
        className="flex items-start justify-between gap-6"
        onSubmit={e => {
          e.preventDefault();
          if (cap !== null && cap !== status.dailyCap) void run(() => slackApi.setDailyCap(cap), 'The daily limit was not saved');
        }}
      >
        <div className="min-w-0">
          <label htmlFor={capId} className="m-0 text-[13.5px] font-semibold text-shodh-text">Slack answers per day</label>
          <p id={capHelpId} className="m-0 mt-0.5 text-[12.5px] text-shodh-text-muted">
            Each answer is one model call, counted apart from your own questions and from evidence tables. At the limit the
            bot says so and makes no call. {usageLine(status, now)}
          </p>
          {cap === null && capInput.trim() !== '' && (
            <p role="alert" className="m-0 mt-1 text-[12.5px] text-shodh-error">
              Enter a whole number from {status.minDailyCap} to {status.maxDailyCap}.
            </p>
          )}
        </div>
        <div className="shrink-0 flex items-center gap-2">
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
            className={cn(INPUT, 'w-20 tabular-nums')}
          />
          <button type="submit" className={BUTTON} disabled={busy || cap === null || cap === status.dailyCap}>Save</button>
        </div>
      </form>

      <div className="flex flex-col gap-1 text-[12.5px] text-shodh-text-muted">
        {on && <p className="m-0">{RUNNING_HINT}</p>}
        <p className="m-0">{lastEventLine(status, now)}</p>
        <p className="m-0">{MODEL_PRIVACY_NOTE}</p>
      </div>

      <div className="flex items-center gap-2">
        {!confirmDisconnect ? (
          <button type="button" className={BUTTON} disabled={busy} onClick={() => setConfirmDisconnect(true)}>
            <Unplug className="w-3.5 h-3.5" aria-hidden="true" />
            Disconnect
          </button>
        ) : (
          <div role="alertdialog" aria-label={`Disconnect ${status.installation?.teamName ?? 'Slack'}`} className="flex flex-col gap-2">
            <p className="m-0 text-[12.5px] text-shodh-text-secondary">
              Disconnect {status.installation?.teamName}? The tokens are deleted from this computer and the bot stops answering.
              Keep what was indexed from Slack ({status.index.channels.length} {status.index.channels.length === 1 ? 'channel' : 'channels'},{' '}
              {status.index.memories} {status.index.memories === 1 ? 'memory' : 'memories'}), or forget all of it?
            </p>
            <div className="flex flex-wrap items-center gap-2">
              <button
                type="button"
                className={BUTTON}
                disabled={busy}
                onClick={() => void run(() => slackApi.disconnect(false), 'Slack could not be disconnected').then(() => setConfirmDisconnect(false))}
              >
                Disconnect and keep it
              </button>
              <button
                type="button"
                className={cn(BUTTON, 'text-shodh-error')}
                disabled={busy}
                onClick={() => void run(() => slackApi.disconnect(true), 'Slack could not be disconnected').then(() => setConfirmDisconnect(false))}
              >
                Disconnect and forget everything from Slack
              </button>
              <button type="button" className={BUTTON} disabled={busy} onClick={() => setConfirmDisconnect(false)}>Cancel</button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

/** The Slack card: live state, setup, and the connector's settings. */
export default function SlackCard({ summary }: { summary: string }) {
  const [status, setStatus] = useState<SlackStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const headingId = useId();

  const load = useCallback(() => {
    slackApi
      .status()
      .then(next => { setStatus(next); setLoadError(null); })
      .catch(err => setLoadError(slackError(err)));
  }, []);

  useEffect(() => {
    load();
    const unsubscribe = onSlackStatus(next => setStatus(next));
    const tick = window.setInterval(() => setNow(Date.now()), CLOCK_TICK_MS);
    return () => {
      unsubscribe();
      window.clearInterval(tick);
    };
  }, [load]);

  const line = status ? stateLine(status, now) : null;

  return (
    <section aria-labelledby={headingId} className="rounded-xl border border-shodh-border bg-shodh-surface p-5 flex flex-col gap-5">
      <header className="flex items-start gap-3">
        <span aria-hidden="true" className="shrink-0 w-9 h-9 rounded-lg bg-shodh-raised-2 flex items-center justify-center">
          <MessagesSquare className="w-5 h-5 text-shodh-text-secondary" />
        </span>
        <div className="min-w-0 flex-1 flex flex-col gap-1">
          <h2 id={headingId} className="m-0 text-[15px] font-semibold text-shodh-text">Slack</h2>
          <p className="m-0 text-[12.5px] text-shodh-text-muted">{summary}</p>
        </div>
      </header>

      {loadError && !status && (
        <p role="alert" className="m-0 text-[12.5px] text-shodh-error">The Slack connector could not be loaded: {loadError}</p>
      )}
      {!status && !loadError && <p className="m-0 text-[12.5px] text-shodh-text-muted">Loading…</p>}

      {status && line && (
        <>
          <div role="status" aria-live="polite" className="flex items-start gap-2.5 rounded-lg bg-shodh-surface-2 px-3.5 py-3">
            <span aria-hidden="true" className={cn('mt-1.5 inline-block w-2 h-2 rounded-full shrink-0', TONE_DOT[line.tone])} />
            <div className="min-w-0">
              <p className="m-0 text-[13px] font-semibold text-shodh-text">{line.title}</p>
              {line.detail && <p className={cn('m-0 mt-0.5 text-[12.5px] break-words', TONE_TEXT[line.tone])}>{line.detail}</p>}
            </div>
          </div>
          {status.installation ? (
            <Connected status={status} onStatus={setStatus} now={now} />
          ) : (
            <Setup status={status} onStatus={setStatus} />
          )}
        </>
      )}
    </section>
  );
}

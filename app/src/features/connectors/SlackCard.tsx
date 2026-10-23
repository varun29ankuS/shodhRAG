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
  MODEL_PRIVACY_NOTE,
  RUNNING_HINT,
  answerSourceLine,
  canConnect,
  checkAppToken,
  checkBotToken,
  lastEventLine,
  setupSteps,
  stateLine,
  tokenHint,
  usageLine,
  validCap,
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
        <TokenField label="App-level token" value={appToken} onChange={setAppToken} check={checkAppToken} placeholder="xapp-…" disabled={busy || localOnly} />
        <TokenField label="Bot token" value={botToken} onChange={setBotToken} check={checkBotToken} placeholder="xoxb-…" disabled={busy || localOnly} />
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

  const on = status.state !== 'off' && status.state !== 'local_only';
  const source = answerSourceLine(status);
  const cap = validCap(capInput, status);
  const chosen = status.answerWorkspace?.id ?? '';
  const chosenListed = workspaces?.some(w => w.id === chosen) ?? true;

  return (
    <div className="flex flex-col gap-5">
      <SwitchRow
        label="Answer in Slack"
        description="When on, Shodh keeps a connection to Slack and answers mentions of the bot and direct messages to it."
        checked={on}
        disabled={busy || status.state === 'local_only'}
        onChange={next => void run(() => slackApi.setEnabled(next), 'The Slack connector could not be switched')}
      />

      <div className="flex flex-col gap-1.5">
        <label htmlFor={pickerId} className="text-[13.5px] font-semibold text-shodh-text">Slack answers from</label>
        <select
          id={pickerId}
          value={chosen}
          disabled={busy || workspaces === null}
          onChange={e => void run(() => slackApi.setAnswerWorkspace(e.target.value || null), 'The workspace could not be chosen')}
          className={cn(INPUT, 'max-w-sm')}
        >
          <option value="">Choose a workspace…</option>
          {!chosenListed && chosen && <option value={chosen}>{status.answerWorkspace?.name || 'Unavailable workspace'}</option>}
          {(workspaces ?? []).map(w => (
            <option key={w.id} value={w.id}>{w.name}</option>
          ))}
        </select>
        <p className={cn('m-0 text-[12.5px]', TONE_TEXT[source.tone])}>{source.text}</p>
      </div>

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
          <>
            <p className="m-0 text-[12.5px] text-shodh-text-secondary">
              Disconnect {status.installation?.teamName}? The tokens are deleted from this computer and the bot stops answering.
            </p>
            <button
              type="button"
              className={cn(BUTTON, 'text-shodh-error')}
              disabled={busy}
              onClick={() => void run(slackApi.disconnect, 'Slack could not be disconnected').then(() => setConfirmDisconnect(false))}
            >
              Disconnect
            </button>
            <button type="button" className={BUTTON} disabled={busy} onClick={() => setConfirmDisconnect(false)}>Cancel</button>
          </>
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

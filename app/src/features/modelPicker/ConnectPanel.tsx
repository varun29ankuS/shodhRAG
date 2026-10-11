import { useEffect, useId, useRef, useState } from 'react';
import type React from 'react';
import { listen } from '@tauri-apps/api/event';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Check, Copy, KeyRound, Loader2, LogIn, Monitor } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import type { ModelPickerHandle, SignInEvent } from './modelApi';
import { SIGN_IN_EVENT, connectApi, toConnectError } from './modelApi';
import type { ProviderId, SubscriptionStatus } from './modelTypes';
import { PROVIDER_LABELS } from './modelTypes';

export const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

export const BUTTON = cn(
  'inline-flex items-center justify-center gap-1.5 h-8 px-3 rounded-lg border border-shodh-border bg-shodh-raised text-[12.5px] font-medium text-shodh-text whitespace-nowrap hover:bg-shodh-raised-2 active:scale-[0.98] transition-colors duration-micro disabled:opacity-50 disabled:pointer-events-none',
  FOCUS_RING,
);

export const PRIMARY_BUTTON = cn(
  'inline-flex items-center justify-center gap-1.5 h-8 px-3 rounded-lg bg-shodh-accent text-[12.5px] font-semibold text-shodh-on-accent whitespace-nowrap hover:bg-shodh-accent-hover active:scale-[0.98] transition-colors duration-micro disabled:opacity-50 disabled:pointer-events-none',
  FOCUS_RING,
);

export const INPUT = cn(
  'h-9 px-2.5 rounded-lg border border-shodh-border bg-shodh-ground text-[13px] text-shodh-text placeholder:text-shodh-text-faint',
  FOCUS_RING,
);

/** A running sign-in, as the panel shows it. */
interface SignInProgress {
  provider: ProviderId;
  page: string | null;
  code: string | null;
  message: string | null;
}

function statusText(s: SubscriptionStatus): string | null {
  switch (s.state) {
    case 'connected':
      return s.accounts.length > 0 ? `Connected as ${s.accounts[0]}` : 'Connected';
    case 'expired':
      return 'Sign-in expired. Sign in again.';
    case 'signed_out':
      return null;
  }
}

/**
 * Settings → Model → Connect: sign in with a subscription, paste an API
 * key, or run a model on this computer. The first-run setup uses it too.
 */
export function ConnectPanel({ picker }: { picker: ModelPickerHandle }) {
  const { view, reload } = picker;
  const [signIn, setSignIn] = useState<SignInProgress | null>(null);
  const [starting, setStarting] = useState<ProviderId | null>(null);
  const [signInError, setSignInError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<SignInEvent>(SIGN_IN_EVENT, event => {
      const e = event.payload;
      if (e.phase === 'step' && e.step) {
        const step = e.step;
        setSignIn(current => {
          const base: SignInProgress = current && current.provider === e.provider ? current : { provider: e.provider, page: null, code: null, message: null };
          if (step.kind === 'open_page') return { ...base, page: step.url };
          if (step.kind === 'device_code') return { ...base, code: step.code };
          if (step.kind === 'progress') return { ...base, message: step.message };
          return base;
        });
        return;
      }
      setSignIn(null);
      if (e.phase === 'connected') {
        notify.success(`${PROVIDER_LABELS[e.provider]} is connected`);
        void reload(false);
      } else if (e.phase === 'failed') {
        setSignInError(e.message ?? 'The sign-in did not finish.');
      }
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(error => console.error('Sign-in listener failed:', error));
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [reload]);

  const startSignIn = async (provider: ProviderId) => {
    setSignInError(null);
    setStarting(provider);
    try {
      try {
        await connectApi.signIn(provider);
      } catch (error) {
        const e = toConnectError(error);
        if (e.code !== 'runtime_missing') throw e;
        notify.info('Downloading the assistant runtime first');
        await connectApi.installRuntime();
        await connectApi.signIn(provider);
      }
      setSignIn({ provider, page: null, code: null, message: 'Opening the sign-in page in your browser.' });
    } catch (error) {
      setSignInError(toConnectError(error).message);
    } finally {
      setStarting(null);
    }
  };

  if (!view) {
    return (
      <p role="status" className="flex items-center gap-2 text-[12.5px] text-shodh-text-muted">
        <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        Checking your connections…
      </p>
    );
  }

  return (
    <div className="flex flex-col divide-y divide-shodh-border-subtle">
      <SubscriptionsRow
        subscriptions={view.subscriptions}
        signIn={signIn}
        starting={starting}
        error={signInError}
        localOnly={view.localOnly}
        onSignIn={provider => void startSignIn(provider)}
        onCancel={() => {
          void connectApi.cancelSignIn();
          setSignIn(null);
        }}
      />
      <ApiKeyRow picker={picker} />
      <LocalRow picker={picker} />
    </div>
  );
}

function RowHeading({ icon: Icon, title, children }: { icon: typeof KeyRound; title: string; children: React.ReactNode }) {
  return (
    <div className="flex items-start gap-3">
      <Icon className="w-4 h-4 mt-0.5 shrink-0 text-shodh-text-muted" aria-hidden="true" />
      <div className="min-w-0">
        <h4 className="m-0 text-[13.5px] font-semibold text-shodh-text">{title}</h4>
        <p className="m-0 mt-0.5 text-[12.5px] text-shodh-text-muted max-w-[65ch]">{children}</p>
      </div>
    </div>
  );
}

function SubscriptionsRow({
  subscriptions,
  signIn,
  starting,
  error,
  localOnly,
  onSignIn,
  onCancel,
}: {
  subscriptions: SubscriptionStatus[];
  signIn: SignInProgress | null;
  starting: ProviderId | null;
  error: string | null;
  localOnly: boolean;
  onSignIn: (provider: ProviderId) => void;
  onCancel: () => void;
}) {
  const [copied, setCopied] = useState(false);
  return (
    <section className="py-4 first:pt-0 flex flex-col gap-3" aria-label="Sign in with a subscription">
      <RowHeading icon={LogIn} title="Sign in with a subscription">
        Use a plan you already pay for. Shodh opens the provider&apos;s page in your browser.
      </RowHeading>
      <ul className="flex flex-col gap-2 pl-7">
        {subscriptions.map(s => {
          const status = statusText(s);
          const busy = starting === s.provider || signIn?.provider === s.provider;
          return (
            <li key={s.provider} className="flex flex-col gap-1">
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                <span className="min-w-[10rem] text-[13px] font-medium text-shodh-text">{s.label}</span>
                {status && (
                  <span className={cn('inline-flex items-center gap-1 text-[12px]', s.state === 'connected' ? 'text-shodh-success' : 'text-shodh-warning')}>
                    {s.state === 'connected' && <Check className="w-3.5 h-3.5" aria-hidden="true" />}
                    {status}
                  </span>
                )}
                {s.state !== 'connected' && (
                  <button
                    type="button"
                    className={cn(BUTTON, 'ml-auto')}
                    disabled={localOnly || busy || (signIn !== null && signIn.provider !== s.provider) || starting !== null}
                    onClick={() => onSignIn(s.provider)}
                    aria-label={`Sign in with ${s.label}`}
                  >
                    {busy && <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
                    Sign in
                  </button>
                )}
              </div>
              {s.note && <p className="m-0 text-[12px] text-shodh-text-muted max-w-[65ch]">{s.note}</p>}
            </li>
          );
        })}
      </ul>
      {localOnly && <p className="m-0 pl-7 text-[12px] text-shodh-text-muted">Local-only mode is on, so cloud sign-ins are off.</p>}
      {signIn && (
        <div role="status" className="ml-7 p-3 rounded-lg bg-shodh-raised flex flex-col gap-2 text-[12.5px] text-shodh-text-secondary">
          <p className="m-0 flex items-center gap-2">
            <Loader2 className="w-3.5 h-3.5 shrink-0 animate-spin motion-reduce:animate-none" aria-hidden="true" />
            Finish signing in to {PROVIDER_LABELS[signIn.provider]} in your browser.
          </p>
          {signIn.code && (
            <div className="flex flex-wrap items-center gap-2">
              <span>Enter this code on the page:</span>
              <code className="px-2 py-0.5 rounded-md bg-shodh-ground font-mono text-[14px] tracking-wider text-shodh-text">{signIn.code}</code>
              <button
                type="button"
                className={BUTTON}
                onClick={() => {
                  void navigator.clipboard.writeText(signIn.code ?? '').then(() => setCopied(true));
                }}
              >
                <Copy className="w-3.5 h-3.5" aria-hidden="true" />
                {copied ? 'Copied' : 'Copy'}
              </button>
            </div>
          )}
          {signIn.message && <p className="m-0 text-[12px] text-shodh-text-muted">{signIn.message}</p>}
          <div className="flex gap-2">
            {signIn.page && (
              <button type="button" className={BUTTON} onClick={() => void openUrl(signIn.page ?? '')}>
                Open the page again
              </button>
            )}
            <button type="button" className={BUTTON} onClick={onCancel}>
              Cancel
            </button>
          </div>
        </div>
      )}
      {error && (
        <p role="alert" className="m-0 pl-7 text-[12.5px] text-shodh-error">
          {error}
        </p>
      )}
    </section>
  );
}

function ApiKeyRow({ picker }: { picker: ModelPickerHandle }) {
  const { view, reload } = picker;
  const [key, setKey] = useState('');
  const [candidates, setCandidates] = useState<ProviderId[]>([]);
  const [chosen, setChosen] = useState<ProviderId | ''>('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const keyId = useId();
  const helpId = useId();
  const errorId = useId();
  const providerId = useId();

  const save = async () => {
    if (!key.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const saved = await connectApi.saveKey(key, chosen || null);
      setKey('');
      setCandidates([]);
      setChosen('');
      notify.success(`${saved.label} is connected`);
      await reload(false);
    } catch (e) {
      const err = toConnectError(e);
      if (err.code === 'ambiguous') {
        setCandidates(err.candidates);
        setChosen(err.candidates[0] ?? '');
        setError(err.message);
      } else {
        setError(err.message);
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="py-4 flex flex-col gap-3" aria-label="Paste an API key">
      <RowHeading icon={KeyRound} title="Paste an API key">
        The provider is recognised from the key. It is checked once with that provider and kept in the system credential store.
      </RowHeading>
      <form
        className="pl-7 flex flex-col gap-2"
        onSubmit={e => {
          e.preventDefault();
          void save();
        }}
      >
        <label htmlFor={keyId} className="text-[12.5px] font-medium text-shodh-text-secondary">
          API key
        </label>
        <div className="flex flex-wrap gap-2">
          <input
            ref={inputRef}
            id={keyId}
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={key}
            onChange={e => {
              setKey(e.target.value);
              setCandidates([]);
              setChosen('');
              setError(null);
            }}
            placeholder="sk-or-…, sk-ant-…, sk-proj-…, AIza…, xai-…"
            aria-describedby={error ? `${helpId} ${errorId}` : helpId}
            aria-invalid={error !== null && candidates.length === 0}
            disabled={view?.localOnly}
            className={cn(INPUT, 'flex-1 min-w-[16rem] font-mono')}
          />
          <button type="submit" className={PRIMARY_BUTTON} disabled={busy || !key.trim() || view?.localOnly}>
            {busy && <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
            {busy ? 'Checking' : 'Connect'}
          </button>
        </div>
        {candidates.length > 0 && (
          <div className="flex flex-col gap-1">
            <label htmlFor={providerId} className="text-[12.5px] font-medium text-shodh-text-secondary">
              Provider
            </label>
            <select
              id={providerId}
              value={chosen}
              onChange={e => setChosen(e.target.value as ProviderId)}
              className={cn(INPUT, 'max-w-[16rem]')}
            >
              {candidates.map(p => (
                <option key={p} value={p}>
                  {PROVIDER_LABELS[p]}
                </option>
              ))}
            </select>
          </div>
        )}
        <p id={helpId} className="m-0 text-[12px] text-shodh-text-muted">
          {view && view.keys.length > 0
            ? `Connected: ${view.keys.map(k => (k.source === 'environment' ? `${k.label} (set by environment)` : k.label)).join(', ')}.`
            : 'OpenRouter, Anthropic, OpenAI, Google and xAI keys work.'}
        </p>
        {error && (
          <p id={errorId} role="alert" className={cn('m-0 text-[12.5px]', candidates.length > 0 ? 'text-shodh-text-secondary' : 'text-shodh-error')}>
            {error}
          </p>
        )}
      </form>
    </section>
  );
}

function LocalRow({ picker }: { picker: ModelPickerHandle }) {
  const { view, reload, loading } = picker;
  if (!view) return null;
  const { lmstudio, ollama } = view;
  return (
    <section className="py-4 last:pb-0 flex flex-col gap-3" aria-label="Run locally">
      <RowHeading icon={Monitor} title="Run locally">
        Models on this computer answer without anything leaving it.
      </RowHeading>
      <div className="pl-7 flex flex-col gap-3 text-[12.5px]">
        <div className="flex flex-col gap-1">
          <span className="font-medium text-shodh-text">LM Studio</span>
          {lmstudio.running ? (
            lmstudio.models.length > 0 ? (
              <span className="text-shodh-text-secondary">{`Running with ${lmstudio.models.join(', ')}.`}</span>
            ) : (
              <span className="text-shodh-text-muted">Running, but no model is loaded. Load one in LM Studio.</span>
            )
          ) : (
            <span className="text-shodh-text-muted">Not running. Start the LM Studio server (port 1234) to use its models.</span>
          )}
        </div>
        <div className="flex flex-col gap-1">
          <span className="font-medium text-shodh-text">Ollama</span>
          {ollama.running ? (
            <>
              <ul className="m-0 p-0 list-none flex flex-wrap gap-1.5" aria-label="Ollama models (not available)">
                {ollama.models.map(m => (
                  <li key={m} aria-disabled="true" className="px-2 py-0.5 rounded-md bg-shodh-raised text-shodh-text-faint">
                    {m}
                  </li>
                ))}
              </ul>
              <span className="text-shodh-text-muted">{view.ollamaNote}</span>
            </>
          ) : (
            <span className="text-shodh-text-muted">Not running. {view.ollamaNote}</span>
          )}
        </div>
        <div>
          <button type="button" className={BUTTON} disabled={loading} onClick={() => void reload(false)}>
            {loading && <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
            Check again
          </button>
        </div>
      </div>
    </section>
  );
}

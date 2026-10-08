import { useEffect, useId, useState } from 'react';
import { ArrowDown, ArrowUp, ChevronRight } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import type { ModelPickerHandle } from './modelApi';
import { connectApi, modelApi, toConnectError } from './modelApi';
import { BUTTON, FOCUS_RING, INPUT } from './ConnectPanel';
import type { PickerError, ProviderId } from './modelTypes';
import { PROVIDER_LABELS, toPickerError } from './modelTypes';

/** Providers whose address the runtime lets you change. */
const BASE_URL_PROVIDERS: readonly ProviderId[] = ['openai', 'anthropic', 'openrouter', 'grok', 'lmstudio'];

/**
 * Settings → Model → Advanced (collapsed by default): a model by id, base
 * URLs, the fallback order of providers, and removing a connection.
 */
export function AdvancedModelSettings({ picker }: { picker: ModelPickerHandle }) {
  const { view } = picker;
  const [open, setOpen] = useState(false);
  const panelId = useId();
  if (!view) return null;
  return (
    <section className="rounded-[14px] bg-shodh-surface border border-shodh-border">
      <h3 className="m-0">
        <button
          type="button"
          aria-expanded={open}
          aria-controls={panelId}
          onClick={() => setOpen(o => !o)}
          className={cn('w-full flex items-center gap-2 px-5 py-4 text-left text-[15px] font-semibold text-shodh-text rounded-[14px]', FOCUS_RING)}
        >
          <ChevronRight className={cn('w-4 h-4 text-shodh-text-muted transition-transform duration-micro', open && 'rotate-90')} aria-hidden="true" />
          Advanced
        </button>
      </h3>
      {open && (
        <div id={panelId} className="px-5 pb-5 flex flex-col gap-6">
          <CustomModel picker={picker} />
          <FallbackOrder picker={picker} />
          <BaseUrls picker={picker} />
          <Connections picker={picker} />
        </div>
      )}
    </section>
  );
}

function SubHeading({ children, hint }: { children: string; hint: string }) {
  return (
    <div className="flex flex-col gap-0.5">
      <h4 className="m-0 text-[13.5px] font-semibold text-shodh-text">{children}</h4>
      <p className="m-0 text-[12px] text-shodh-text-muted max-w-[65ch]">{hint}</p>
    </div>
  );
}

function CustomModel({ picker }: { picker: ModelPickerHandle }) {
  const { view, select } = picker;
  const usable = (view?.connected ?? []).filter(p => p !== 'ollama');
  const [provider, setProvider] = useState<ProviderId | ''>(usable[0] ?? '');
  const [id, setId] = useState('');
  const [busy, setBusy] = useState(false);
  const [refusal, setRefusal] = useState<PickerError | null>(null);
  const providerId = useId();
  const modelId = useId();
  const errorId = useId();

  useEffect(() => {
    if (!provider && usable[0]) setProvider(usable[0]);
  }, [provider, usable]);

  const use = async (sessionOverride: boolean) => {
    if (!provider || !id.trim()) return;
    setBusy(true);
    setRefusal(null);
    const error = await select({ provider, model: id.trim() }, { sessionOverride });
    setBusy(false);
    if (error) setRefusal(error);
    else {
      notify.success(`${id.trim()} will answer from now on`);
      setId('');
    }
  };

  return (
    <div className="flex flex-col gap-2">
      <SubHeading hint="Any model id your connected provider offers. It replaces the pick above.">Model by id</SubHeading>
      {usable.length === 0 ? (
        <p className="m-0 text-[12.5px] text-shodh-text-muted">Connect a provider first.</p>
      ) : (
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={e => {
            e.preventDefault();
            void use(false);
          }}
        >
          <div className="flex flex-col gap-1">
            <label htmlFor={providerId} className="text-[12px] font-medium text-shodh-text-secondary">
              Provider
            </label>
            <select id={providerId} value={provider} onChange={e => setProvider(e.target.value as ProviderId)} className={INPUT}>
              {usable.map(p => (
                <option key={p} value={p}>
                  {PROVIDER_LABELS[p]}
                </option>
              ))}
            </select>
          </div>
          <div className="flex flex-col gap-1 flex-1 min-w-[14rem]">
            <label htmlFor={modelId} className="text-[12px] font-medium text-shodh-text-secondary">
              Model id
            </label>
            <input
              id={modelId}
              value={id}
              onChange={e => setId(e.target.value)}
              spellCheck={false}
              placeholder="e.g. claude-sonnet-5-5"
              aria-describedby={refusal ? errorId : undefined}
              className={cn(INPUT, 'font-mono')}
            />
          </div>
          <button type="submit" className={BUTTON} disabled={busy || !id.trim()}>
            Use
          </button>
        </form>
      )}
      {refusal && (
        <div id={errorId} role="alert" className="flex flex-wrap items-center gap-2 text-[12.5px] text-shodh-text-secondary">
          <span className="flex-1 min-w-[14rem]">{refusal.message}</span>
          {refusal.code === 'environment_set' && (
            <button type="button" className={BUTTON} onClick={() => void use(true)}>
              Use for this session
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function FallbackOrder({ picker }: { picker: ModelPickerHandle }) {
  const { view, reload } = picker;
  const [saving, setSaving] = useState(false);
  if (!view) return null;
  const connected = view.providerOrder.filter(p => view.connected.includes(p) && p !== 'ollama');
  const move = async (index: number, delta: number) => {
    const next = [...connected];
    const target = index + delta;
    if (target < 0 || target >= next.length) return;
    [next[index], next[target]] = [next[target], next[index]];
    setSaving(true);
    try {
      await modelApi.setProviderOrder(next);
      await reload(false);
    } catch (error) {
      notify.error('The order was not saved', { description: toPickerError(error).message });
    } finally {
      setSaving(false);
    }
  };
  return (
    <div className="flex flex-col gap-2">
      <SubHeading hint="Each pick tries your connected providers in this order.">Fallback order</SubHeading>
      {connected.length < 2 ? (
        <p className="m-0 text-[12.5px] text-shodh-text-muted">Connect two or more providers to change the order.</p>
      ) : (
        <ol className="m-0 p-0 list-none flex flex-col gap-1 max-w-[26rem]">
          {connected.map((p, i) => (
            <li key={p} className="flex items-center gap-2 px-3 h-9 rounded-lg bg-shodh-ground text-[13px] text-shodh-text">
              <span className="w-5 text-shodh-text-muted tabular-nums">{i + 1}.</span>
              <span className="flex-1">{PROVIDER_LABELS[p]}</span>
              <button
                type="button"
                disabled={saving || i === 0}
                onClick={() => void move(i, -1)}
                aria-label={`Move ${PROVIDER_LABELS[p]} up`}
                className={cn('w-7 h-7 inline-flex items-center justify-center rounded-md text-shodh-text-muted hover:bg-shodh-raised disabled:opacity-40', FOCUS_RING)}
              >
                <ArrowUp className="w-3.5 h-3.5" aria-hidden="true" />
              </button>
              <button
                type="button"
                disabled={saving || i === connected.length - 1}
                onClick={() => void move(i, 1)}
                aria-label={`Move ${PROVIDER_LABELS[p]} down`}
                className={cn('w-7 h-7 inline-flex items-center justify-center rounded-md text-shodh-text-muted hover:bg-shodh-raised disabled:opacity-40', FOCUS_RING)}
              >
                <ArrowDown className="w-3.5 h-3.5" aria-hidden="true" />
              </button>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}

function BaseUrlRow({ provider, saved, onSaved }: { provider: ProviderId; saved: string; onSaved: () => void }) {
  const [value, setValue] = useState(saved);
  const [error, setError] = useState<string | null>(null);
  const inputId = useId();
  const errorId = useId();
  useEffect(() => setValue(saved), [saved]);
  const save = async () => {
    setError(null);
    try {
      await modelApi.setBaseUrl(provider, value.trim() || null);
      onSaved();
    } catch (e) {
      setError(toPickerError(e).message);
    }
  };
  return (
    <div className="flex flex-col gap-1">
      <label htmlFor={inputId} className="text-[12px] font-medium text-shodh-text-secondary">
        {PROVIDER_LABELS[provider]}
      </label>
      <div className="flex gap-2">
        <input
          id={inputId}
          value={value}
          onChange={e => setValue(e.target.value)}
          spellCheck={false}
          placeholder={provider === 'lmstudio' ? 'http://127.0.0.1:1234/v1' : 'Default'}
          aria-describedby={error ? errorId : undefined}
          aria-invalid={error !== null}
          className={cn(INPUT, 'flex-1 min-w-0 font-mono')}
        />
        <button type="button" className={BUTTON} disabled={value.trim() === saved} onClick={() => void save()}>
          Save
        </button>
      </div>
      {error && (
        <p id={errorId} role="alert" className="m-0 text-[12px] text-shodh-error">
          {error}
        </p>
      )}
    </div>
  );
}

function BaseUrls({ picker }: { picker: ModelPickerHandle }) {
  const { view, reload } = picker;
  if (!view) return null;
  const shown = BASE_URL_PROVIDERS.filter(p => p === 'lmstudio' || view.connected.includes(p));
  return (
    <div className="flex flex-col gap-2">
      <SubHeading hint="Send a provider's requests to a gateway or proxy instead. Leave empty for the default.">Base URL</SubHeading>
      <div className="flex flex-col gap-3 max-w-[32rem]">
        {shown.map(p => (
          <BaseUrlRow
            key={p}
            provider={p}
            saved={view.prefs.baseUrls.find(b => b.provider === p)?.url ?? ''}
            onSaved={() => void reload(false)}
          />
        ))}
      </div>
    </div>
  );
}

function Connections({ picker }: { picker: ModelPickerHandle }) {
  const { view, reload } = picker;
  const [busy, setBusy] = useState<ProviderId | null>(null);
  if (!view) return null;
  const signedIn = view.subscriptions.filter(s => s.state !== 'signed_out');
  const remove = async (provider: ProviderId, subscription: boolean) => {
    setBusy(provider);
    try {
      if (subscription) await connectApi.signOut(provider);
      else await connectApi.removeKey(provider);
      notify.success(`${PROVIDER_LABELS[provider]} is disconnected`);
      await reload(false);
    } catch (error) {
      notify.error('The connection was not removed', { description: toConnectError(error).message });
    } finally {
      setBusy(null);
    }
  };
  const rows = [
    ...signedIn.map(s => ({ provider: s.provider, subscription: true, fixed: false })),
    ...view.keys.map(k => ({ provider: k.provider, subscription: false, fixed: k.source === 'environment' })),
  ];
  return (
    <div className="flex flex-col gap-2">
      <SubHeading hint="Signing out removes the account from Shodh only. Removing a key deletes it from the system credential store.">
        Remove a connection
      </SubHeading>
      {rows.length === 0 ? (
        <p className="m-0 text-[12.5px] text-shodh-text-muted">Nothing is connected.</p>
      ) : (
        <ul className="m-0 p-0 list-none flex flex-col gap-1 max-w-[26rem]">
          {rows.map(r => (
            <li key={r.provider} className="flex items-center gap-2 px-3 h-9 rounded-lg bg-shodh-ground text-[13px] text-shodh-text">
              <span className="flex-1">{PROVIDER_LABELS[r.provider]}</span>
              {r.fixed ? (
                <span className="text-[12px] text-shodh-text-muted">Set by environment</span>
              ) : (
                <button type="button" className={BUTTON} disabled={busy !== null} onClick={() => void remove(r.provider, r.subscription)}>
                  {r.subscription ? 'Sign out' : 'Remove key'}
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

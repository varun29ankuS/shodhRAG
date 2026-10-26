import React, { useEffect, useId, useRef, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Check, Copy, ExternalLink, Loader2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { slackError } from './api';
import { telegramApi } from './telegramApi';
import {
  BOTFATHER_URL,
  TELEGRAM_MODEL_NOTE,
  TELEGRAM_STEPS,
  checkTelegramToken,
  telegramStep,
  telegramSteps,
  type TelegramStatus,
  type TelegramStep,
} from './telegramModel';
import { LinkCard } from './TelegramLink';
import { Callout, FOCUS_RING, INPUT, LearnMore, OUTLINE_BUTTON, PRIMARY_BUTTON, QUIET_BUTTON } from './ui';

/** The wizard's place and the pasted token: in memory while the view is open, never saved. */
export interface TelegramWizardState {
  step: TelegramStep;
  token: string;
  /** What Telegram refused on Connect. */
  error: string | null;
}

export const EMPTY_TELEGRAM_WIZARD: TelegramWizardState = { step: 1, token: '', error: null };

/**
 * Setting Telegram up, one step at a time: make a bot with @BotFather, paste its token
 * (checked with Telegram before anything is saved), link the user's own account with a
 * one-time link or QR code. "Later" leaves the link for the overview.
 */
export function TelegramSetup({
  status,
  wizard,
  onWizard,
  onStatus,
  onLater,
}: {
  status: TelegramStatus;
  wizard: TelegramWizardState;
  onWizard: (next: TelegramWizardState) => void;
  onStatus: (status: TelegramStatus) => void;
  onLater: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const firstRender = useRef(true);
  const tokenId = useId();
  const hintId = useId();
  const steps = telegramSteps();
  const step = telegramStep(status, wizard.step);
  const current = steps[step - 1];
  const localOnly = status.state === 'local_only';
  const check = checkTelegramToken(wizard.token);

  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false;
      return;
    }
    if (step !== 2) headingRef.current?.focus();
  }, [step]);

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 2000);
    return () => window.clearTimeout(timer);
  }, [copied]);

  const go = (next: TelegramStep) => onWizard({ ...wizard, step: next });

  const openBotFather = () =>
    void openUrl(BOTFATHER_URL).catch(err => notify.error('Telegram could not be opened', { description: slackError(err) }));

  const copyLink = async () => {
    try {
      await navigator.clipboard.writeText(BOTFATHER_URL);
      setCopied(true);
    } catch (err) {
      notify.error('The link could not be copied', { description: slackError(err) });
    }
  };

  const connect = async () => {
    if (!check.ok) return;
    setBusy(true);
    onWizard({ ...wizard, error: null });
    try {
      const next = await telegramApi.connect(wizard.token);
      onWizard({ ...EMPTY_TELEGRAM_WIZARD, step: 3 });
      onStatus(next);
      notify.success('Telegram connected', { description: next.bot ? `Shodh reads @${next.bot.username}.` : undefined });
    } catch (err) {
      onWizard({ ...wizard, error: slackError(err) });
    } finally {
      setBusy(false);
    }
  };

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (busy || localOnly) return;
    if (step === 1) go(2);
    else if (step === 2) void connect();
  };

  return (
    <div className="flex flex-col gap-5">
      {localOnly && (
        <Callout tone="neutral" role="status">
          Local-only mode is on, so Shodh does not connect to Telegram. Turn it off in Settings → Privacy to set Telegram up.
        </Callout>
      )}

      <nav aria-label="Setup progress" className="flex flex-col gap-2">
        <p aria-live="polite" className="m-0 text-[12px] font-medium text-shodh-text-muted">
          Step {step} of {TELEGRAM_STEPS.length}
        </p>
        <ol className="m-0 p-0 list-none grid grid-cols-3 gap-2">
          {steps.map(s => {
            const done = s.step < step;
            const here = s.step === step;
            const broken = s.step === 2 && wizard.error !== null && step === 2;
            // Back to step 1 only before a bot is connected.
            const reachable = !status.bot && s.step < 3;
            return (
              <li key={s.step} className="min-w-0">
                <button
                  type="button"
                  disabled={!reachable || busy || here}
                  aria-current={here ? 'step' : undefined}
                  onClick={() => go(s.step)}
                  className={cn('w-full flex flex-col gap-1.5 text-left rounded-md disabled:cursor-default', FOCUS_RING)}
                >
                  <span
                    aria-hidden="true"
                    className={cn(
                      'block h-1 rounded-full transition-colors duration-micro motion-reduce:transition-none',
                      broken ? 'bg-shodh-error' : here || done ? 'bg-shodh-accent' : 'bg-shodh-raised-2',
                    )}
                  />
                  <span className={cn('hidden sm:block truncate text-[11.5px]', here ? 'text-shodh-text font-medium' : 'text-shodh-text-muted')}>
                    {done && <Check className="inline w-3 h-3 mr-1 -mt-0.5" aria-hidden="true" />}
                    {s.title}
                    <span className="sr-only">{done ? ' (done)' : here ? ' (current)' : ''}</span>
                  </span>
                </button>
              </li>
            );
          })}
        </ol>
      </nav>

      <form onSubmit={submit} aria-labelledby="telegram-setup-step" className="flex flex-col gap-4 rounded-xl border border-shodh-border bg-shodh-surface p-5">
        <div className="flex flex-col gap-1">
          <h3 id="telegram-setup-step" ref={headingRef} tabIndex={-1} className="m-0 text-[16px] font-semibold text-shodh-text focus:outline-none">
            {current.title}
          </h3>
          <p className="m-0 text-[13px] text-shodh-text-secondary max-w-[64ch]">{current.detail}</p>
        </div>

        {step === 1 && (
          <div className="flex flex-col gap-3">
            <div className="flex flex-wrap items-center gap-2">
              <button type="button" className={PRIMARY_BUTTON} disabled={localOnly} onClick={openBotFather}>
                <ExternalLink className="w-4 h-4" aria-hidden="true" /> Open @BotFather
              </button>
              <button type="button" className={OUTLINE_BUTTON} disabled={localOnly} onClick={() => void copyLink()}>
                {copied ? <Check className="w-3.5 h-3.5 text-shodh-success" aria-hidden="true" /> : <Copy className="w-3.5 h-3.5" aria-hidden="true" />}
                {copied ? 'Link copied' : 'Copy link'}
              </button>
              <span aria-live="polite" className="sr-only">{copied ? 'The link to @BotFather was copied.' : ''}</span>
            </div>
            <ol className="m-0 pl-5 list-decimal flex flex-col gap-1 text-[12.5px] text-shodh-text-secondary max-w-[64ch]">
              <li>Send <code className="font-mono text-shodh-text">/newbot</code> to @BotFather.</li>
              <li>Name it (for example “Shodh”), then choose a username ending in “bot”.</li>
              <li>Keep the reply open: it holds the token for the next step.</li>
            </ol>
            <p className="m-0 text-[12px] text-shodh-text-muted">If Telegram opens behind Shodh, copy the link and open it in Telegram.</p>
          </div>
        )}

        {step === 2 && (
          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5 max-w-[480px]">
              <label htmlFor={tokenId} className="text-[12.5px] font-semibold text-shodh-text">Bot token</label>
              <div className="relative">
                <input
                  id={tokenId}
                  type="password"
                  autoComplete="off"
                  spellCheck={false}
                  autoFocus
                  value={wizard.token}
                  placeholder="123456789:AAH…"
                  disabled={busy || localOnly}
                  aria-invalid={check.problem !== null}
                  aria-describedby={hintId}
                  onChange={e => onWizard({ ...wizard, token: e.target.value, error: null })}
                  className={cn(INPUT, 'font-mono pr-9', check.problem && 'border-shodh-error')}
                />
                {check.ok && <Check className="absolute right-3 top-1/2 -translate-y-1/2 w-4 h-4 text-shodh-success" aria-label="Looks right" />}
              </div>
              {check.problem ? (
                <p id={hintId} role="alert" className="m-0 text-[12px] text-shodh-error">{check.problem}</p>
              ) : (
                <p id={hintId} className="m-0 text-[12px] text-shodh-text-muted">The bot’s number, a colon, then 35 characters.</p>
              )}
            </div>
            <Callout tone="neutral">
              <LearnMore summary="Telegram bot chats are not end-to-end encrypted.">
                <p className="m-0">{status.privacyNote}</p>
                <p className="m-0 mt-2">{TELEGRAM_MODEL_NOTE}</p>
              </LearnMore>
            </Callout>
            {wizard.error && (
              <Callout tone="error" role="alert">
                <p className="m-0 text-shodh-error font-medium">Telegram did not accept the token.</p>
                <p className="m-0 mt-0.5 break-words">{wizard.error}</p>
              </Callout>
            )}
          </div>
        )}

        {step === 3 && status.bot && (
          <LinkCard kind="owner" botUsername={status.bot.username} disabled={localOnly} />
        )}

        <div className="flex items-center gap-2 pt-1">
          {step === 2 && (
            <button type="button" className={QUIET_BUTTON} disabled={busy} onClick={() => go(1)}>
              Back
            </button>
          )}
          {step === 3 ? (
            <>
              <p className="m-0 text-[12px] text-shodh-text-muted" role="status" aria-live="polite">
                Waiting for you to press Start in Telegram…
              </p>
              <button type="button" className={cn(QUIET_BUTTON, 'ml-auto')} onClick={onLater}>
                Link later
              </button>
            </>
          ) : (
            <button type="submit" className={cn(PRIMARY_BUTTON, 'ml-auto')} disabled={busy || localOnly || (step === 2 && !check.ok)}>
              {busy && <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
              {step === 2 ? (busy ? 'Checking with Telegram…' : 'Connect') : 'I have the token'}
            </button>
          )}
        </div>
      </form>
    </div>
  );
}

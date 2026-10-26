import React, { useEffect, useId, useRef, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Check, ChevronDown, Copy, ExternalLink, Loader2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { slackApi, slackError } from './api';
import {
  MODEL_PRIVACY_NOTE,
  WIZARD_STEPS,
  canLeaveStep,
  checkAppToken,
  checkBotToken,
  reachableStep,
  scopeWords,
  stepToFix,
  tokenHint,
  wizardSteps,
  type SlackStatus,
  type WizardStep,
} from './slackModel';
import { Callout, FOCUS_RING, INPUT, LearnMore, OUTLINE_BUTTON, PRIMARY_BUTTON, QUIET_BUTTON } from './ui';

/** The wizard's place and what was typed: kept in memory while the view is open, never saved. */
export interface WizardState {
  step: WizardStep;
  appToken: string;
  botToken: string;
  /** What Slack refused on Connect, and the step that fixes it. */
  error: string | null;
  errorStep: WizardStep | null;
}

export const EMPTY_WIZARD: WizardState = { step: 1, appToken: '', botToken: '', error: null, errorStep: null };

function TokenField({
  label,
  prefix,
  value,
  onChange,
  check,
  disabled,
  autoFocus,
}: {
  label: string;
  prefix: string;
  value: string;
  onChange: (value: string) => void;
  check: typeof checkAppToken;
  disabled: boolean;
  autoFocus?: boolean;
}) {
  const id = useId();
  const hintId = useId();
  const helpId = useId();
  const hint = tokenHint(value, check);
  const ok = value.trim() !== '' && hint === null;
  return (
    <div className="flex flex-col gap-1.5 max-w-[480px]">
      <label htmlFor={id} className="text-[12.5px] font-semibold text-shodh-text">{label}</label>
      <div className="relative">
        <input
          id={id}
          type="password"
          autoComplete="off"
          spellCheck={false}
          autoFocus={autoFocus}
          value={value}
          placeholder={`${prefix}…`}
          disabled={disabled}
          aria-invalid={hint !== null}
          aria-describedby={hint ? hintId : helpId}
          onChange={e => onChange(e.target.value)}
          className={cn(INPUT, 'font-mono pr-9', hint && 'border-shodh-error')}
        />
        {ok && <Check className="absolute right-3 top-1/2 -translate-y-1/2 w-4 h-4 text-shodh-success" aria-label="Looks right" />}
      </div>
      {hint ? (
        <p id={hintId} role="alert" className="m-0 text-[12px] text-shodh-error">{hint}</p>
      ) : (
        <p id={helpId} className="m-0 text-[12px] text-shodh-text-muted">Starts with {prefix}</p>
      )}
    </div>
  );
}

/** "What Shodh will be allowed to do": the permissions in plain words, their ids below. */
function Permissions({ status }: { status: SlackStatus }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  return (
    <div className="rounded-lg border border-shodh-border-subtle bg-shodh-surface-2">
      <button
        type="button"
        aria-expanded={open}
        aria-controls={id}
        onClick={() => setOpen(o => !o)}
        className={cn('w-full flex items-center gap-2 px-3.5 h-10 text-left text-[12.5px] font-medium text-shodh-text-secondary rounded-lg hover:text-shodh-text', FOCUS_RING)}
      >
        What Shodh will be allowed to do
        <ChevronDown className={cn('ml-auto w-3.5 h-3.5 transition-transform duration-micro motion-reduce:transition-none', open && 'rotate-180')} aria-hidden="true" />
      </button>
      <div id={id} hidden={!open} className="px-3.5 pb-3 flex flex-col gap-2">
        <p className="m-0 text-[12px] text-shodh-text-muted">
          No public URL: Shodh connects out from this computer (Socket Mode). It never reads a channel it is not added to.
        </p>
        <ul className="m-0 pl-4 list-disc flex flex-col gap-0.5 text-[12.5px] text-shodh-text-secondary">
          {status.botScopes.map(scope => (
            <li key={scope}>{scopeWords(scope)}</li>
          ))}
        </ul>
        <p className="m-0 text-[11.5px] text-shodh-text-faint break-words">Slack scopes: {status.botScopes.join(', ')}</p>
      </div>
    </div>
  );
}

/**
 * Connecting Slack, one step at a time: create the app from Shodh's manifest, paste the
 * app-level token, install and paste the bot token, connect. The tokens stay in memory
 * (in `wizard`) until Connect sends them; nothing is saved before Slack accepts them.
 */
export function SlackSetup({
  status,
  wizard,
  onWizard,
  onConnected,
}: {
  status: SlackStatus;
  wizard: WizardState;
  onWizard: (next: WizardState) => void;
  onConnected: (status: SlackStatus) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const firstRender = useRef(true);
  const steps = wizardSteps(status);
  const localOnly = status.state === 'local_only';
  const step = reachableStep(wizard.step, wizard.appToken, wizard.botToken);
  const current = steps[step - 1];
  const canLeave = canLeaveStep(step, wizard.appToken, wizard.botToken);

  // A new step takes focus to its heading, except where its token field takes it.
  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false;
      return;
    }
    if (step === 1 || step === 4) headingRef.current?.focus();
  }, [step]);

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 2000);
    return () => window.clearTimeout(timer);
  }, [copied]);

  const go = (next: WizardStep) => onWizard({ ...wizard, step: next });

  // Typing clears what Slack said about the last try.
  const type = (field: 'appToken' | 'botToken', value: string) => onWizard({ ...wizard, [field]: value, error: null, errorStep: null });

  const openSlack = () =>
    void openUrl(status.createAppUrl).catch(err => notify.error('Slack could not be opened', { description: slackError(err) }));

  const copyLink = async () => {
    try {
      await navigator.clipboard.writeText(status.createAppUrl);
      setCopied(true);
    } catch (err) {
      notify.error('The link could not be copied', { description: slackError(err) });
    }
  };

  const connect = async () => {
    if (!canLeaveStep(4, wizard.appToken, wizard.botToken)) return;
    setBusy(true);
    onWizard({ ...wizard, error: null, errorStep: null });
    try {
      const next = await slackApi.connect(wizard.appToken, wizard.botToken);
      onWizard(EMPTY_WIZARD);
      onConnected(next);
      notify.success('Slack connected', {
        description: next.installation ? `Shodh is in ${next.installation.teamName} as @${next.installation.botName}.` : undefined,
      });
    } catch (err) {
      const message = slackError(err);
      onWizard({ ...wizard, error: message, errorStep: stepToFix(message) });
    } finally {
      setBusy(false);
    }
  };

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!canLeave || busy || localOnly) return;
    if (step === 4) void connect();
    else go((step + 1) as WizardStep);
  };

  return (
    <div className="flex flex-col gap-5">
      {localOnly && (
        <Callout tone="neutral" role="status">
          Local-only mode is on, so Shodh does not connect to Slack. Turn it off in Settings → Privacy to set Slack up.
        </Callout>
      )}

      <nav aria-label="Setup progress" className="flex flex-col gap-2">
        <p aria-live="polite" className="m-0 text-[12px] font-medium text-shodh-text-muted">
          Step {step} of {WIZARD_STEPS.length}
        </p>
        <ol className="m-0 p-0 list-none grid grid-cols-4 gap-2">
          {steps.map(s => {
            const done = s.step < step;
            const here = s.step === step;
            const broken = wizard.errorStep === s.step && wizard.error !== null;
            const reachable = reachableStep(s.step, wizard.appToken, wizard.botToken) === s.step;
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
                  <span
                    className={cn(
                      'hidden sm:block truncate text-[11.5px]',
                      broken ? 'text-shodh-error font-medium' : here ? 'text-shodh-text font-medium' : 'text-shodh-text-muted',
                    )}
                  >
                    {done && !broken && <Check className="inline w-3 h-3 mr-1 -mt-0.5" aria-hidden="true" />}
                    {s.title}
                    <span className="sr-only">{done ? ' (done)' : here ? ' (current)' : ''}{broken ? ' (needs fixing)' : ''}</span>
                  </span>
                </button>
              </li>
            );
          })}
        </ol>
      </nav>

      <form
        onSubmit={submit}
        aria-labelledby="slack-setup-step"
        className="flex flex-col gap-4 rounded-xl border border-shodh-border bg-shodh-surface p-5"
      >
        <div className="flex flex-col gap-1">
          <h3 id="slack-setup-step" ref={headingRef} tabIndex={-1} className="m-0 text-[16px] font-semibold text-shodh-text focus:outline-none">
            {current.title}
          </h3>
          <p className="m-0 text-[13px] text-shodh-text-secondary max-w-[64ch]">{current.detail}</p>
        </div>

        {step === 1 && (
          <div className="flex flex-col gap-4">
            <div className="flex flex-wrap items-center gap-2">
              <button type="button" className={PRIMARY_BUTTON} disabled={localOnly} onClick={openSlack}>
                <ExternalLink className="w-4 h-4" aria-hidden="true" /> Open Slack
              </button>
              <button type="button" className={OUTLINE_BUTTON} disabled={localOnly} onClick={() => void copyLink()}>
                {copied ? <Check className="w-3.5 h-3.5 text-shodh-success" aria-hidden="true" /> : <Copy className="w-3.5 h-3.5" aria-hidden="true" />}
                {copied ? 'Link copied' : 'Copy link'}
              </button>
              <span aria-live="polite" className="sr-only">{copied ? 'The link to create the Slack app was copied.' : ''}</span>
            </div>
            <p className="m-0 text-[12px] text-shodh-text-muted">
              If your browser opens behind Shodh, copy the link and paste it into the browser.
            </p>
            <Permissions status={status} />
          </div>
        )}

        {step === 2 && (
          <TokenField
            label="App-level token"
            prefix="xapp-"
            value={wizard.appToken}
            onChange={v => type('appToken', v)}
            check={checkAppToken}
            disabled={busy || localOnly}
            autoFocus
          />
        )}

        {step === 3 && (
          <TokenField
            label="Bot token"
            prefix="xoxb-"
            value={wizard.botToken}
            onChange={v => type('botToken', v)}
            check={checkBotToken}
            disabled={busy || localOnly}
            autoFocus
          />
        )}

        {step === 4 && (
          <div className="flex flex-col gap-4">
            <dl className="m-0 grid grid-cols-[auto_1fr_auto] items-center gap-x-4 gap-y-2 text-[12.5px] max-w-[480px]">
              <dt className="text-shodh-text-secondary">App-level token</dt>
              <dd className="m-0 inline-flex items-center gap-1.5 text-shodh-text">
                <Check className="w-3.5 h-3.5 text-shodh-success" aria-hidden="true" /> Pasted
              </dd>
              <dd className="m-0">
                <button type="button" className={QUIET_BUTTON} disabled={busy} onClick={() => go(2)} aria-label="Change the app-level token">Change</button>
              </dd>
              <dt className="text-shodh-text-secondary">Bot token</dt>
              <dd className="m-0 inline-flex items-center gap-1.5 text-shodh-text">
                <Check className="w-3.5 h-3.5 text-shodh-success" aria-hidden="true" /> Pasted
              </dd>
              <dd className="m-0">
                <button type="button" className={QUIET_BUTTON} disabled={busy} onClick={() => go(3)} aria-label="Change the bot token">Change</button>
              </dd>
            </dl>
            <Callout tone="neutral">
              <LearnMore summary="Questions from Slack go to the model you chose in Settings → Model.">{MODEL_PRIVACY_NOTE}</LearnMore>
            </Callout>
          </div>
        )}

        {wizard.error && (
          <Callout tone="error" role="alert">
            <p className="m-0 text-shodh-error font-medium">Slack did not accept the connection.</p>
            <p className="m-0 mt-0.5 break-words">{wizard.error}</p>
            {wizard.errorStep !== null && wizard.errorStep !== step && (
              <button type="button" className={cn(QUIET_BUTTON, '-ml-2.5 mt-1')} onClick={() => go(wizard.errorStep as WizardStep)}>
                Go to step {wizard.errorStep}: {steps[wizard.errorStep - 1].title}
              </button>
            )}
          </Callout>
        )}

        <div className="flex items-center gap-2 pt-1">
          {step > 1 && (
            <button type="button" className={QUIET_BUTTON} disabled={busy} onClick={() => go((step - 1) as WizardStep)}>
              Back
            </button>
          )}
          <button type="submit" className={cn(PRIMARY_BUTTON, 'ml-auto')} disabled={!canLeave || busy || localOnly}>
            {busy && <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
            {step === 4 ? (busy ? 'Checking with Slack…' : 'Connect') : step === 1 ? 'I created the app' : 'Next'}
          </button>
        </div>
      </form>
    </div>
  );
}

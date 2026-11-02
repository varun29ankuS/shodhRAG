import React, { useCallback, useEffect, useId, useRef, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Check, Copy, ExternalLink, Loader2, Lock, RefreshCw, Search } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { slackError } from './api';
import { githubApi } from './githubApi';
import {
  GITHUB_STEPS,
  HARD_LIMITS,
  LEVEL_TEXT,
  WINDOWS_COMMANDS_NOTE,
  codeCountdown,
  filterRepos,
  githubStep,
  githubSteps,
  levelLabel,
  nextGrant,
  type AvailableRepo,
  type GitHubRepoView,
  type GitHubStatus,
  type GitHubStep,
  type Level,
} from './githubModel';
import { SwitchRow } from '../../components/PrivacySettings';
import { Callout, FOCUS_RING, INPUT, LearnMore, OUTLINE_BUTTON, PRIMARY_BUTTON, QUIET_BUTTON } from './ui';

/** The wizard's place: in memory while the view is open, never saved. */
export interface GitHubWizardState {
  step: GitHubStep;
  /** The user is going through the setup (it stays open after sign-in until Done). */
  active: boolean;
  /** Repositories ticked on step 2, by `owner/name`. */
  picked: string[];
}

export const EMPTY_GITHUB_WIZARD: GitHubWizardState = { step: 1, active: false, picked: [] };

function openLink(url: string) {
  void openUrl(url).catch(err => notify.error('The page could not be opened', { description: slackError(err) }));
}

/** The repositories the app is installed on, loaded on demand. */
export function useAvailableRepos(enabled: boolean) {
  const [repos, setRepos] = useState<AvailableRepo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const load = useCallback(async () => {
    setLoading(true);
    try {
      setRepos(await githubApi.availableRepos());
      setError(null);
    } catch (err) {
      setError(slackError(err));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    if (enabled) void load();
  }, [enabled, load]);
  return { repos, error, loading, reload: load };
}

/** The code to type on GitHub, big, with copy and open. */
export function SignInCode({ status, now }: { status: GitHubStatus; now: number }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 2000);
    return () => window.clearTimeout(timer);
  }, [copied]);
  const signIn = status.signIn;
  if (!signIn) return null;
  const left = codeCountdown(signIn, now);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(signIn.userCode);
      setCopied(true);
    } catch (err) {
      notify.error('The code could not be copied', { description: slackError(err) });
    }
  };
  return (
    <div className="flex flex-col gap-3 rounded-lg border border-shodh-border bg-shodh-surface-2 p-4">
      <p className="m-0 text-small text-shodh-text-secondary">Type this code on GitHub:</p>
      <p className="m-0 font-mono text-[28px] font-semibold tracking-[0.2em] text-shodh-text select-all" aria-label={`Code ${signIn.userCode.split('').join(' ')}`}>
        {signIn.userCode}
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <button type="button" className={PRIMARY_BUTTON} autoFocus onClick={() => openLink(signIn.verificationUri)}>
          <ExternalLink className="w-4 h-4" aria-hidden="true" /> Open github.com/login/device
        </button>
        <button type="button" className={OUTLINE_BUTTON} onClick={() => void copy()}>
          {copied ? <Check className="w-3.5 h-3.5 text-shodh-success" aria-hidden="true" /> : <Copy className="w-3.5 h-3.5" aria-hidden="true" />}
          {copied ? 'Code copied' : 'Copy code'}
        </button>
        <span aria-live="polite" className="sr-only">{copied ? 'The code was copied.' : ''}</span>
      </div>
      <p className="m-0 text-caption text-shodh-text-muted" role="status" aria-live="polite">
        <Loader2 className="inline w-3.5 h-3.5 mr-1.5 -mt-0.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        {left ? `Waiting for you on GitHub. The code works for ${left} more.` : 'The code expired. Start again.'}
      </p>
    </div>
  );
}

/** One repository's permissions: read or read and write, and merge. */
export function RepoPermissions({
  repo,
  busy,
  onChange,
}: {
  repo: GitHubRepoView;
  busy: boolean;
  onChange: (grant: { write: boolean; merge: boolean }) => void;
}) {
  const name = useId();
  return (
    <div className="flex flex-col gap-3">
      <fieldset className="m-0 p-0 border-0 flex flex-col gap-2">
        <legend className="mb-1.5 text-small font-semibold text-shodh-text">{repo.fullName}</legend>
        {(['read', 'write'] as Level[]).map(level => (
          <label key={level} className={cn('flex items-start gap-2.5 rounded-lg border px-3 py-2.5 cursor-pointer', (level === 'write') === repo.write ? 'border-shodh-accent bg-shodh-raised' : 'border-shodh-border')}>
            <input
              type="radio"
              name={name}
              className="mt-1"
              checked={(level === 'write') === repo.write}
              disabled={busy}
              onChange={() => onChange(nextGrant(repo, { level }))}
            />
            <span className="min-w-0">
              <span className="block text-small font-medium text-shodh-text">{level === 'write' ? 'Read and write' : 'Read only'}</span>
              <span className="block text-caption text-shodh-text-muted">{LEVEL_TEXT[level]}</span>
            </span>
          </label>
        ))}
      </fieldset>
      <SwitchRow
        label="Merge pull requests"
        description={repo.write ? LEVEL_TEXT.merge : 'Needs Read and write first.'}
        checked={repo.write && repo.merge}
        disabled={busy || !repo.write}
        onChange={merge => onChange(nextGrant(repo, { merge }))}
      />
    </div>
  );
}

/** The picker: the repositories the app is installed on, with a search. */
export function RepoPicker({
  repos,
  error,
  loading,
  installUrl,
  picked,
  onPick,
  onReload,
  disabled,
}: {
  repos: AvailableRepo[] | null;
  error: string | null;
  loading: boolean;
  installUrl: string;
  picked: readonly string[];
  onPick: (fullName: string, on: boolean) => void;
  onReload: () => void;
  disabled: boolean;
}) {
  const [query, setQuery] = useState('');
  const searchId = useId();
  const shown = repos ? filterRepos(repos, query) : [];
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <div className="relative flex-1 min-w-[220px] max-w-[420px]">
          <label htmlFor={searchId} className="sr-only">Search repositories</label>
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-shodh-text-muted" aria-hidden="true" />
          <input
            id={searchId}
            type="search"
            value={query}
            placeholder="Search by owner or name"
            onChange={e => setQuery(e.target.value)}
            className={cn(INPUT, 'pl-8')}
          />
        </div>
        <button type="button" className={QUIET_BUTTON} disabled={loading} onClick={onReload}>
          <RefreshCw className={cn('w-3.5 h-3.5', loading && 'animate-spin motion-reduce:animate-none')} aria-hidden="true" /> Refresh
        </button>
        <button type="button" className={QUIET_BUTTON} onClick={() => openLink(installUrl)}>
          <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" /> Install on more repositories
        </button>
      </div>
      {error && (
        <Callout tone="error" role="alert">
          <p className="m-0">The repositories could not be loaded: {error}</p>
        </Callout>
      )}
      {repos === null && !error ? (
        <p className="m-0 text-small text-shodh-text-muted" role="status">Loading the repositories the Shodh app is installed on…</p>
      ) : repos && repos.length === 0 ? (
        <Callout tone="warning">
          <p className="m-0">The Shodh app is not installed on any of your repositories yet.</p>
          <button type="button" className={cn(OUTLINE_BUTTON, 'mt-2')} onClick={() => openLink(installUrl)}>
            <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" /> Install the Shodh app on GitHub
          </button>
          <p className="m-0 mt-2 text-caption text-shodh-text-muted">Choose the repositories there, then press Refresh here.</p>
        </Callout>
      ) : (
        <ul aria-label="Repositories" className="m-0 p-0 list-none flex flex-col max-h-80 overflow-y-auto scrollbar-thin rounded-lg border border-shodh-border divide-y divide-shodh-border-subtle">
          {shown.map(r => {
            const on = r.onList || picked.includes(r.fullName);
            return (
              <li key={r.fullName}>
                <label className={cn('flex items-center gap-3 px-3 py-2.5 cursor-pointer hover:bg-shodh-raised', r.onList && 'cursor-default')}>
                  <input
                    type="checkbox"
                    checked={on}
                    disabled={disabled || r.onList}
                    onChange={e => onPick(r.fullName, e.target.checked)}
                    className={FOCUS_RING}
                  />
                  <span className="min-w-0 flex-1 text-small text-shodh-text truncate">{r.fullName}</span>
                  {r.private && (
                    <span className="inline-flex items-center gap-1 text-caption text-shodh-text-muted">
                      <Lock className="w-3 h-3" aria-hidden="true" /> Private
                    </span>
                  )}
                  {r.onList && <span className="text-caption text-shodh-text-muted">On your list</span>}
                </label>
              </li>
            );
          })}
          {shown.length === 0 && <li className="px-3 py-2.5 text-small text-shodh-text-muted">No repository matches “{query}”.</li>}
        </ul>
      )}
    </div>
  );
}

/**
 * Setting GitHub up, one step at a time: sign in with a code on github.com, choose the
 * repositories Shodh may reach, decide what it may do on each.
 */
export function GitHubSetup({
  status,
  wizard,
  onWizard,
  onStatus,
  now,
  onDone,
}: {
  status: GitHubStatus;
  wizard: GitHubWizardState;
  onWizard: (next: GitHubWizardState) => void;
  onStatus: (status: GitHubStatus) => void;
  now: number;
  onDone: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const firstRender = useRef(true);
  const steps = githubSteps();
  const step = githubStep(status, wizard.step);
  const current = steps[step - 1];
  const localOnly = status.state === 'local_only';
  const available = useAvailableRepos(step === 2 && status.account !== null);

  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false;
      return;
    }
    headingRef.current?.focus();
  }, [step]);

  const run = async (action: () => Promise<GitHubStatus>, failure: string): Promise<boolean> => {
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

  const signIn = () => {
    onWizard({ ...wizard, active: true });
    void run(() => githubApi.signInStart(), 'Sign-in could not start');
  };

  const addPicked = async () => {
    setBusy(true);
    try {
      let latest: GitHubStatus | null = null;
      for (const fullName of wizard.picked) {
        const [owner, name] = fullName.split('/');
        latest = await githubApi.setRepo(owner, name, false, false);
      }
      if (latest) onStatus(latest);
      onWizard({ ...wizard, step: 3, picked: [] });
    } catch (err) {
      notify.error('A repository could not be added', { description: slackError(err) });
    } finally {
      setBusy(false);
    }
  };

  const setGrant = (repo: GitHubRepoView, grant: { write: boolean; merge: boolean }) =>
    void run(() => githubApi.setRepo(repo.owner, repo.name, grant.write, grant.merge), `${repo.fullName} could not be changed`);

  return (
    <div className="flex flex-col gap-5">
      {localOnly && (
        <Callout tone="neutral" role="status">
          Local-only mode is on, so Shodh does not connect to GitHub. Turn it off in Settings → Privacy to set GitHub up.
        </Callout>
      )}

      <nav aria-label="Setup progress" className="flex flex-col gap-2">
        <p aria-live="polite" className="m-0 text-caption font-medium text-shodh-text-muted">
          Step {step} of {GITHUB_STEPS.length}
        </p>
        <ol className="m-0 p-0 list-none grid grid-cols-3 gap-2">
          {steps.map(s => {
            const done = s.step < step;
            const here = s.step === step;
            const reachable = status.account !== null && s.step > 1;
            return (
              <li key={s.step} className="min-w-0">
                <button
                  type="button"
                  disabled={!reachable || busy || here}
                  aria-current={here ? 'step' : undefined}
                  onClick={() => onWizard({ ...wizard, step: s.step })}
                  className={cn('w-full flex flex-col gap-1.5 text-left rounded-md disabled:cursor-default', FOCUS_RING)}
                >
                  <span aria-hidden="true" className={cn('block h-1 rounded-full', here || done ? 'bg-shodh-accent' : 'bg-shodh-raised-2')} />
                  <span className={cn('hidden sm:block truncate text-caption', here ? 'text-shodh-text font-medium' : 'text-shodh-text-muted')}>
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

      <section aria-labelledby="github-setup-step" className="flex flex-col gap-4 rounded-xl border border-shodh-border bg-shodh-surface p-5">
        <div className="flex flex-col gap-1">
          <h3 id="github-setup-step" ref={headingRef} tabIndex={-1} className="m-0 text-[16px] font-semibold text-shodh-text focus:outline-none">
            {current.title}
          </h3>
          <p className="m-0 text-small text-shodh-text-secondary max-w-[64ch]">{current.detail}</p>
        </div>

        {step === 1 && (
          <div className="flex flex-col gap-4">
            {status.signIn ? (
              <SignInCode status={status} now={now} />
            ) : (
              <div className="flex flex-wrap items-center gap-2">
                <button type="button" className={PRIMARY_BUTTON} disabled={busy || localOnly} onClick={signIn}>
                  {busy && <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
                  Sign in with GitHub
                </button>
              </div>
            )}
            {status.signInError && !status.signIn && (
              <Callout tone="error" role="alert">
                <p className="m-0 text-shodh-error font-medium">Sign-in did not finish.</p>
                <p className="m-0 mt-0.5 break-words">{status.signInError}</p>
              </Callout>
            )}
            {status.commandsCanReadSignIn && (
              <Callout tone="warning" role="status">
                <p className="m-0">{WINDOWS_COMMANDS_NOTE}</p>
              </Callout>
            )}
            <Callout tone="neutral">
              <LearnMore summary="What the Shodh GitHub app can and cannot do.">
                <p className="m-0">{status.permissionsNote}</p>
                <ul className="m-0 mt-2 pl-5 list-disc flex flex-col gap-0.5">
                  {HARD_LIMITS.map(limit => <li key={limit}>{limit}</li>)}
                </ul>
              </LearnMore>
            </Callout>
          </div>
        )}

        {step === 2 && (
          <RepoPicker
            repos={available.repos}
            error={available.error}
            loading={available.loading}
            installUrl={status.installUrl}
            picked={wizard.picked}
            disabled={busy}
            onReload={() => void available.reload()}
            onPick={(fullName, on) =>
              onWizard({ ...wizard, picked: on ? [...wizard.picked, fullName] : wizard.picked.filter(p => p !== fullName) })
            }
          />
        )}

        {step === 3 && (
          <div className="flex flex-col gap-5">
            {status.repos.length === 0 ? (
              <p className="m-0 text-small text-shodh-text-muted">No repositories on your list yet. Go back and choose some, or do it later under Repositories.</p>
            ) : (
              status.repos.map(repo => (
                <div key={repo.fullName} className="flex flex-col gap-1 rounded-lg border border-shodh-border-subtle p-4">
                  <RepoPermissions repo={repo} busy={busy} onChange={grant => setGrant(repo, grant)} />
                  <p className="m-0 mt-1 text-caption text-shodh-text-muted">Now: {levelLabel(repo)}.</p>
                </div>
              ))
            )}
          </div>
        )}

        <div className="flex items-center gap-2 pt-1">
          {step === 1 && status.signIn && (
            <button type="button" className={QUIET_BUTTON} disabled={busy} onClick={() => void run(() => githubApi.signInCancel(), 'Sign-in could not be stopped')}>
              Cancel sign-in
            </button>
          )}
          {step === 2 && (
            <>
              <button type="button" className={QUIET_BUTTON} disabled={busy} onClick={() => onWizard({ ...wizard, step: 3 })}>
                Skip
              </button>
              <button type="button" className={cn(PRIMARY_BUTTON, 'ml-auto')} disabled={busy || wizard.picked.length === 0} onClick={() => void addPicked()}>
                {busy && <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
                {wizard.picked.length === 1 ? 'Add 1 repository' : `Add ${wizard.picked.length} repositories`}
              </button>
            </>
          )}
          {step === 3 && (
            <>
              <button type="button" className={QUIET_BUTTON} disabled={busy} onClick={() => onWizard({ ...wizard, step: 2 })}>
                Back
              </button>
              <button type="button" className={cn(PRIMARY_BUTTON, 'ml-auto')} disabled={busy} onClick={onDone}>
                Done
              </button>
            </>
          )}
        </div>
      </section>
    </div>
  );
}

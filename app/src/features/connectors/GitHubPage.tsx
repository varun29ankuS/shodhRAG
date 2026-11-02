import React, { useCallback, useEffect, useRef, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { ArrowLeft, CheckCircle2, ChevronRight, ExternalLink, LogOut, Plus, RefreshCw, Trash2, XCircle } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { StaleNotice } from '../../components/ui/StaleNotice';
import { slackError } from './api';
import { githubApi } from './githubApi';
import {
  CODE_MODE_NOTE,
  HARD_LIMITS,
  LEVEL_TEXT,
  REVOKE_NOTE,
  activityTime,
  githubNextSteps,
  githubPill,
  githubStateLine,
  levelLabel,
  reposLine,
  type GitHubActivityItem,
  type GitHubRepoView,
  type GitHubStatus,
  type GitHubTabId,
} from './githubModel';
import { GITHUB_TABS, GITHUB_TAB_LABELS, moveTab } from './navigation';
import { stalePill } from './slackModel';
import { GitHubMark } from './GitHubLogo';
import { GitHubSetup, RepoPermissions, RepoPicker, SignInCode, useAvailableRepos, type GitHubWizardState } from './GitHubSetup';
import { Callout, DANGER_BUTTON, OUTLINE_BUTTON, PRIMARY_BUTTON, QUIET_BUTTON, Section, Skeleton, StatusPill, TABLIST, TONE_DOT, TONE_TEXT, tabClass } from './ui';
import type { GitHubStatusState } from './useGitHubStatus';

function openLink(url: string) {
  void openUrl(url).catch(err => notify.error('The page could not be opened', { description: slackError(err) }));
}

interface TabProps {
  status: GitHubStatus;
  onStatus: (status: GitHubStatus) => void;
  busy: boolean;
  setBusy: (busy: boolean) => void;
  now: number;
}

/** Runs a command that returns the new status; a failure is a toast. */
function useRun({ onStatus, setBusy }: Pick<TabProps, 'onStatus' | 'setBusy'>) {
  return async (action: () => Promise<GitHubStatus>, failure: string): Promise<boolean> => {
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

function Panel({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5 rounded-lg border border-shodh-border-subtle bg-shodh-surface p-4 min-w-0">
      <p className="m-0 text-caption font-medium text-shodh-text-muted">{title}</p>
      {children}
    </div>
  );
}

// ── Overview ────────────────────────────────────────────────────────

function OverviewTab({ status, now, onTab, onSignIn }: TabProps & { onTab: (tab: GitHubTabId) => void; onSignIn: () => void }) {
  const line = githubStateLine(status, now);
  const steps = githubNextSteps(status, now);
  return (
    <div className="flex flex-col gap-6">
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
        <Panel title="Sign-in">
          <p className="m-0 flex items-center gap-2 text-body font-semibold text-shodh-text">
            <span aria-hidden="true" className={cn('w-2 h-2 rounded-full shrink-0', TONE_DOT[line.tone])} />
            <span className="min-w-0 break-words">{line.title}</span>
          </p>
          {line.detail && <p className={cn('m-0 text-small break-words', TONE_TEXT[line.tone])}>{line.detail}</p>}
          {status.account && (
            <button type="button" className={cn(QUIET_BUTTON, 'self-start -ml-2.5')} onClick={() => openLink(status.account!.profileUrl)}>
              <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" /> Open your GitHub profile
            </button>
          )}
        </Panel>
        <Panel title="Repositories">
          <p className="m-0 text-small text-shodh-text">{reposLine(status)}</p>
          <button type="button" className={cn(QUIET_BUTTON, 'self-start -ml-2.5')} onClick={() => onTab('repositories')}>
            Open Repositories
          </button>
        </Panel>
      </div>

      {steps.length > 0 && (
        <Section title="Next steps">
          <ul className="m-0 p-0 list-none flex flex-col gap-2">
            {steps.map(s => (
              <li key={s.id}>
                <Callout tone={s.tone}>
                  <div className="flex flex-wrap items-center gap-3">
                    <div className="min-w-0 flex-1">
                      <p className="m-0 font-medium text-shodh-text">{s.title}</p>
                      <p className="m-0 mt-0.5">{s.detail}</p>
                    </div>
                    {s.action && (
                      <button
                        type="button"
                        className={OUTLINE_BUTTON}
                        onClick={() => (s.id === 'sign-in' ? onSignIn() : s.tab && onTab(s.tab))}
                      >
                        {s.action}
                      </button>
                    )}
                  </div>
                </Callout>
              </li>
            ))}
          </ul>
        </Section>
      )}

      <Section title="What Shodh does on GitHub" description="Everything below needs the repository on your list. Every write waits for your yes, each time.">
        <ul className="m-0 pl-5 list-disc flex flex-col gap-1 text-small text-shodh-text-secondary max-w-[70ch]">
          <li>{LEVEL_TEXT.read}</li>
          <li>{LEVEL_TEXT.write}</li>
          <li>{LEVEL_TEXT.merge}</li>
          <li>{CODE_MODE_NOTE}</li>
        </ul>
      </Section>
    </div>
  );
}

// ── Repositories ────────────────────────────────────────────────────

function RepoRow({
  repo,
  busy,
  highlighted,
  onChange,
  onRemove,
}: {
  repo: GitHubRepoView;
  busy: boolean;
  highlighted: boolean;
  onChange: (grant: { write: boolean; merge: boolean }) => void;
  onRemove: () => void;
}) {
  const ref = useRef<HTMLLIElement>(null);
  useEffect(() => {
    if (highlighted) ref.current?.scrollIntoView({ block: 'center', behavior: 'smooth' });
  }, [highlighted]);
  return (
    <li
      ref={ref}
      data-repo={repo.fullName}
      className={cn(
        'flex flex-col gap-3 rounded-lg border p-4 transition-colors duration-micro',
        highlighted ? 'border-shodh-accent ring-2 ring-shodh-accent/40' : 'border-shodh-border-subtle',
      )}
    >
      <RepoPermissions repo={repo} busy={busy} onChange={onChange} />
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-caption text-shodh-text-muted flex-1 min-w-0">{levelLabel(repo)}</span>
        <button type="button" className={QUIET_BUTTON} onClick={() => openLink(repo.url)}>
          <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" /> Open on GitHub
        </button>
        <button type="button" className={DANGER_BUTTON} disabled={busy} onClick={onRemove}>
          <Trash2 className="w-3.5 h-3.5" aria-hidden="true" /> Remove
        </button>
      </div>
    </li>
  );
}

function RepositoriesTab({ status, busy, setBusy, onStatus, highlight }: TabProps & { highlight: string | null }) {
  const run = useRun({ onStatus, setBusy });
  const [adding, setAdding] = useState(false);
  const [picked, setPicked] = useState<string[]>([]);
  const available = useAvailableRepos(adding && status.account !== null);
  const signedIn = status.account !== null && status.state !== 'local_only';

  const add = async () => {
    setBusy(true);
    try {
      let latest: GitHubStatus | null = null;
      for (const fullName of picked) {
        const [owner, name] = fullName.split('/');
        latest = await githubApi.setRepo(owner, name, false, false);
      }
      if (latest) onStatus(latest);
      notify.success(picked.length === 1 ? `${picked[0]} added, read only` : `${picked.length} repositories added, read only`);
      setPicked([]);
      setAdding(false);
    } catch (err) {
      notify.error('A repository could not be added', { description: slackError(err) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <Section
        title="On your list"
        description="Shodh reaches only these repositories. Read is enough to fetch and read issues; write and merge always ask you first."
        actions={
          signedIn && !adding ? (
            <button type="button" className={OUTLINE_BUTTON} onClick={() => setAdding(true)}>
              <Plus className="w-3.5 h-3.5" aria-hidden="true" /> Add repositories
            </button>
          ) : undefined
        }
      >
        {status.repos.length === 0 ? (
          <p className="m-0 text-small text-shodh-text-muted">No repositories yet: Shodh can reach none.</p>
        ) : (
          <ul aria-label="Repositories on your list" className="m-0 p-0 list-none flex flex-col gap-3">
            {status.repos.map(repo => (
              <RepoRow
                key={repo.fullName}
                repo={repo}
                busy={busy}
                highlighted={highlight !== null && highlight.toLowerCase() === repo.fullName.toLowerCase()}
                onChange={grant => void run(() => githubApi.setRepo(repo.owner, repo.name, grant.write, grant.merge), `${repo.fullName} could not be changed`)}
                onRemove={() => void run(() => githubApi.removeRepo(repo.owner, repo.name), `${repo.fullName} could not be removed`)}
              />
            ))}
          </ul>
        )}
      </Section>

      {adding && (
        <Section title="Add repositories" description="These are the repositories the Shodh app is installed on. Added ones start read only.">
          <RepoPicker
            repos={available.repos}
            error={available.error}
            loading={available.loading}
            installUrl={status.installUrl}
            picked={picked}
            disabled={busy}
            onReload={() => void available.reload()}
            onPick={(fullName, on) => setPicked(p => (on ? [...p, fullName] : p.filter(x => x !== fullName)))}
          />
          <div className="flex items-center gap-2">
            <button type="button" className={QUIET_BUTTON} disabled={busy} onClick={() => { setAdding(false); setPicked([]); }}>
              Cancel
            </button>
            <button type="button" className={cn(PRIMARY_BUTTON, 'ml-auto')} disabled={busy || picked.length === 0} onClick={() => void add()}>
              {picked.length === 1 ? 'Add 1 repository' : `Add ${picked.length} repositories`}
            </button>
          </div>
        </Section>
      )}
    </div>
  );
}

// ── Activity ────────────────────────────────────────────────────────

function ActivityTab({ now }: TabProps) {
  const [items, setItems] = useState<GitHubActivityItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const load = useCallback(async () => {
    setLoading(true);
    try {
      setItems(await githubApi.activity(100));
      setError(null);
    } catch (err) {
      setError(slackError(err));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);
  return (
    <Section
      title="Activity"
      description="Everything Shodh did on GitHub and every change to this connector, newest first. The full record is in Activity → Audit log."
      actions={
        <button type="button" className={QUIET_BUTTON} disabled={loading} onClick={() => void load()}>
          <RefreshCw className={cn('w-3.5 h-3.5', loading && 'animate-spin motion-reduce:animate-none')} aria-hidden="true" /> Refresh
        </button>
      }
    >
      {error && (
        <Callout tone="error" role="alert">
          <p className="m-0">The activity could not be loaded: {error}</p>
        </Callout>
      )}
      {items === null && !error ? (
        <div className="flex flex-col gap-2" aria-busy="true" aria-label="Loading activity">
          <Skeleton className="h-10 w-full" />
          <Skeleton className="h-10 w-full" />
        </div>
      ) : items && items.length === 0 ? (
        <p className="m-0 text-small text-shodh-text-muted">Nothing yet.</p>
      ) : (
        <ul aria-label="GitHub activity" className="m-0 p-0 list-none flex flex-col rounded-lg border border-shodh-border-subtle divide-y divide-shodh-border-subtle">
          {(items ?? []).map((item, i) => (
            <li key={`${item.at}-${i}`} className="flex items-start gap-2.5 px-3 py-2.5">
              {item.ok ? (
                <CheckCircle2 className="w-4 h-4 mt-0.5 shrink-0 text-shodh-success" aria-label="Done" />
              ) : (
                <XCircle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-error" aria-label="Failed" />
              )}
              <span className="min-w-0 flex-1 text-small text-shodh-text break-words">{item.summary}</span>
              <span className="shrink-0 text-caption text-shodh-text-muted tabular-nums">{activityTime(item, now)}</span>
            </li>
          ))}
        </ul>
      )}
    </Section>
  );
}

// ── Settings ────────────────────────────────────────────────────────

function SettingsTab({ status, busy, onSignOut, onSignIn }: TabProps & { onSignOut: () => void; onSignIn: () => void }) {
  return (
    <div className="flex flex-col gap-6">
      <Section title="Your GitHub sign-in" description={status.account ? `Signed in as @${status.account.login}.` : 'Not signed in.'}>
        <div className="flex flex-wrap items-center gap-2">
          {status.state === 'needs_sign_in' && (
            <button type="button" className={PRIMARY_BUTTON} disabled={busy} onClick={onSignIn}>
              Sign in again
            </button>
          )}
          {status.account && (
            <button type="button" className={DANGER_BUTTON} disabled={busy} onClick={onSignOut}>
              <LogOut className="w-3.5 h-3.5" aria-hidden="true" /> Sign out
            </button>
          )}
          <button type="button" className={QUIET_BUTTON} onClick={() => openLink(status.authorizedAppsUrl)}>
            <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" /> Authorized GitHub Apps
          </button>
        </div>
        <p className="m-0 text-caption text-shodh-text-muted max-w-[68ch]">{REVOKE_NOTE}</p>
      </Section>
      <Section title="The Shodh GitHub app" description={status.permissionsNote}>
        <div className="flex flex-wrap items-center gap-2">
          <button type="button" className={OUTLINE_BUTTON} onClick={() => openLink(status.installUrl)}>
            <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" /> Choose where it is installed
          </button>
        </div>
      </Section>
      <Section title="Never, whatever you allow">
        <ul className="m-0 pl-5 list-disc flex flex-col gap-1 text-small text-shodh-text-secondary max-w-[70ch]">
          {HARD_LIMITS.map(limit => <li key={limit}>{limit}</li>)}
        </ul>
      </Section>
    </div>
  );
}

function SignOutConfirm({ status, busy, onCancel, onConfirm }: { status: GitHubStatus; busy: boolean; onCancel: () => void; onConfirm: () => void }) {
  return (
    <div role="alertdialog" aria-labelledby="github-sign-out-title" aria-describedby="github-sign-out-detail" className="flex flex-col gap-3 rounded-xl border border-shodh-error/40 bg-shodh-surface p-5">
      <h3 id="github-sign-out-title" className="m-0 text-body font-semibold text-shodh-text">
        Sign out of GitHub{status.account ? ` (@${status.account.login})` : ''}?
      </h3>
      <p id="github-sign-out-detail" className="m-0 text-small text-shodh-text-secondary max-w-[64ch]">
        Shodh stops all GitHub work and deletes the sign-in from this computer. Your list of repositories is kept for when
        you sign in again. {REVOKE_NOTE}
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <button type="button" className={DANGER_BUTTON} disabled={busy} onClick={onConfirm}>
          <LogOut className="w-3.5 h-3.5" aria-hidden="true" /> Sign out
        </button>
        <button type="button" className={QUIET_BUTTON} autoFocus disabled={busy} onClick={onCancel}>
          Keep signed in
        </button>
      </div>
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
      <span aria-current="page" className="text-shodh-text font-medium">GitHub</span>
    </nav>
  );
}

/**
 * The GitHub page: a guided setup (sign in with a code, choose repositories, decide what
 * Shodh may do), then its sections in tabs. The agent's GitHub actions open this page
 * with the repository they concern highlighted.
 */
export function GitHubPage({
  github,
  wizard,
  onWizard,
  tab,
  onTab,
  onBack,
  highlight,
}: {
  github: GitHubStatusState;
  wizard: GitHubWizardState;
  onWizard: (next: GitHubWizardState) => void;
  tab: GitHubTabId;
  onTab: (tab: GitHubTabId) => void;
  onBack: () => void;
  highlight: string | null;
}) {
  const { status, loadError, setStatus, retry, now } = github;
  const [busy, setBusy] = useState(false);
  const [confirmSignOut, setConfirmSignOut] = useState(false);
  const tabRefs = useRef<Partial<Record<GitHubTabId, HTMLButtonElement | null>>>({});

  const signIn = async () => {
    setBusy(true);
    try {
      setStatus(await githubApi.signInStart());
    } catch (err) {
      notify.error('Sign-in could not start', { description: slackError(err) });
    } finally {
      setBusy(false);
    }
  };

  const signOut = async () => {
    setBusy(true);
    try {
      setStatus(await githubApi.signOut());
      setConfirmSignOut(false);
      onWizard({ step: 1, active: false, picked: [] });
      notify.success('Signed out of GitHub', { description: 'The sign-in was deleted from this computer.' });
    } catch (err) {
      notify.error('Could not sign out of GitHub', { description: slackError(err) });
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
            <p className="m-0">The GitHub connector could not be loaded: {loadError}</p>
            <button type="button" className={cn(OUTLINE_BUTTON, 'mt-2')} onClick={retry}>Try again</button>
          </Callout>
        ) : (
          <div className="flex flex-col gap-5" aria-busy="true" aria-label="Loading GitHub">
            <div className="flex items-center gap-3">
              <GitHubMark size="lg" />
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

  const pill = loadError ? stalePill(githubPill(status, now)) : githubPill(status, now);
  const inSetup = !status.account || wizard.active;
  const tabProps = { status, onStatus: setStatus, busy, setBusy, now };

  return (
    <div className="flex flex-col gap-5">
      <Breadcrumb onBack={onBack} />
      {loadError && <StaleNotice error={loadError} onRetry={retry} />}

      <header className="flex items-start gap-3.5">
        <GitHubMark size="lg" />
        <div className="flex-1 min-w-0">
          <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
            <h2 className="m-0 text-[20px] font-semibold text-shodh-text">GitHub</h2>
            <span role="status" aria-live="polite">
              <StatusPill pill={pill} />
            </span>
          </div>
          <p className="m-0 mt-0.5 text-small text-shodh-text-secondary truncate">
            {status.account
              ? `@${status.account.login} · ${reposLine(status)}`
              : 'Shodh fetches, pushes its own branches and opens pull requests on the repositories you choose.'}
          </p>
        </div>
      </header>

      {confirmSignOut && (
        <SignOutConfirm status={status} busy={busy} onCancel={() => setConfirmSignOut(false)} onConfirm={() => void signOut()} />
      )}

      {status.signIn && !inSetup && <SignInCode status={status} now={now} />}

      {inSetup ? (
        <GitHubSetup
          status={status}
          wizard={wizard}
          onWizard={onWizard}
          onStatus={setStatus}
          now={now}
          onDone={() => {
            onWizard({ step: 1, active: false, picked: [] });
            onTab('overview');
          }}
        />
      ) : (
        <>
          <div role="tablist" aria-label="GitHub sections" className={TABLIST}>
            {GITHUB_TABS.map(t => (
              <button
                key={t}
                ref={el => { tabRefs.current[t] = el; }}
                type="button"
                role="tab"
                id={`github-tab-${t}`}
                aria-selected={tab === t}
                aria-controls={`github-panel-${t}`}
                tabIndex={tab === t ? 0 : -1}
                onClick={() => onTab(t)}
                onKeyDown={e => {
                  const next = moveTab(t, e.key, GITHUB_TABS);
                  if (!next) return;
                  e.preventDefault();
                  onTab(next);
                  tabRefs.current[next]?.focus();
                }}
                className={tabClass(tab === t)}
              >
                {GITHUB_TAB_LABELS[t]}
                {t === 'repositories' && status.repos.length === 0 && (
                  <span className="min-w-[18px] h-[18px] px-1 inline-flex items-center justify-center rounded-full text-caption font-semibold bg-shodh-warning-soft text-shodh-warning">
                    !<span className="sr-only"> none yet</span>
                  </span>
                )}
              </button>
            ))}
          </div>

          <div role="tabpanel" id={`github-panel-${tab}`} aria-labelledby={`github-tab-${tab}`} tabIndex={0} className="focus:outline-none">
            {tab === 'overview' && <OverviewTab {...tabProps} onTab={onTab} onSignIn={() => void signIn()} />}
            {tab === 'repositories' && <RepositoriesTab {...tabProps} highlight={highlight} />}
            {tab === 'activity' && <ActivityTab {...tabProps} />}
            {tab === 'settings' && <SettingsTab {...tabProps} onSignOut={() => setConfirmSignOut(true)} onSignIn={() => void signIn()} />}
          </div>
        </>
      )}
    </div>
  );
}

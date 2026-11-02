/**
 * The GitHub page's words and rules, from the status the backend sends
 * (`app/src-tauri/src/connectors/github.rs`). Pure, so `node --test` loads it
 * (`app/tests/github.test.ts`).
 */

import { localTime, type TimeOptions } from '../tables/format.ts';
import type { Pill, StateLine, Tone } from './slackModel.ts';
import { countdown } from './telegramModel.ts';

/** Emitted with the full {@link GitHubStatus} whenever it changes. */
export const GITHUB_STATUS_EVENT = 'connectors-github-status';

export type GitHubState = 'not_signed_in' | 'signing_in' | 'signed_in' | 'needs_sign_in' | 'local_only';

export interface GitHubAccountView {
  login: string;
  name: string | null;
  profileUrl: string;
  signedInAt: string;
}

export interface GitHubSignInView {
  userCode: string;
  verificationUri: string;
  expiresAt: string;
}

export interface GitHubRepoView {
  owner: string;
  name: string;
  fullName: string;
  url: string;
  write: boolean;
  merge: boolean;
}

export interface GitHubStatus {
  state: GitHubState;
  account: GitHubAccountView | null;
  signIn: GitHubSignInView | null;
  signInError: string | null;
  repos: GitHubRepoView[];
  rateLimitedUntil: string | null;
  installUrl: string;
  authorizedAppsUrl: string;
  permissionsNote: string;
  /** Commands Shodh runs can read this sign-in (Windows: commands are not sandboxed yet). */
  commandsCanReadSignIn: boolean;
}

/** A repository the Shodh app is installed on (`github_available_repos`). */
export interface AvailableRepo {
  owner: string;
  name: string;
  fullName: string;
  private: boolean;
  onList: boolean;
}

/** One line of the connector's activity (`github_activity`). */
export interface GitHubActivityItem {
  at: string;
  action: string;
  summary: string;
  ok: boolean;
}

export type GitHubTabId = 'overview' | 'repositories' | 'activity' | 'settings';

/** What each level lets Shodh do, in plain words. */
export const LEVEL_TEXT = {
  read: 'Fetch the code and read issues and pull requests. Nothing changes on GitHub.',
  write: 'Also push its own shodh/… branches, open pull requests and comment. Each one shows you exactly what it will do and waits for your yes.',
  merge: 'Also merge pull requests, only when GitHub says every required check passed and every required review is given. Never overrides branch protection. Each merge waits for your yes.',
} as const;

/** What Shodh never does on GitHub, whatever is allowed. */
export const HARD_LIMITS: readonly string[] = [
  'Never force-pushes or rewrites history on GitHub.',
  'Never pushes to the default branch or a protected branch: only to its own shodh/… branches.',
  'Never deletes a branch other than its own merged shodh/… branches, and never tags or repositories.',
  'Never changes settings, secrets, webhooks, Actions workflows or who has access.',
];

/** Code mode, in one line. */
export const CODE_MODE_NOTE =
  'In Code mode, fetches and pushes go through Shodh itself: commands get no GitHub sign-in in their environment, and on macOS they have no network at all.';

/** Said wherever the sign-in matters while commands are not sandboxed (Windows). */
export const WINDOWS_COMMANDS_NOTE =
  'On Windows, Code mode can’t write to GitHub yet: commands there aren’t sandboxed, so an approved command could reach your GitHub sign-in. In Code mode Shodh still fetches and reads issues and pull requests; pushes, pull requests, comments, merges and branch deletions work from a normal chat, each with your yes. One risk remains: a command you approve runs as you and could read the sign-in from Windows Credential Manager and use GitHub directly. Approve commands with that in mind.';

/** What is not yet verified on macOS. */
export const MACOS_NOTE =
  'On macOS, commands run in a sandbox with no network. Whether a sandboxed command can read the environment of Shodh’s own git process, which carries the sign-in while a fetch or push runs, is not yet verified.';

/** Why signing out does not revoke on GitHub. */
export const REVOKE_NOTE =
  'Signing out deletes the sign-in from this computer. To revoke Shodh’s access on GitHub too, open Authorized GitHub Apps on github.com and revoke Shodh there.';

// ── The level of a repository ───────────────────────────────────────

export type Level = 'read' | 'write';

export function levelOf(repo: Pick<GitHubRepoView, 'write'>): Level {
  return repo.write ? 'write' : 'read';
}

/** "Read", "Read and write", "Read, write and merge". */
export function levelLabel(repo: Pick<GitHubRepoView, 'write' | 'merge'>): string {
  if (repo.write && repo.merge) return 'Read, write and merge';
  if (repo.write) return 'Read and write';
  return 'Read only';
}

/** What `github_set_repo` is sent when the user picks `level` or flips merge. Merge needs write. */
export function nextGrant(repo: Pick<GitHubRepoView, 'write' | 'merge'>, change: { level?: Level; merge?: boolean }): { write: boolean; merge: boolean } {
  const write = change.level ? change.level === 'write' : repo.write;
  const merge = write && (change.merge ?? repo.merge);
  return { write, merge };
}

// ── The picker ──────────────────────────────────────────────────────

/** Repositories matching `query` (owner or name, any case), on-list ones last. */
export function filterRepos(repos: readonly AvailableRepo[], query: string): AvailableRepo[] {
  const q = query.trim().toLowerCase();
  return repos
    .filter(r => !q || r.fullName.toLowerCase().includes(q))
    .slice()
    .sort((a, b) => Number(a.onList) - Number(b.onList) || a.fullName.toLowerCase().localeCompare(b.fullName.toLowerCase()));
}

// ── The wizard ──────────────────────────────────────────────────────

export type GitHubStep = 1 | 2 | 3;

export const GITHUB_STEPS: readonly GitHubStep[] = [1, 2, 3];

export interface GitHubStepText {
  step: GitHubStep;
  title: string;
  detail: string;
}

export function githubSteps(): GitHubStepText[] {
  return [
    {
      step: 1,
      title: 'Sign in with GitHub',
      detail: 'Type a short code on github.com. Shodh never sees your password, and keeps the sign-in in this computer’s credential store.',
    },
    {
      step: 2,
      title: 'Choose repositories',
      detail: 'Shodh reaches only the repositories you pick here, and only those the Shodh app is installed on.',
    },
    {
      step: 3,
      title: 'Decide what Shodh may do',
      detail: 'Read is enough to fetch and read issues. Write lets it push its own branches and open pull requests, each with your yes.',
    },
  ];
}

/** The step the setup shows: signing in until there is an account. */
export function githubStep(status: Pick<GitHubStatus, 'account'>, wanted: GitHubStep): GitHubStep {
  if (!status.account) return 1;
  return wanted === 1 ? 2 : wanted;
}

/** "9:41" left of a sign-in code, or `null` once it expired. */
export function codeCountdown(signIn: Pick<GitHubSignInView, 'expiresAt'> | null, now: number): string | null {
  return signIn ? countdown(signIn.expiresAt, now) : null;
}

// ── The tile and the page ───────────────────────────────────────────

function parseTime(iso: string | null): number | null {
  if (!iso) return null;
  const ms = Date.parse(iso);
  return Number.isNaN(ms) ? null : ms;
}

function rateLimited(status: Pick<GitHubStatus, 'rateLimitedUntil'>, now: number): boolean {
  const until = parseTime(status.rateLimitedUntil);
  return until !== null && until > now;
}

/** The pill of the GitHub connector. `team` carries the account's login. */
export function githubPill(status: GitHubStatus, now: number = Date.now()): Pill {
  const login = status.account ? `@${status.account.login}` : null;
  switch (status.state) {
    case 'not_signed_in':
      return { kind: 'not_connected', tone: 'neutral', label: 'Not connected', reason: null, team: null };
    case 'signing_in':
      return { kind: 'connecting', tone: 'active', label: 'Signing in…', reason: null, team: login };
    case 'local_only':
      return { kind: 'off', tone: 'neutral', label: 'Off', reason: 'Local-only mode', team: login };
    case 'needs_sign_in':
      return { kind: 'attention', tone: 'error', label: 'Needs attention', reason: 'Sign in again', team: login };
    case 'signed_in':
      if (status.repos.length === 0) {
        return { kind: 'attention', tone: 'warning', label: 'Needs attention', reason: 'No repositories yet', team: login };
      }
      if (rateLimited(status, now)) {
        return { kind: 'attention', tone: 'warning', label: 'Needs attention', reason: 'Rate limited by GitHub', team: login };
      }
      return { kind: 'connected', tone: 'ok', label: 'Connected', reason: null, team: login };
  }
}

/** The tile's one action. */
export function githubTileAction(status: Pick<GitHubStatus, 'account'>): 'Connect' | 'Manage' {
  return status.account ? 'Manage' : 'Connect';
}

/** What the connection is, in plain words. */
export function githubStateLine(status: GitHubStatus, now: number, options: TimeOptions = {}): StateLine {
  const login = status.account ? `@${status.account.login}` : 'GitHub';
  switch (status.state) {
    case 'not_signed_in':
      return { tone: 'neutral', title: 'Not signed in', detail: 'Sign in with GitHub, then choose the repositories Shodh may reach.' };
    case 'signing_in':
      return { tone: 'active', title: 'Waiting for the code on GitHub…', detail: 'Type the code shown here at github.com/login/device.' };
    case 'local_only':
      return {
        tone: 'neutral',
        title: 'Paused: Local-only mode is on',
        detail: 'Shodh does not connect to GitHub while Local-only mode is on. Turn it off in Settings → Privacy.',
      };
    case 'needs_sign_in':
      return {
        tone: 'error',
        title: 'Sign in again',
        detail: 'GitHub no longer accepts Shodh’s sign-in (it was revoked or expired). Shodh does no GitHub work until you sign in again.',
      };
    case 'signed_in': {
      const until = parseTime(status.rateLimitedUntil);
      if (until !== null && until > now) {
        return { tone: 'warning', title: `Signed in as ${login}`, detail: `GitHub is rate limiting Shodh until ${localTime(until, now, options)}.` };
      }
      return { tone: 'ok', title: `Signed in as ${login}`, detail: null };
    }
  }
}

/** "3 repositories: 1 read only, 2 read and write". */
export function reposLine(status: Pick<GitHubStatus, 'repos'>): string {
  const n = status.repos.length;
  if (n === 0) return 'No repositories yet: Shodh can reach none.';
  const writable = status.repos.filter(r => r.write).length;
  const merging = status.repos.filter(r => r.write && r.merge).length;
  const parts = [`${n} ${n === 1 ? 'repository' : 'repositories'}`];
  const levels: string[] = [];
  if (n - writable > 0) levels.push(`${n - writable} read only`);
  if (writable > 0) levels.push(`${writable} with write`);
  if (merging > 0) levels.push(`${merging} with merge`);
  return `${parts[0]}: ${levels.join(', ')}`;
}

/** Something left to set up or fix, on the overview. */
export interface GitHubNextStep {
  id: string;
  tone: Tone;
  title: string;
  detail: string;
  tab: GitHubTabId | null;
  action: string | null;
}

export function githubNextSteps(status: GitHubStatus, now: number = Date.now()): GitHubNextStep[] {
  const steps: GitHubNextStep[] = [];
  if (status.state === 'local_only') {
    steps.push({
      id: 'local-only',
      tone: 'neutral',
      title: 'Local-only mode is on',
      detail: 'Shodh does not connect to GitHub while it is on. Turn it off in Settings → Privacy.',
      tab: null,
      action: null,
    });
  }
  if (status.state === 'needs_sign_in') {
    steps.push({
      id: 'sign-in',
      tone: 'error',
      title: 'Sign in to GitHub again',
      detail: 'GitHub no longer accepts Shodh’s sign-in. Nothing reaches GitHub until you sign in.',
      tab: 'settings',
      action: 'Sign in again',
    });
  }
  if (status.account && status.repos.length === 0) {
    steps.push({
      id: 'repos',
      tone: 'warning',
      title: 'Choose the repositories Shodh may reach',
      detail: 'Until you do, Shodh can reach none of them.',
      tab: 'repositories',
      action: 'Choose repositories',
    });
  }
  if (rateLimited(status, now)) {
    steps.push({
      id: 'rate-limit',
      tone: 'warning',
      title: 'GitHub is rate limiting Shodh',
      detail: 'GitHub work waits until the limit resets.',
      tab: null,
      action: null,
    });
  }
  return steps;
}

/** When an activity item happened, in local time. */
export function activityTime(item: Pick<GitHubActivityItem, 'at'>, now: number, options: TimeOptions = {}): string {
  const at = parseTime(item.at);
  return at === null ? '' : localTime(at, now, options);
}

// ── The agent's navigation ──────────────────────────────────────────

/**
 * Where an agent's `navigated` focus opens the GitHub page: `github` (the overview) or
 * `github:repositories:<owner/name>` (the repository highlighted). `null` for anything else.
 */
export function githubFocus(focus: string | null | undefined): { tab: GitHubTabId; item: string | null } | null {
  if (!focus) return null;
  if (focus === 'github') return { tab: 'overview', item: null };
  const match = /^github:(overview|repositories|activity|settings)(?::([A-Za-z0-9-]{1,39}\/[A-Za-z0-9._-]{1,100}))?$/.exec(focus);
  if (!match) return null;
  return { tab: match[1] as GitHubTabId, item: match[2] ?? null };
}

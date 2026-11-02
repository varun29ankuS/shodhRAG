/**
 * GitHub page tests: the pill and next steps for every state, the setup's step, the
 * permission levels (merge needs write), the repository picker's search, the agent's
 * navigation focus, and the page navigation an Inbox item uses. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/github.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  HARD_LIMITS,
  codeCountdown,
  filterRepos,
  githubFocus,
  githubNextSteps,
  githubPill,
  githubStateLine,
  githubStep,
  githubTileAction,
  levelLabel,
  nextGrant,
  reposLine,
  type AvailableRepo,
  type GitHubStatus,
} from '../src/features/connectors/githubModel.ts';
import { GITHUB_TABS, moveTab, parseConnectorTarget, tabsOf } from '../src/features/connectors/navigation.ts';

const NOW = Date.parse('2026-11-02T10:00:00Z');

function status(over: Partial<GitHubStatus> = {}): GitHubStatus {
  return {
    state: 'signed_in',
    account: { login: 'ann', name: 'Ann Lee', profileUrl: 'https://github.com/ann', signedInAt: '2026-11-01T09:00:00Z' },
    signIn: null,
    signInError: null,
    repos: [{ owner: 'octo-org', name: 'hello', fullName: 'octo-org/hello', url: 'https://github.com/octo-org/hello', write: true, merge: false }],
    rateLimitedUntil: null,
    installUrl: 'https://github.com/apps/shodh-rag/installations/new',
    authorizedAppsUrl: 'https://github.com/settings/apps/authorizations',
    permissionsNote: 'note',
    commandsCanReadSignIn: false,
    ...over,
  };
}

test('the pill says what needs the user, most pressing first', () => {
  assert.deepEqual(githubPill(status({ state: 'not_signed_in', account: null, repos: [] }), NOW), {
    kind: 'not_connected', tone: 'neutral', label: 'Not connected', reason: null, team: null,
  });
  assert.equal(githubPill(status(), NOW).label, 'Connected');
  assert.equal(githubPill(status(), NOW).team, '@ann');
  assert.equal(githubPill(status({ repos: [] }), NOW).reason, 'No repositories yet');
  assert.equal(githubPill(status({ state: 'needs_sign_in' }), NOW).reason, 'Sign in again');
  assert.equal(githubPill(status({ state: 'needs_sign_in' }), NOW).tone, 'error');
  assert.equal(githubPill(status({ state: 'local_only' }), NOW).reason, 'Local-only mode');
  assert.equal(githubPill(status({ state: 'signing_in' }), NOW).label, 'Signing in…');
  assert.equal(githubPill(status({ rateLimitedUntil: '2026-11-02T11:00:00Z' }), NOW).reason, 'Rate limited by GitHub');
  // A limit that already reset is not shown.
  assert.equal(githubPill(status({ rateLimitedUntil: '2026-11-02T09:00:00Z' }), NOW).label, 'Connected');
  assert.equal(githubTileAction(status({ account: null })), 'Connect');
  assert.equal(githubTileAction(status()), 'Manage');
});

test('the state line and next steps for every state', () => {
  assert.equal(githubStateLine(status(), NOW).title, 'Signed in as @ann');
  assert.equal(githubStateLine(status({ state: 'needs_sign_in' }), NOW).tone, 'error');
  assert.match(githubStateLine(status({ state: 'local_only' }), NOW).detail ?? '', /Local-only mode/);
  assert.deepEqual(githubNextSteps(status(), NOW), []);
  assert.deepEqual(githubNextSteps(status({ repos: [] }), NOW).map(s => s.id), ['repos']);
  assert.deepEqual(githubNextSteps(status({ state: 'needs_sign_in' }), NOW).map(s => s.id), ['sign-in']);
  assert.equal(githubNextSteps(status({ repos: [] }), NOW)[0].tab, 'repositories');
});

test('the setup signs in first, then picks repositories and permissions', () => {
  assert.equal(githubStep(status({ account: null }), 3), 1);
  assert.equal(githubStep(status(), 1), 2);
  assert.equal(githubStep(status(), 3), 3);
  assert.equal(codeCountdown({ expiresAt: '2026-11-02T10:14:30Z' }, NOW), '14:30');
  assert.equal(codeCountdown({ expiresAt: '2026-11-02T09:59:00Z' }, NOW), null);
  assert.equal(codeCountdown(null, NOW), null);
});

test('merge needs write, and dropping write drops merge', () => {
  const repo = { write: true, merge: true };
  assert.deepEqual(nextGrant(repo, { level: 'read' }), { write: false, merge: false });
  assert.deepEqual(nextGrant({ write: false, merge: false }, { merge: true }), { write: false, merge: false });
  assert.deepEqual(nextGrant({ write: true, merge: false }, { merge: true }), { write: true, merge: true });
  assert.deepEqual(nextGrant({ write: false, merge: false }, { level: 'write' }), { write: true, merge: false });
  assert.equal(levelLabel({ write: true, merge: true }), 'Read, write and merge');
  assert.equal(levelLabel({ write: false, merge: true }), 'Read only');
  assert.equal(reposLine(status({ repos: [] })), 'No repositories yet: Shodh can reach none.');
  assert.equal(reposLine(status()), '1 repository: 1 with write');
});

test('the picker searches by owner or name and lists new repositories first', () => {
  const repos: AvailableRepo[] = [
    { owner: 'octo-org', name: 'hello', fullName: 'octo-org/hello', private: false, onList: true },
    { owner: 'ann', name: 'notes', fullName: 'ann/notes', private: true, onList: false },
    { owner: 'octo-org', name: 'site', fullName: 'octo-org/site', private: false, onList: false },
  ];
  assert.deepEqual(filterRepos(repos, '').map(r => r.fullName), ['ann/notes', 'octo-org/site', 'octo-org/hello']);
  assert.deepEqual(filterRepos(repos, 'OCTO').map(r => r.fullName), ['octo-org/site', 'octo-org/hello']);
  assert.deepEqual(filterRepos(repos, 'nothing'), []);
});

test('the agent opens the page at the repository it acted on', () => {
  assert.deepEqual(githubFocus('github'), { tab: 'overview', item: null });
  assert.deepEqual(githubFocus('github:repositories:octo-org/hello'), { tab: 'repositories', item: 'octo-org/hello' });
  assert.deepEqual(githubFocus('github:activity'), { tab: 'activity', item: null });
  assert.equal(githubFocus('github:repositories:../etc'), null);
  assert.equal(githubFocus('github:billing'), null);
  assert.equal(githubFocus('task-12'), null);
  assert.equal(githubFocus(null), null);
  // The Rust tools send exactly these focuses.
  const rust = readFileSync(new URL('../../crates/shodh-rag/src/connectors/github/tools.rs', import.meta.url), 'utf8');
  assert.ok(rust.includes('Some("github".to_string())'));
  assert.ok(rust.includes('format!("github:repositories:{}", repo.full_name())'));
});

test('an Inbox item opens the GitHub page at its tab', () => {
  assert.deepEqual(tabsOf('github'), GITHUB_TABS);
  assert.deepEqual(parseConnectorTarget({ connector: 'github', tab: 'activity' }), { connector: 'github', tab: 'activity' });
  assert.deepEqual(parseConnectorTarget({ connector: 'github', tab: 'people' }), { connector: 'github', tab: null });
  assert.equal(moveTab('settings', 'ArrowRight', GITHUB_TABS), 'overview');
  // Every tab the Rust notices link to exists on the page.
  const rust = readFileSync(new URL('../src-tauri/src/connectors/github_notices.rs', import.meta.url), 'utf8');
  for (const [, tab] of rust.matchAll(/GitHubTab::\w+ => "([a-z]+)"/g)) {
    assert.ok((GITHUB_TABS as readonly string[]).includes(tab), tab);
  }
});

test('the hard limits are stated in plain words', () => {
  assert.ok(HARD_LIMITS.some(l => l.includes('force-pushes')));
  assert.ok(HARD_LIMITS.some(l => l.includes('default branch')));
  assert.ok(HARD_LIMITS.some(l => l.includes('workflows')));
});

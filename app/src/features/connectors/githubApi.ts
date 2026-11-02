/**
 * Tauri commands of the GitHub page (`connectors/github.rs`). Every call is the user's
 * own action; the sign-in happens on github.com and its token never reaches this page.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { GITHUB_STATUS_EVENT, type AvailableRepo, type GitHubActivityItem, type GitHubStatus } from './githubModel';

export const githubApi = {
  status: () => invoke<GitHubStatus>('github_status'),
  /** Asks GitHub for a code to type at github.com/login/device; the sign-in finishes in the background. */
  signInStart: () => invoke<GitHubStatus>('github_sign_in_start'),
  signInCancel: () => invoke<GitHubStatus>('github_sign_in_cancel'),
  signOut: () => invoke<GitHubStatus>('github_sign_out'),
  availableRepos: () => invoke<AvailableRepo[]>('github_available_repos'),
  setRepo: (owner: string, name: string, write: boolean, merge: boolean) =>
    invoke<GitHubStatus>('github_set_repo', { owner, name, write, merge }),
  removeRepo: (owner: string, name: string) => invoke<GitHubStatus>('github_remove_repo', { owner, name }),
  activity: (limit?: number) => invoke<GitHubActivityItem[]>('github_activity', { limit: limit ?? null }),
};

/** Calls `handler` with every status the backend emits; returns the unsubscribe. */
export function onGitHubStatus(handler: (status: GitHubStatus) => void): () => void {
  let disposed = false;
  let unlisten: (() => void) | null = null;
  listen<GitHubStatus>(GITHUB_STATUS_EVENT, event => handler(event.payload))
    .then(fn => {
      if (disposed) fn();
      else unlisten = fn;
    })
    .catch(err => console.error(`Failed to listen for ${GITHUB_STATUS_EVENT}:`, err));
  return () => {
    disposed = true;
    if (unlisten) unlisten();
  };
}

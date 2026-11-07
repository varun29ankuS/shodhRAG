#!/usr/bin/env node
// A release is built only from a commit on which CI passed: every job of
// ci.yml (REQUIRED_CHECKS) has a successful check run on that commit, and so
// has the nightly run of that commit when there is one (NIGHTLY_CHECK).
//
//   node scripts/release/ci-gate.mjs --repo O/R --sha SHA [--timeout-minutes N] [--interval-seconds S]
//
// CI on a master commit runs at the same time as the release workflows that
// the same push starts, so this waits, polling the commit's check runs, until
// every required check has finished: it succeeds when all passed, and fails
// as soon as one failed (or was cancelled, skipped or timed out), or when the
// wait runs out. Needs GH_TOKEN (checks: read).

import { parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';

export class GateError extends Error {}

// ci.yml's job names (its check run names). On a master commit every job
// runs (pull requests alone skip heavy jobs a change cannot affect), so a
// skipped one fails the gate. The release builds job calls release.yml, so
// its check runs are named after release.yml's jobs ("Release builds / ..."):
// it is not listed here, and CI gate, which fails when it did not pass,
// stands for it. test/ci-gate.test.mjs fails when they drift apart.
export const REQUIRED_CHECKS = [
  'Changes',
  'Workflow lint',
  'Release and CI scripts',
  'Format',
  'Check + Clippy',
  'Tests',
  'macOS build + OCR tests (shodh-rag)',
  'Retrieval eval',
  'Retrieval eval (int8, Apple silicon)',
  'Frontend Typecheck + Tests',
  'Footprint budget',
  'CI gate',
];

// nightly.yml's aggregate on the same commit (its job "CI" calls ci.yml). It
// is not required, since most commits are never measured by a nightly run,
// but when one ran, a release waits for it and is blocked unless it passed:
// a commit whose push run passed and whose nightly run failed (a runner
// image, a retired label) is not released until a re-run passes.
export const NIGHTLY_CHECK = 'CI / CI gate';

const GITHUB_ACTIONS_APP = 'github-actions';

const newest = (checkRuns, name) =>
  checkRuns.filter((r) => r.name === name && r.app?.slug === GITHUB_ACTIONS_APP).sort((a, b) => b.id - a.id)[0];

/**
 * Evaluates a commit's check runs (the API's check_runs array). For each
 * required name the newest run by GitHub Actions counts (a re-run replaces
 * the run it repeats); the nightly aggregate counts the same way when present.
 * Returns { state: 'success' | 'pending' | 'failure', lines } with one line
 * per required check (and one for the nightly run when there is one).
 */
export function evaluateChecks(checkRuns, required = REQUIRED_CHECKS) {
  const lines = [];
  let pending = false;
  let failed = false;
  const judge = (name, run) => {
    if (run.status !== 'completed') {
      pending = true;
      lines.push(`${name}: ${run.status}`);
    } else if (run.conclusion === 'success') {
      lines.push(`${name}: success`);
    } else {
      failed = true;
      lines.push(`${name}: ${run.conclusion} (${run.html_url ?? run.id})`);
    }
  };
  for (const name of required) {
    const run = newest(checkRuns, name);
    if (!run) {
      pending = true;
      lines.push(`${name}: not started`);
    } else {
      judge(name, run);
    }
  }
  const nightly = newest(checkRuns, NIGHTLY_CHECK);
  if (nightly) judge(`${NIGHTLY_CHECK} (nightly)`, nightly);
  return { state: failed ? 'failure' : pending ? 'pending' : 'success', lines };
}

async function checkRunsFor({ repo, sha, token, fetchImpl }) {
  const all = [];
  for (let page = 1; page <= 10; page++) {
    const response = await fetchImpl(`https://api.github.com/repos/${repo}/commits/${sha}/check-runs?per_page=100&page=${page}`, {
      headers: { Accept: 'application/vnd.github+json', Authorization: `Bearer ${token}` },
    });
    if (!response.ok) throw new GateError(`Listing the check runs of ${sha} answered ${response.status}`);
    const body = await response.json();
    all.push(...body.check_runs);
    if (all.length >= body.total_count || body.check_runs.length === 0) break;
  }
  return all;
}

export async function waitForCi({
  repo,
  sha,
  token,
  timeoutMinutes = 180,
  intervalSeconds = 60,
  fetchImpl = fetch,
  sleep = (ms) => new Promise((r) => setTimeout(r, ms)),
  now = () => Date.now(),
}) {
  if (!/^[0-9a-f]{40}$/.test(sha ?? '')) throw new GateError(`${sha} is not a commit SHA`);
  const deadline = now() + timeoutMinutes * 60_000;
  let last = '';
  for (;;) {
    const { state, lines } = evaluateChecks(await checkRunsFor({ repo, sha, token, fetchImpl }));
    const report = lines.join('\n  ');
    if (report !== last) console.log(`CI on ${sha}: ${state}\n  ${report}`);
    last = report;
    if (state === 'success') return lines;
    if (state === 'failure') throw new GateError(`CI did not pass on ${sha}: nothing is released from it.\n  ${report}`);
    if (now() + intervalSeconds * 1000 > deadline) {
      throw new GateError(`CI on ${sha} had not finished after ${timeoutMinutes} minutes: nothing is released from it.\n  ${report}`);
    }
    await sleep(intervalSeconds * 1000);
  }
}

async function main(argv) {
  const { values } = parseArgs({
    args: argv,
    options: {
      repo: { type: 'string' },
      sha: { type: 'string' },
      'timeout-minutes': { type: 'string', default: '180' },
      'interval-seconds': { type: 'string', default: '60' },
    },
  });
  const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN;
  if (!values.repo || !values.sha || !token) throw new GateError('Usage: GH_TOKEN=... ci-gate.mjs --repo O/R --sha SHA');
  await waitForCi({
    repo: values.repo,
    sha: values.sha,
    token,
    timeoutMinutes: Number(values['timeout-minutes']),
    intervalSeconds: Number(values['interval-seconds']),
  });
  console.log(`CI passed on ${values.sha}`);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  main(process.argv.slice(2)).catch((e) => {
    if (!(e instanceof GateError)) throw e;
    console.error(`::error::${e.message.replace(/\n/g, '%0A')}`);
    process.exit(1);
  });
}

#!/usr/bin/env node
// Whether a nightly run (nightly.yml) failed as a release reads it: by its
// CI gate, the check scripts/release/ci-gate.mjs requires of a commit's
// nightly run (NIGHTLY_CHECK, "CI / CI gate"), not by the whole call of
// ci.yml. A job CI gate does not wait for (one that is not required) fails
// the call, and so the nightly's `ci` job, without blocking a release; it
// must not open the "Nightly CI is failing" issue either.
//
//   gh run view RUN --json jobs > jobs.json
//   node scripts/release/nightly-verdict.mjs --jobs jobs.json [--github-output]
//
// Prints the verdict; with --github-output also writes `failed` (true or
// false) and `gate` (the gate's conclusion, or "missing") to $GITHUB_OUTPUT.
// A gate that is missing, not finished, or anything but a success is a
// failure: a run that never reached its gate did not pass.

import { appendFileSync, readFileSync } from 'node:fs';
import { parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';

import { NIGHTLY_CHECK } from './ci-gate.mjs';

export class VerdictError extends Error {}

/**
 * The verdict on a run's jobs (the `jobs` array of `gh run view --json
 * jobs`): { failed, gate, others } where `gate` is the CI gate's conclusion
 * ("missing" when the run has no such job, or its status when unfinished) and
 * `others` lists the other jobs that did not pass, as "name: conclusion".
 * When a job of the gate's name appears more than once, the newest
 * (highest databaseId) counts, as a re-run's does.
 */
export function nightlyVerdict(jobs) {
  if (!Array.isArray(jobs)) throw new VerdictError('The run has no list of jobs');
  const gates = jobs.filter((j) => j?.name === NIGHTLY_CHECK).sort((a, b) => (b.databaseId ?? 0) - (a.databaseId ?? 0));
  const gate = gates[0];
  const state = !gate ? 'missing' : gate.status !== 'completed' ? (gate.status ?? 'unknown') : (gate.conclusion ?? 'unknown');
  const others = jobs
    .filter((j) => j?.name !== NIGHTLY_CHECK && j?.conclusion && !['success', 'skipped'].includes(j.conclusion))
    .map((j) => `${j.name}: ${j.conclusion}`);
  return { failed: state !== 'success', gate: state, others };
}

function main(argv) {
  const { values } = parseArgs({
    args: argv,
    options: {
      jobs: { type: 'string' },
      'github-output': { type: 'boolean', default: false },
    },
  });
  if (!values.jobs) throw new VerdictError('Usage: nightly-verdict.mjs --jobs FILE [--github-output]');
  let parsed;
  try {
    parsed = JSON.parse(readFileSync(values.jobs, 'utf8'));
  } catch (e) {
    throw new VerdictError(`${values.jobs} is not the JSON of gh run view --json jobs: ${e.message}`);
  }
  const verdict = nightlyVerdict(parsed?.jobs);
  console.log(`${NIGHTLY_CHECK}: ${verdict.gate}`);
  for (const line of verdict.others) console.log(`  did not pass: ${line}`);
  if (verdict.failed) {
    console.log('The nightly run failed as a release reads it.');
  } else if (verdict.others.length > 0) {
    console.log(`The nightly run passed as a release reads it: CI gate does not wait for the jobs above.`);
  } else {
    console.log('The nightly run passed.');
  }
  if (values['github-output']) {
    const path = process.env.GITHUB_OUTPUT;
    if (!path) throw new VerdictError('--github-output needs GITHUB_OUTPUT');
    appendFileSync(path, `failed=${verdict.failed}\ngate=${verdict.gate}\n`);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    if (!(e instanceof VerdictError)) throw e;
    console.error(`::error::${e.message}`);
    process.exit(1);
  }
}

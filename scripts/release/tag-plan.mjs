#!/usr/bin/env node
// What tag-release.yml does with a master commit whose files carry version
// vX.Y.Z (scripts/release/versions.mjs):
//
//   tagged, release published                  nothing
//   tagged at this commit, not published       release again (no new tag):
//     (an earlier run failed)                    a re-run converges
//   tagged at another commit, not published    fail: bump the version, or run
//                                                Release with that tag
//   not tagged, above every vX.Y.Z tag         tag this commit and release
//   not tagged, not above the newest tag       fail
//
// On pull requests the decision is only reported.
//
//   node scripts/release/tag-plan.mjs --tag vX.Y.Z --sha SHA --event push|pull_request
//     [--tagged-sha SHA] [--published] [--tags "v0.0.9 v0.0.10 ..."] [--github-output]

import { appendFileSync } from 'node:fs';
import { parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';

import { compareVersions } from './versions.mjs';

export class PlanError extends Error {}

const TAG = /^v(\d+\.\d+\.\d+)$/;

/**
 * Returns { release, createTag, message }. Throws PlanError for the states
 * that need a person.
 */
export function decideTag({ tag, sha, event, taggedSha = '', published = false, tags = [] }) {
  const own = TAG.exec(tag ?? '');
  if (!own) throw new PlanError(`${tag} is not a release tag (vX.Y.Z)`);
  if (!/^[0-9a-f]{40}$/.test(sha ?? '')) throw new PlanError(`${sha} is not a commit SHA`);
  const dryRun = event === 'pull_request';
  const act = (createTag, message) =>
    dryRun ? { release: false, createTag: false, message: `Merging this would: ${message}` } : { release: true, createTag, message };

  if (taggedSha) {
    if (published) return { release: false, createTag: false, message: `${tag} is released: nothing to do.` };
    if (taggedSha === sha) return act(false, `${tag} is tagged at this commit but not published (an earlier run failed): release it again.`);
    throw new PlanError(
      `${tag} is tagged at ${taggedSha}, not this commit, and has no published release. ` +
        `Bump the version (Prepare release), or run the Release workflow with tag ${tag} to publish that commit.`,
    );
  }
  const versions = tags.map((t) => TAG.exec(t)?.[1]).filter(Boolean);
  const newest = versions.sort(compareVersions).at(-1);
  if (newest && compareVersions(own[1], newest) <= 0) {
    throw new PlanError(`The version ${tag} is not above the newest tag v${newest}`);
  }
  return act(true, `${tag} is new (newest tag: ${newest ? `v${newest}` : 'none'}): once CI passes on this commit, tag it and release it.`);
}

function main(argv) {
  const { values } = parseArgs({
    args: argv,
    options: {
      tag: { type: 'string' },
      sha: { type: 'string' },
      event: { type: 'string' },
      'tagged-sha': { type: 'string', default: '' },
      published: { type: 'boolean', default: false },
      tags: { type: 'string', default: '' },
      'github-output': { type: 'boolean', default: false },
    },
  });
  const plan = decideTag({
    tag: values.tag,
    sha: values.sha,
    event: values.event,
    taggedSha: values['tagged-sha'],
    published: values.published,
    tags: values.tags.split(/\s+/).filter(Boolean),
  });
  console.log(values.event === 'pull_request' ? `::notice::${plan.message}` : plan.message);
  if (values['github-output']) {
    appendFileSync(process.env.GITHUB_OUTPUT, `release=${plan.release}\ncreate-tag=${plan.createTag}\n`);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    if (!(e instanceof PlanError)) throw e;
    console.error(`::error::${e.message}`);
    process.exit(1);
  }
}

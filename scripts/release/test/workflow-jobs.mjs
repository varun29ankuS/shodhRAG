// The repository's workflow files, read as text: shared by the tests that
// check what a workflow's jobs are and what they may run.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

export const ROOT = fileURLToPath(new URL('../../../', import.meta.url));
export const WORKFLOWS = join(ROOT, '.github', 'workflows');
export const workflow = (file) => readFileSync(join(WORKFLOWS, file), 'utf8');

// A workflow's jobs: each id, its block (up to the next job) and its name
// (quotes and a trailing comment removed). Fails when there is no jobs: key,
// so a test over a workflow can never pass on nothing.
export function jobsOf(file) {
  const yml = workflow(file);
  const start = yml.search(/^jobs:[ \t]*(#.*)?$/m);
  assert.ok(start >= 0, `${file} has a top-level jobs: key`);
  const section = yml.slice(start);
  const starts = [...section.matchAll(/^ {2}([A-Za-z_][A-Za-z0-9_-]*):[ \t]*(#.*)?$/gm)];
  assert.ok(starts.length > 0, `${file} has jobs`);
  return starts.map((m, i) => {
    const block = section.slice(m.index, starts[i + 1]?.index ?? section.length);
    const raw = block.match(/^ {4}name:[ \t]*(.+)$/m)?.[1];
    const name = raw
      ?.replace(/[ \t]+#.*$/, '')
      .trim()
      .replace(/^(['"])(.*)\1$/, '$2');
    return { id: m[1], block, name, calls: /^ {4}uses:[ \t]/m.test(block) };
  });
}

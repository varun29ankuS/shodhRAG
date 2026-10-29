import assert from 'node:assert/strict';
import { test } from 'node:test';

import { PlanError, decideTag } from '../tag-plan.mjs';

const SHA = 'a'.repeat(40);
const OTHER = 'b'.repeat(40);
const TAGS = ['v0.0.9', 'v0.0.10', 'nightly', 'v0.0.10-rc.1'];

test('tagged and published: nothing to do', () => {
  const plan = decideTag({ tag: 'v0.0.10', sha: SHA, event: 'push', taggedSha: OTHER, published: true, tags: TAGS });
  assert.deepEqual([plan.release, plan.createTag], [false, false]);
  assert.match(plan.message, /released: nothing to do/);
});

test('tagged at this commit, not published: release again without a new tag', () => {
  const plan = decideTag({ tag: 'v0.0.10', sha: SHA, event: 'push', taggedSha: SHA, published: false, tags: TAGS });
  assert.deepEqual([plan.release, plan.createTag], [true, false]);
});

test('tagged at another commit, not published: a person decides', () => {
  assert.throws(
    () => decideTag({ tag: 'v0.0.10', sha: SHA, event: 'push', taggedSha: OTHER, published: false, tags: TAGS }),
    (e) => e instanceof PlanError && /Bump the version/.test(e.message) && e.message.includes(OTHER),
  );
});

test('a new version above every tag: tag and release', () => {
  const plan = decideTag({ tag: 'v0.0.11', sha: SHA, event: 'push', tags: TAGS });
  assert.deepEqual([plan.release, plan.createTag], [true, true]);
  assert.match(plan.message, /newest tag: v0\.0\.10/);
  // 0.0.10 is newer than 0.0.9 numerically, not as text.
  assert.equal(decideTag({ tag: 'v0.1.0', sha: SHA, event: 'push', tags: ['v0.0.99'] }).createTag, true);
  assert.equal(decideTag({ tag: 'v0.0.1', sha: SHA, event: 'push', tags: [] }).createTag, true);
});

test('a new version not above the newest tag fails', () => {
  assert.throws(() => decideTag({ tag: 'v0.0.8', sha: SHA, event: 'push', tags: TAGS }), /not above the newest tag v0\.0\.10/);
});

test('pull requests only report', () => {
  for (const args of [
    { tag: 'v0.0.11', tags: TAGS },
    { tag: 'v0.0.10', taggedSha: SHA, published: false, tags: TAGS },
  ]) {
    const plan = decideTag({ sha: SHA, event: 'pull_request', ...args });
    assert.deepEqual([plan.release, plan.createTag], [false, false]);
    assert.match(plan.message, /^Merging this would:/);
  }
});

test('malformed input is refused', () => {
  assert.throws(() => decideTag({ tag: '0.0.11', sha: SHA, event: 'push' }), /not a release tag/);
  assert.throws(() => decideTag({ tag: 'v0.0.11', sha: 'master', event: 'push' }), /not a commit SHA/);
});

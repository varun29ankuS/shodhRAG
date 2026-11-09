/**
 * The design check as the code pane shows it (`features/code/designModel.ts`): the
 * badge's count and words, a finding turned into the review pane's card, when a
 * mechanical fix can be sent, and that the pane sends it only as a review action (so
 * its edit asks the user in every mode). Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/designCheck.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  SEVERITY_OF,
  asReviewFinding,
  badgeDetail,
  badgeText,
  designFixProblem,
  filesWithIssues,
  isUiFile,
  totalIssues,
} from '../src/features/code/designModel.ts';
import type { DesignFinding, StoredDesignCheck } from '../src/features/code/designModel.ts';
import type { TargetConversation } from '../src/features/code/reviewModel.ts';

const finding = (over: Partial<DesignFinding> = {}): DesignFinding => ({
  rule: 'tiny-text',
  severity: 'warning',
  origin: 'impeccable',
  title: 'Tiny body text',
  path: 'src/Card.tsx',
  line: 4,
  endLine: 4,
  explanation: 'Body text below 12px is hard to read.',
  evidence: 'text-[10px]',
  fix: 'Use 12px at the very least.',
  replace: { path: 'src/Card.tsx', old: '<p className="text-[10px]">', new: '<p className="text-xs">' },
  ...over,
});

const check = (findings: DesignFinding[], over: Partial<StoredDesignCheck> = {}): StoredDesignCheck => {
  const counts = { error: 0, warning: 0, advisory: 0 };
  for (const f of findings) counts[f.severity] += 1;
  return {
    id: 'dc_1',
    createdAt: '2026-11-09T00:00:00Z',
    target: { kind: 'changes', label: 'the changes', path: '.' },
    workspaceId: 'ws',
    creationId: null,
    conversationId: 'conv',
    byAgent: false,
    automatic: true,
    report: { findings, counts, filesChecked: 2, skipped: [], leftOut: 0, partial: false, rules: 34 },
    ...over,
  };
};

const code = (over: Partial<TargetConversation> = {}): TargetConversation => ({
  id: 'conv',
  mode: 'code',
  agentMode: 'ask',
  workspaceId: 'ws',
  running: false,
  ...over,
});

test('the badge counts every finding, also those counted but not listed', () => {
  assert.equal(badgeText(null), 'Design check');
  assert.equal(badgeText(check([])), 'Design check: No issues');
  assert.equal(badgeText(check([finding()])), 'Design check: 1 issue');
  const three = check([finding(), finding({ severity: 'error', rule: 'img-missing-alt' }), finding({ severity: 'advisory' })]);
  assert.equal(badgeText(three), 'Design check: 3 issues');
  assert.equal(totalIssues(three.report), 3);
  // A finding past the per-rule limit is counted in `counts`, not listed.
  const capped = check([finding()]);
  capped.report.counts.warning = 25;
  capped.report.leftOut = 24;
  assert.equal(badgeText(capped), 'Design check: 25 issues');
  assert.match(badgeDetail(three), /2 UI files of the changes, 1 error, 1 warning, 1 to consider\. No model is used\./);
  assert.match(badgeDetail(three), /^Checked after the last change/);
  assert.equal(filesWithIssues(three.report), 1);
});

test('a design finding becomes the review pane’s card with its file:line and exact fix', () => {
  const card = asReviewFinding(finding());
  assert.equal(card.severity, 'medium');
  assert.equal(card.title, 'Tiny body text (tiny-text)');
  assert.deepEqual(card.citation, { path: 'src/Card.tsx', line: 4, endLine: 4, side: 'new' });
  assert.match(card.scenario, /Found: text-\[10px\]/);
  assert.deepEqual(card.replace, finding().replace);
  assert.equal(card.verify, null, 'a deterministic finding proposes no check to run');
  assert.deepEqual(SEVERITY_OF, { error: 'high', warning: 'medium', advisory: 'low' });
});

test('a fix goes only to the conversation of the files, never in Plan mode or mid-answer', () => {
  const c = check([finding()]);
  for (const mode of ['ask', 'acceptEdits', 'auto']) {
    assert.equal(designFixProblem(code({ agentMode: mode }), c, finding()), null, mode);
  }
  assert.match(designFixProblem(code({ agentMode: 'plan' }), c, finding()) ?? '', /Plan mode only reads/);
  assert.match(designFixProblem(code({ running: true }), c, finding()) ?? '', /Wait for it to finish/);
  assert.match(designFixProblem(code({ workspaceId: 'other' }), c, finding()) ?? '', /another project/);
  assert.match(designFixProblem(code({ id: 'other' }), c, finding()) ?? '', /another conversation/);
  assert.match(designFixProblem(code({ mode: 'research' }), c, finding()) ?? '', /Code conversation/);
  assert.match(designFixProblem(null, c, finding()) ?? '', /Code conversation/);
  assert.match(designFixProblem(code(), c, finding({ replace: null })) ?? '', /not mechanical/);
  // An artifact's fix goes to the Creator conversation that builds it.
  const artifact = check([finding()], { workspaceId: null, creationId: 'cr_1', conversationId: 'creator-conv' });
  const creator = code({ id: 'creator-conv', mode: 'research', agentMode: 'creator', workspaceId: null });
  assert.equal(designFixProblem(creator, artifact, finding()), null);
  assert.match(designFixProblem(code(), artifact, finding()) ?? '', /conversation that builds it/);
});

test('the pane sends a design fix only as a review action, after the user saw it', () => {
  const pane = readFileSync(new URL('../src/features/code/ReviewPane.tsx', import.meta.url), 'utf8');
  // The design cards ask through the same confirmation as the review's.
  const design = pane.slice(pane.indexOf("aria-label=\"Design check\""));
  assert.ok(design.includes('onAsk={setPending}'), 'design fixes wait for the user’s yes');
  assert.ok(design.includes('onClick={confirm}'), 'the yes is the pane’s confirm');
  // That confirm is the one place a message is sent, and it is marked as the review's.
  const sends = pane.match(/session\.send\(/g) ?? [];
  assert.equal(sends.length, 1);
  assert.ok(pane.includes('session.send(pending.text, { spaceId: null, spaceName: null }, { fromReview: true })'));
  // Nothing applies a fix by itself.
  for (const forbidden of ['applyEdit', 'writeFile', 'fs.write', 'invoke(\'edit']) {
    assert.ok(!pane.includes(forbidden), forbidden);
  }
});

test('the design check reads UI files only', () => {
  for (const ui of ['index.html', 'a.HTM', 'styles/site.css', 'x.scss', 'y.less', 'App.tsx', 'b.jsx', 'c.vue', 'd.svelte']) {
    assert.ok(isUiFile(ui), ui);
  }
  for (const other of ['main.rs', 'app.ts', 'notes.md', 'vendor/app.min.css', 'README']) {
    assert.ok(!isUiFile(other), other);
  }
});

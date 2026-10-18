/**
 * Feedback opens a prefilled GitHub issue (there is no feedback mailbox), says so,
 * and never puts more in the link than browsers accept. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/feedbackIssue.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  ISSUES_URL,
  MAX_ISSUE_URL_LENGTH,
  SHORTENED_NOTE,
  SHORTENED_NOTE_NOT_COPIED,
  feedbackIssueLink,
  issueBody,
  issueTitle,
} from '../src/lib/feedbackIssue.ts';
import type { FeedbackReport } from '../src/lib/feedbackIssue.ts';

const report = (description: string): FeedbackReport => ({
  kind: 'bug',
  title: '  Esc closes the viewer ',
  description,
  version: '0.0.1',
  platform: 'Win32',
});

/** The title and body a new-issue link carries. */
function fields(url: string): { title: string; body: string } {
  const params = new URL(url).searchParams;
  return { title: params.get('title') ?? '', body: params.get('body') ?? '' };
}

test('a report becomes a new issue on the project with its kind, text and app version', () => {
  const link = feedbackIssueLink(report('Steps: open a PDF, press Ctrl+F, press Esc.'));
  assert.equal(link.shortened, false);
  assert.ok(link.url.startsWith(`${ISSUES_URL}/new?`));
  const { title, body } = fields(link.url);
  assert.equal(title, '[Bug] Esc closes the viewer');
  assert.equal(title, issueTitle(report('')));
  assert.match(body, /^Steps: open a PDF/);
  assert.match(body, /Shodh 0\.0\.1 · Win32$/);
});

test('a long description is shortened to fit the link, at whole characters, and says where the rest is', () => {
  const long = '数据'.repeat(1500) + '😀'.repeat(800);
  const link = feedbackIssueLink(report(long));
  assert.equal(link.shortened, true);
  assert.ok(link.url.length <= MAX_ISSUE_URL_LENGTH);
  const { body } = fields(link.url);
  assert.ok(body.includes(SHORTENED_NOTE));
  assert.ok(long.startsWith(body.slice(0, body.indexOf('…'))));
  assert.ok(body.indexOf('…') > 100);
  const uncopied = fields(feedbackIssueLink(report(long), false).url).body;
  assert.ok(uncopied.includes(SHORTENED_NOTE_NOT_COPIED) && !uncopied.includes('clipboard'));
  assert.equal(issueBody(report(' x ')), 'x\n\n---\nShodh 0.0.1 · Win32');
});

test('the dialog opens the link with the opener plugin, words it honestly, and is a real dialog', () => {
  const source = readFileSync(new URL('../src/components/FeedbackDialog.tsx', import.meta.url), 'utf8');
  assert.match(source, /await openUrl\(link\.url\)/);
  assert.doesNotMatch(source, /window\.open|mailto:|kalki\.app|has been sent/);
  assert.match(source, /Your browser should open a new GitHub issue/);
  for (const attr of ['role="dialog"', 'aria-modal="true"', 'aria-labelledby={headingId}', 'aria-label="Close"']) {
    assert.ok(source.includes(attr), attr);
  }
  assert.match(source, /e\.key === 'Escape'/);
  assert.match(source, /e\.key !== 'Tab'/);
});

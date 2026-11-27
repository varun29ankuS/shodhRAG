/**
 * The run view (features/agent/RunPanel.tsx, runSummary.ts): a run's plan, its steps and,
 * once finished, what it did, in one panel that replaced the plan panel, the folded steps
 * line and the docked plan. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/runPanel.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { initialTranscript } from '../src/features/agent/reducer.ts';
import type { TranscriptState, TranscriptStep } from '../src/features/agent/reducer.ts';
import { runSummary } from '../src/features/agent/runSummary.ts';
import { appDir, source, withoutComments } from './jsxScan.ts';

function step(id: string, tool: string, label: string, status: TranscriptStep['status'] = 'done', startedAtMs = 0): TranscriptStep {
  return {
    id, parentId: null, tool, label, args: {}, tier: 'read', startedAtMs, status,
    summary: null, detail: null, durationMs: 10, progress: null, approval: null, children: [],
  } as TranscriptStep;
}

function transcript(over: Partial<TranscriptState>): TranscriptState {
  return { ...initialTranscript('run-1', 0), ...over };
}

const passage = (n: number, file: string, path: string) => ({ n, file, path, page: null, heading: null, score: 1, text: '' });

test('a running run has no summary yet', () => {
  assert.equal(runSummary(transcript({ status: 'running', passages: [passage(1, 'a.pdf', 'c:/a.pdf')] })), null);
  assert.equal(runSummary(transcript({ status: 'error' })), null);
});

test('a finished run says what it read, changed, ran and left open', () => {
  const summary = runSummary(transcript({
    status: 'completed',
    passages: [passage(2, 'b.pdf', 'c:/b.pdf'), passage(1, 'a.pdf', 'c:/a.pdf'), passage(3, 'a.pdf', 'c:/a.pdf')],
    steps: {
      s1: step('s1', 'search_documents', 'Searched the lease', 'done', 1),
      s2: step('s2', 'edit_file', 'Edited notes.md', 'done', 2),
      s3: step('s3', 'run_command', 'Ran cargo test', 'done', 3),
      s4: step('s4', 'edit_file', 'Edited draft.md', 'failed', 4),
      s5: { ...step('s5', 'write_file', 'Wrote child.md', 'done', 5), parentId: 's2' },
    },
    plan: [
      { id: '1', text: 'Find the notice period', status: 'done', need: true, coverage: 'covered', evidence: [1] },
      { id: '2', text: 'Find the renewal fee', status: 'done', need: true, coverage: 'missing', evidence: [] },
      { id: '3', text: 'Email the landlord', status: 'blocked', need: false, coverage: null, evidence: [], reason: 'no email tool' },
    ],
  }));
  assert.ok(summary);
  // One per source file, by its first passage, in passage order.
  assert.deepEqual(summary.sources, [{ n: 1, label: 'a.pdf' }, { n: 2, label: 'b.pdf' }]);
  // Only top-level steps that finished: a failed edit and a sub-step are not changes.
  assert.deepEqual(summary.changed, ['Edited notes.md']);
  assert.deepEqual(summary.commands, ['Ran cargo test']);
  assert.deepEqual(summary.open, ['Find the renewal fee: not found', 'Email the landlord: no email tool']);
});

test('a finished run that did nothing to report has no summary', () => {
  assert.equal(runSummary(transcript({ status: 'completed' })), null);
});

test('the run view is one line while closed, open while the run goes on, closed when it ends', () => {
  const panel = withoutComments(source('features/agent/RunPanel.tsx'));
  assert.match(panel, /const \[open, setOpen\] = useState\(live\);/);
  assert.match(panel, /useEffect\(\(\) => \{\s*setOpen\(live\);\s*\}, \[live\]\);/);
  // Its header wraps; nothing in it is cut.
  assert.doesNotMatch(panel, /\btruncate\b|line-clamp/);
  // Summary first, then the plan, then the steps.
  const at = (text: string) => panel.indexOf(text);
  assert.ok(at('<Section title="Summary">') < at('<Section title="Plan">'));
  assert.ok(at('<Section title="Plan">') < at('<Section title="Steps">'));
  // A source opens its passage, as a citation does.
  assert.match(panel, /onClick=\{e => onOpenCitation\(hit, e\.currentTarget\)\}/);
});

test('the plan panel and the docked plan are gone: the run view holds the plan', () => {
  assert.ok(!existsSync(join(appDir, 'src', 'features', 'agent', 'PlanPanel.tsx')));
  const ask = withoutComments(source('features/ask/AskView.tsx'));
  assert.doesNotMatch(ask, /PlanPanel|inlinePlan|planDocked/);
  assert.doesNotMatch(source('features/ask/wideBlock.ts'), /PLAN_DOCK|planDocks/);
});

test('a done plan item strikes its own words only; what it found is cited with pills', () => {
  const panel = withoutComments(source('features/agent/RunPanel.tsx'));
  // The struck span holds the item's text alone; coverage and the blocked reason follow it.
  assert.match(panel, /<span className=\{cn\('break-words leading-\[1\.45\]', TEXT_STYLE\[item\.status\]\)\}>\s*\{item\.text\}\s*<span className="sr-only">\{` \(\$\{STATUS_TEXT\[item\.status\]\}\)`\}<\/span>\s*<\/span>\s*<NeedCoverage/);
  // Each passage it was found in is a pill that opens the passage, not "[1]" as text.
  assert.match(panel, /\{item\.evidence\.map\(n => <CitationPill key=\{n\} n=\{n\} hits=\{hits\} onOpenCitation=\{onOpenCitation\} \/>\)\}/);
  assert.doesNotMatch(panel, /`\[\$\{n\}\]`/);
});

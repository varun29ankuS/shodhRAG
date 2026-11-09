/**
 * Design checks as the code pane shows them (`design_check_commands.rs`,
 * `shodh_rag::design_check`): Shodh's deterministic check of UI files, its findings
 * turned into the review pane's finding cards, the badge words ("Design check: 3
 * issues"), and when a mechanical fix can be sent to the conversation.
 *
 * A fix is never applied here: the pane shows the exact message first and sends it as
 * a review action, so the conversation's edit asks the user in every mode.
 *
 * Pure (type-only imports, no enums) so it is unit-tested directly with Node
 * (`app/tests/designCheck.test.ts`).
 */

import type { Finding, Severity, TargetConversation } from './reviewModel';

export type DesignSeverity = 'error' | 'warning' | 'advisory';

export interface DesignReplacement {
  path: string;
  old: string;
  new: string;
}

export interface DesignFinding {
  rule: string;
  severity: DesignSeverity;
  origin: 'impeccable' | 'shodh';
  title: string;
  path: string;
  line: number;
  endLine: number;
  explanation: string;
  evidence: string;
  fix: string;
  replace: DesignReplacement | null;
}

export interface DesignReport {
  findings: DesignFinding[];
  counts: { error: number; warning: number; advisory: number };
  filesChecked: number;
  skipped: { path: string; reason: string }[];
  leftOut: number;
  partial: boolean;
  rules: number;
}

export interface StoredDesignCheck {
  id: string;
  createdAt: string;
  target: { kind: 'folder' | 'file' | 'changes' | 'artifact'; label: string; path: string };
  workspaceId: string | null;
  creationId: string | null;
  conversationId: string | null;
  byAgent: boolean;
  automatic: boolean;
  report: DesignReport;
}

/** What `design-check-updated` says. */
export interface DesignCheckUpdate {
  id: string;
  conversationId: string | null;
  creationId: string | null;
  workspaceId: string | null;
  total: number;
}

/** Every finding of a report, listed or not. */
export function totalIssues(report: DesignReport): number {
  return report.counts.error + report.counts.warning + report.counts.advisory;
}

/** "Design check: 3 issues", "Design check: 1 issue", "Design check: No issues". */
export function badgeText(check: StoredDesignCheck | null): string {
  if (!check) return 'Design check';
  const n = totalIssues(check.report);
  if (n === 0) return 'Design check: No issues';
  return `Design check: ${n} ${n === 1 ? 'issue' : 'issues'}`;
}

/** The badge's longer description, for its title and screen readers. */
export function badgeDetail(check: StoredDesignCheck): string {
  const { counts, filesChecked } = check.report;
  const files = `${filesChecked} UI ${filesChecked === 1 ? 'file' : 'files'}`;
  const parts: string[] = [];
  if (counts.error) parts.push(`${counts.error} ${counts.error === 1 ? 'error' : 'errors'}`);
  if (counts.warning) parts.push(`${counts.warning} ${counts.warning === 1 ? 'warning' : 'warnings'}`);
  if (counts.advisory) parts.push(`${counts.advisory} to consider`);
  const found = parts.length ? parts.join(', ') : 'no issues';
  return `${check.automatic ? 'Checked after the last change' : 'Checked'}: ${files} of ${check.target.label}, ${found}. No model is used.`;
}

/** How a design severity shows on the review pane's cards. */
export const SEVERITY_OF: Record<DesignSeverity, Severity> = {
  error: 'high',
  warning: 'medium',
  advisory: 'low',
};

/** A design finding as the review pane's finding card shows it. */
export function asReviewFinding(finding: DesignFinding): Finding {
  return {
    severity: SEVERITY_OF[finding.severity],
    title: `${finding.title} (${finding.rule})`,
    citation: { path: finding.path, line: finding.line, endLine: Math.max(finding.line, finding.endLine), side: 'new' },
    scenario: finding.evidence ? `${finding.explanation} Found: ${finding.evidence}` : finding.explanation,
    fix: finding.fix,
    replace: finding.replace,
    edges: [],
    verify: null,
  };
}

/**
 * Why a check's mechanical fix cannot be sent to the open conversation now, or null:
 * only to a Code conversation (Creator included) of the check's project or creation
 * (the very conversation it belongs to, when it has one), not in Plan mode, not while
 * an answer runs, and only a fix with an exact replacement.
 */
export function designFixProblem(
  target: TargetConversation | null,
  check: Pick<StoredDesignCheck, 'workspaceId' | 'creationId' | 'conversationId'>,
  finding: Pick<DesignFinding, 'replace'>,
): string | null {
  if (!finding.replace) return 'This fix is not mechanical: Shodh cannot write it as one exact edit. Ask the conversation to make it.';
  if (check.creationId) {
    // A creation is changed only by the Creator conversation that builds it.
    if (!target || !check.conversationId || target.id !== check.conversationId) {
      return 'Fixes to an artifact go to the conversation that builds it. Open it to send one.';
    }
  } else {
    if (!target || target.mode !== 'code') return 'Fixes go to a Code conversation of this project. Open one to send them.';
    if (!check.workspaceId) return 'This check is not about a project folder, so nothing can be changed for it.';
    if (target.workspaceId !== check.workspaceId) return 'The open conversation works on another project. Open a Code conversation of this one.';
    if (check.conversationId && target.id !== check.conversationId) return 'This check came from another conversation. Open it to send the fix there.';
  }
  if (target.agentMode === 'plan') return 'Plan mode only reads: edits do not run. Pick another mode to send a fix.';
  if (target.running) return 'An answer is running in the conversation. Wait for it to finish.';
  return null;
}

/** The finding count of a check per file, worst first: for the pane's summary line. */
export function filesWithIssues(report: DesignReport): number {
  return new Set(report.findings.map(f => f.path)).size;
}

/** Whether `path` is a file the design check reads (by its extension). */
export function isUiFile(path: string): boolean {
  const name = path.toLowerCase().split('/').pop() ?? '';
  if (name.includes('.min.')) return false;
  return /\.(html?|css|scss|less|jsx|tsx|vue|svelte)$/.test(name);
}

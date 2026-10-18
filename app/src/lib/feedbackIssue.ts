/**
 * Feedback is filed as a GitHub issue on the project: the dialog opens a new issue
 * in the browser with the report filled in, and the person submits it there. There
 * is no feedback mailbox for Shodh, so nothing is sent from the app. GitHub issues
 * are public; the report carries the app version and platform only. No imports:
 * unit-tested under Node.
 */

export const ISSUES_URL = 'https://github.com/varun29ankuS/shodhRAG/issues';

/** Kept below what browsers and GitHub accept for a prefilled new-issue link. */
export const MAX_ISSUE_URL_LENGTH = 7_000;

export type FeedbackKind = 'bug' | 'feedback' | 'feature';

export const FEEDBACK_KIND_LABEL: Record<FeedbackKind, string> = {
  bug: 'Bug',
  feedback: 'Feedback',
  feature: 'Feature request',
};

export interface FeedbackReport {
  kind: FeedbackKind;
  title: string;
  description: string;
  version: string;
  platform: string;
}

/** Closes a shortened description whose full text was copied to the clipboard. */
export const SHORTENED_NOTE = '[Shortened to fit the link. The full text is on your clipboard: paste it here.]';
/** Closes a shortened description when the clipboard could not take the full text. */
export const SHORTENED_NOTE_NOT_COPIED = '[Shortened to fit the link.]';

export function issueTitle(report: FeedbackReport): string {
  return `[${FEEDBACK_KIND_LABEL[report.kind]}] ${report.title.trim()}`;
}

/** The issue's text with `description` (the whole report's, or a shortened one). */
export function issueBody(report: FeedbackReport, description = report.description.trim()): string {
  return `${description}\n\n---\nShodh ${report.version} · ${report.platform}`;
}

function linkFor(title: string, body: string): string {
  return `${ISSUES_URL}/new?title=${encodeURIComponent(title)}&body=${encodeURIComponent(body)}`;
}

export interface FeedbackLink {
  url: string;
  /** The description was shortened to fit the link; the full body goes to the clipboard. */
  shortened: boolean;
}

/**
 * The new-issue link for `report`, its description shortened when the link would be
 * too long; `fullTextOnClipboard` says whether the shortened issue may point to the
 * clipboard for the rest.
 */
export function feedbackIssueLink(report: FeedbackReport, fullTextOnClipboard = true): FeedbackLink {
  const title = issueTitle(report);
  const full = linkFor(title, issueBody(report));
  if (full.length <= MAX_ISSUE_URL_LENGTH) return { url: full, shortened: false };
  const note = fullTextOnClipboard ? SHORTENED_NOTE : SHORTENED_NOTE_NOT_COPIED;
  // Whole characters (code points): a cut inside a surrogate pair cannot be encoded.
  const chars = Array.from(report.description.trim());
  const shortenedTo = (count: number) => linkFor(title, issueBody(report, `${chars.slice(0, count).join('')}…\n\n${note}`));
  // The longest prefix of the description whose link fits (encoded length grows with it).
  let low = 0;
  let high = chars.length;
  while (low < high) {
    const mid = Math.ceil((low + high) / 2);
    if (shortenedTo(mid).length <= MAX_ISSUE_URL_LENGTH) low = mid;
    else high = mid - 1;
  }
  return { url: shortenedTo(low), shortened: true };
}

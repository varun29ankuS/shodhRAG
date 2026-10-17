import React, { useEffect, useId, useRef, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { useTheme } from '../contexts/ThemeContext';
import { Bug, ExternalLink, MessageSquare, X } from 'lucide-react';
import { notify } from '../lib/notify';
import { getAppInfo } from '../lib/version';
import { captureError, addBreadcrumb } from '../lib/errorReporting';
import { FEEDBACK_KIND_LABEL, ISSUES_URL, feedbackIssueLink, issueBody } from '../lib/feedbackIssue';
import type { FeedbackKind, FeedbackReport } from '../lib/feedbackIssue';

interface FeedbackDialogProps {
  isOpen: boolean;
  onClose: () => void;
  type?: 'bug' | 'feedback';
}

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

const HEADING: Record<FeedbackKind, string> = {
  bug: 'Report a bug',
  feedback: 'Send feedback',
  feature: 'Request a feature',
};

/** What happened when the issue link was opened. */
interface Opened {
  /** The description was shortened to fit the link. */
  shortened: boolean;
  /** The full text is on the clipboard (only when shortened). */
  copied: boolean;
}

async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/**
 * Feedback, bug reports and feature requests become a GitHub issue: the browser
 * opens a new issue with the report filled in, and the person submits it there
 * (see `lib/feedbackIssue`). Nothing is sent from the app.
 */
export function FeedbackDialog({ isOpen, onClose, type = 'feedback' }: FeedbackDialogProps) {
  const { colors } = useTheme();
  const [feedbackType, setFeedbackType] = useState<FeedbackKind>(type);
  const [title, setTitle] = useState('');
  const [description, setDescription] = useState('');
  const [opening, setOpening] = useState(false);
  const [opened, setOpened] = useState<Opened | null>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const titleInputRef = useRef<HTMLInputElement>(null);
  const doneRef = useRef<HTMLButtonElement>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);
  const headingId = useId();
  const noteId = useId();
  const titleId = useId();
  const descriptionId = useId();

  // Focus moves into the dialog when it opens and back to where it was when it closes.
  useEffect(() => {
    if (!isOpen) return;
    returnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    titleInputRef.current?.focus();
    return () => {
      const back = returnFocusRef.current;
      returnFocusRef.current = null;
      if (back && back.isConnected) back.focus();
    };
  }, [isOpen]);

  useEffect(() => {
    if (opened) doneRef.current?.focus();
  }, [opened]);

  // The text is kept when the dialog is cancelled; it is cleared once it went to GitHub.
  const close = () => {
    if (opened) {
      setOpened(null);
      setTitle('');
      setDescription('');
    }
    onClose();
  };

  const openIssue = async () => {
    const info = getAppInfo();
    const report: FeedbackReport = {
      kind: feedbackType,
      title,
      description,
      version: info.version,
      platform: info.platform,
    };
    setOpening(true);
    addBreadcrumb('Feedback issue opened', 'feedback', { type: feedbackType });
    try {
      let link = feedbackIssueLink(report);
      let copied = false;
      if (link.shortened) {
        copied = await copyText(issueBody(report));
        if (!copied) link = feedbackIssueLink(report, false);
      }
      await openUrl(link.url);
      setOpened({ shortened: link.shortened, copied });
    } catch (error) {
      console.error('Failed to open the feedback issue:', error);
      captureError(error instanceof Error ? error : new Error(String(error)), { context: 'feedback_issue' });
      notify.error('Your browser could not be opened', {
        description: `Your text is still here. You can report it at ${ISSUES_URL}.`,
      });
    } finally {
      setOpening(false);
    }
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!title.trim() || !description.trim()) {
      notify.error('Add a title and a description');
      return;
    }
    void openIssue();
  };

  // Esc closes the dialog, marked handled so an answer running behind it keeps
  // going; Tab stays inside it.
  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      close();
      return;
    }
    if (e.key !== 'Tab' || !dialogRef.current) return;
    const focusables = Array.from(dialogRef.current.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
      el => el.getClientRects().length > 0,
    );
    if (focusables.length === 0) return;
    const first = focusables[0];
    const last = focusables[focusables.length - 1];
    const active = document.activeElement;
    if (e.shiftKey && (active === first || !dialogRef.current.contains(active))) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && active === last) {
      e.preventDefault();
      first.focus();
    }
  };

  if (!isOpen) return null;

  const fieldStyle = { background: colors.inputBg, borderColor: colors.border, color: colors.text };
  const focusRing = 'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring';

  return (
    <>
      <div className="fixed inset-0 z-50" style={{ background: 'rgba(0, 0, 0, 0.5)' }} onClick={close} aria-hidden="true" />

      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={headingId}
        aria-describedby={noteId}
        onKeyDown={onKeyDown}
        className="fixed top-1/2 left-1/2 transform -translate-x-1/2 -translate-y-1/2 z-50 w-full max-w-2xl"
      >
        <div className="rounded-xl shadow-2xl border-2 overflow-hidden" style={{ background: colors.bg, borderColor: colors.border }}>
          <div className="flex items-center justify-between p-6 border-b" style={{ background: colors.bgSecondary, borderColor: colors.border }}>
            <div className="flex items-center gap-3">
              {feedbackType === 'bug' ? (
                <Bug className="w-6 h-6" style={{ color: colors.error }} aria-hidden="true" />
              ) : (
                <MessageSquare className="w-6 h-6" style={{ color: colors.primary }} aria-hidden="true" />
              )}
              <h2 id={headingId} className="text-xl font-bold" style={{ color: colors.text }}>
                {HEADING[feedbackType]}
              </h2>
            </div>
            <button
              type="button"
              onClick={close}
              className={`p-2 rounded-lg transition-all hover:opacity-70 ${focusRing}`}
              style={{ background: colors.bgTertiary }}
              aria-label="Close"
              title="Close (Esc)"
            >
              <X className="w-5 h-5" style={{ color: colors.text }} aria-hidden="true" />
            </button>
          </div>

          {opened ? (
            <div className="p-6 space-y-4">
              <p id={noteId} role="status" className="m-0 text-[15px]" style={{ color: colors.text }}>
                Your browser should open a new GitHub issue with your report. Submit it there to send it.
              </p>
              {opened.shortened && (
                <p className="m-0 text-sm" style={{ color: colors.textMuted }}>
                  {opened.copied
                    ? 'Your description was too long for the link, so the issue holds the start of it. The full text is on your clipboard: paste it into the issue.'
                    : 'Your description was too long for the link, so the issue holds the start of it, and the full text could not be copied. Copy it from here before you close this.'}
                </p>
              )}
              {opened.shortened && !opened.copied && (
                <textarea
                  readOnly
                  value={description}
                  aria-label="Your full description"
                  rows={5}
                  className="w-full px-4 py-3 rounded-lg border-2 resize-none focus:outline-none focus:ring-2"
                  style={fieldStyle}
                />
              )}
              <p className="m-0 text-sm" style={{ color: colors.textMuted }}>
                {'If nothing opened, report it at '}
                <span className="font-mono">{ISSUES_URL.replace('https://', '')}</span>.
              </p>
              <div className="flex gap-3 justify-end pt-4 border-t" style={{ borderColor: colors.border }}>
                <button
                  type="button"
                  onClick={() => void openIssue()}
                  disabled={opening}
                  className={`px-6 py-3 rounded-lg font-medium transition-all border-2 flex items-center gap-2 disabled:opacity-50 ${focusRing}`}
                  style={{ background: 'transparent', borderColor: colors.border, color: colors.text }}
                >
                  <ExternalLink className="w-4 h-4" aria-hidden="true" />
                  Open again
                </button>
                <button
                  ref={doneRef}
                  type="button"
                  onClick={close}
                  className={`px-6 py-3 rounded-lg font-medium transition-all ${focusRing}`}
                  style={{ background: colors.primary, color: '#ffffff' }}
                >
                  Done
                </button>
              </div>
            </div>
          ) : (
            <form onSubmit={handleSubmit} className="p-6 space-y-6">
              <div className="flex gap-3" role="radiogroup" aria-label="Kind of report">
                {(['bug', 'feedback', 'feature'] as const).map((t) => (
                  <button
                    key={t}
                    type="button"
                    role="radio"
                    aria-checked={feedbackType === t}
                    onClick={() => setFeedbackType(t)}
                    className={`flex-1 py-2 px-4 rounded-lg border-2 transition-all text-sm font-medium ${focusRing}`}
                    style={{
                      background: feedbackType === t ? colors.primary : colors.bgSecondary,
                      borderColor: feedbackType === t ? colors.primary : colors.border,
                      color: feedbackType === t ? '#ffffff' : colors.text,
                    }}
                  >
                    {FEEDBACK_KIND_LABEL[t]}
                  </button>
                ))}
              </div>

              <div>
                <label htmlFor={titleId} className="block text-sm font-medium mb-2" style={{ color: colors.text }}>
                  Title <span style={{ color: colors.error }} aria-hidden="true">*</span>
                </label>
                <input
                  ref={titleInputRef}
                  id={titleId}
                  type="text"
                  value={title}
                  onChange={(e) => setTitle(e.target.value)}
                  placeholder={feedbackType === 'bug' ? 'Brief description of the bug' : 'What would you like to share?'}
                  maxLength={200}
                  className="w-full px-4 py-3 rounded-lg border-2 transition-all focus:outline-none focus:ring-2"
                  style={fieldStyle}
                  required
                />
              </div>

              <div>
                <label htmlFor={descriptionId} className="block text-sm font-medium mb-2" style={{ color: colors.text }}>
                  Description <span style={{ color: colors.error }} aria-hidden="true">*</span>
                </label>
                <textarea
                  id={descriptionId}
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                  placeholder={feedbackType === 'bug' ? 'Steps to reproduce, expected vs actual behavior...' : 'Tell us more...'}
                  rows={6}
                  className="w-full px-4 py-3 rounded-lg border-2 transition-all focus:outline-none focus:ring-2 resize-none"
                  style={fieldStyle}
                  required
                />
              </div>

              <p id={noteId} className="m-0 text-xs" style={{ color: colors.textMuted }}>
                This opens a new issue on GitHub in your browser, with your report and the app version filled in; you
                submit it there. GitHub issues are public, so leave out anything private.
              </p>

              <div className="flex gap-3 justify-end pt-4 border-t" style={{ borderColor: colors.border }}>
                <button
                  type="button"
                  onClick={close}
                  className={`px-6 py-3 rounded-lg font-medium transition-all border-2 ${focusRing}`}
                  style={{ background: 'transparent', borderColor: colors.border, color: colors.text }}
                >
                  Cancel
                </button>
                <button
                  type="submit"
                  disabled={opening || !title.trim() || !description.trim()}
                  className={`px-6 py-3 rounded-lg font-medium transition-all flex items-center gap-2 disabled:opacity-50 ${focusRing}`}
                  style={{ background: colors.primary, color: '#ffffff' }}
                >
                  <ExternalLink className="w-4 h-4" aria-hidden="true" />
                  {opening ? 'Opening GitHub…' : 'Open as a GitHub issue'}
                </button>
              </div>
            </form>
          )}
        </div>
      </div>
    </>
  );
}

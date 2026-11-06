import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertOctagon,
  AlertTriangle,
  BookOpen,
  CheckCircle2,
  Copy,
  Info,
  Loader2,
  Play,
  ScanEye,
  ShieldAlert,
  Wrench,
  X,
  XCircle,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { ariaKeyShortcuts, IS_MAC_PLATFORM } from '../../lib/shortcuts';
import { Kbd } from '../../components/ui/Kbd';
import { useTheme } from '../../contexts/ThemeContext';
import { useChatSession } from '../ask/ChatSessionContext';
import { useDefaultAgentMode } from '../../hooks/useDefaultAgentMode';
import { resolveAgentMode } from '../agent/agentModes';
import { MermaidBlock } from '../ask/visual/MermaidBlock';
import { CodeDiff, CodeFileView } from './CodeDiff';
import { useCodeReview } from './CodeReviewContext';
import type { ReviewSubject } from './CodeReviewContext';
import { codeApi } from './api';
import { languageOf, textAsChange } from './diffModel';
import {
  REVIEW_CHECKS,
  SEVERITY_LABEL,
  applyFixMessage,
  checkMessage,
  citationSelection,
  citationText,
  conversationProblem,
  pieceFromLines,
  piecesFromChanges,
  reviewedWords,
  verification,
} from './reviewModel';
import type { Finding, ReviewCheck, ReviewRequest, Severity, StoredReview } from './reviewModel';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';
const QUIET_BUTTON = cn(
  'inline-flex items-center gap-1.5 h-7 px-2 rounded-md text-caption font-medium text-shodh-text-secondary bg-shodh-raised-2 hover:bg-shodh-pressed hover:text-shodh-text transition-colors duration-micro disabled:opacity-50 disabled:cursor-not-allowed',
  FOCUS_RING,
);

const SEVERITY_ICON: Record<Severity, LucideIcon> = {
  critical: AlertOctagon,
  high: ShieldAlert,
  medium: AlertTriangle,
  low: Info,
};

const SEVERITY_TONE: Record<Severity, string> = {
  critical: 'text-shodh-error border-shodh-error/50',
  high: 'text-shodh-warning border-shodh-warning/60',
  medium: 'text-shodh-text-secondary border-shodh-border-strong',
  low: 'text-shodh-text-muted border-shodh-border',
};

/** The folder of `path` ("." at the top). */
function folderOf(path: string): string {
  const at = path.lastIndexOf('/');
  return at > 0 ? path.slice(0, at) : '.';
}

/** The request for `subject` as the pane shows it, or why there is none. */
function requestFor(
  subject: ReviewSubject,
  check: ReviewCheck,
  notes: { question: string; flows: string; context: string; folder: string },
  canVerify: boolean,
): ReviewRequest | string {
  const note = (text: string) => (text.trim() ? text.trim() : null);
  const base: ReviewRequest = {
    check,
    pieces: [],
    question: note(notes.question),
    folder: null,
    flows: null,
    context: note(notes.context),
    canVerify,
  };
  if (check === 'custom' && !base.question) return 'Type what the reviewer should look at.';
  if (check === 'architecture') {
    const folder = subject.kind === 'folder' ? subject.folder : notes.folder.trim() || '.';
    if (!subject.workspaceId) return 'An architecture review needs a project: open this from a workspace with a code folder.';
    return { ...base, folder, flows: note(notes.flows) };
  }
  if (subject.kind === 'folder') return 'Pick Architecture to review a folder, or open a file or diff.';
  if (subject.kind === 'diff') {
    const pieces = piecesFromChanges(subject.changes, subject.selection, path => subject.language ?? languageOf(path));
    return pieces.length ? { ...base, pieces } : 'There are no lines to review here.';
  }
  const lines = textAsChange(subject.path, subject.text, subject.firstLine).hunks[0]?.lines ?? [];
  const chosen = subject.selection
    ? lines.filter(l => (l.new ?? 0) >= subject.selection!.from && (l.new ?? 0) <= subject.selection!.to)
    : lines;
  return chosen.length ? { ...base, pieces: [pieceFromLines(subject.path, subject.language, chosen)] } : 'There are no lines to review here.';
}

function FindingCard({
  finding,
  index,
  review,
  problem,
  onCite,
  onApply,
  onCheck,
}: {
  finding: Finding;
  index: number;
  review: StoredReview;
  problem: string | null;
  onCite: (finding: Finding) => void;
  onApply: (finding: Finding) => void;
  onCheck: (finding: Finding, index: number) => void;
}) {
  const pane = useCodeReview();
  const run = pane?.runs[`${review.id}:${index}`] ?? null;
  const status = verification(finding.verify, run);
  const Icon = SEVERITY_ICON[finding.severity];
  const StatusIcon = status.status === 'confirmed' ? CheckCircle2 : status.status === 'not_confirmed' ? XCircle : status.status === 'pending' ? Loader2 : BookOpen;
  const copyFix = async () => {
    try {
      await navigator.clipboard.writeText(finding.replace?.new ?? finding.fix);
      notify.success('Fix copied');
    } catch (e) {
      notify.error('The fix could not be copied', { description: String(e) });
    }
  };
  return (
    <li className="rounded-lg border border-shodh-border-subtle bg-shodh-surface px-3 py-2.5 flex flex-col gap-1.5">
      <div className="flex items-start gap-2">
        <span className={cn('inline-flex items-center gap-1 shrink-0 h-6 px-1.5 rounded-md border text-caption font-semibold', SEVERITY_TONE[finding.severity])}>
          <Icon className="w-3.5 h-3.5" aria-hidden="true" />
          {SEVERITY_LABEL[finding.severity]}
        </span>
        <p className="m-0 min-w-0 text-small font-semibold text-shodh-text break-words">{finding.title}</p>
      </div>
      {finding.citation ? (
        <button
          type="button"
          onClick={() => onCite(finding)}
          className={cn('self-start inline-flex items-center h-6 px-1.5 rounded-md bg-shodh-raised-2 font-mono text-caption text-shodh-accent-text hover:bg-shodh-pressed', FOCUS_RING)}
          aria-label={`Show ${citationText(finding.citation)}`}
        >
          {citationText(finding.citation)}
        </button>
      ) : (
        <p className="m-0 text-caption text-shodh-text-muted">No line given.</p>
      )}
      {finding.scenario && (
        <p className="m-0 text-small text-shodh-text-secondary">
          <span className="font-semibold text-shodh-text">What goes wrong: </span>
          {finding.scenario}
        </p>
      )}
      {finding.fix && (
        <p className="m-0 text-small text-shodh-text-secondary">
          <span className="font-semibold text-shodh-text">Suggested fix: </span>
          {finding.fix}
        </p>
      )}
      <div className="flex flex-wrap items-center gap-1.5">
        {finding.replace && (
          <button
            type="button"
            onClick={() => onApply(finding)}
            disabled={problem !== null}
            title={problem ?? 'Sends the fix to the Code conversation as one edit: it asks for approval like any other edit, as the conversation’s mode decides.'}
            className={QUIET_BUTTON}
          >
            <Wrench className="w-3.5 h-3.5" aria-hidden="true" />
            Apply fix
          </button>
        )}
        <button type="button" onClick={() => void copyFix()} className={QUIET_BUTTON}>
          <Copy className="w-3.5 h-3.5" aria-hidden="true" />
          Copy fix
        </button>
      </div>
      <div className="flex flex-col gap-1 rounded-md bg-shodh-raised px-2 py-1.5">
        <p className={cn('m-0 inline-flex items-start gap-1.5 text-caption', status.status === 'confirmed' ? 'text-shodh-success' : status.status === 'not_confirmed' ? 'text-shodh-text' : 'text-shodh-text-muted')}>
          <StatusIcon className={cn('w-3.5 h-3.5 mt-px shrink-0', status.status === 'pending' && 'animate-spin')} aria-hidden="true" />
          <span>{status.label}</span>
        </p>
        {finding.verify && (
          <>
            <p className="m-0 text-caption text-shodh-text-muted">Check proposed by the reviewer:</p>
            <pre className="m-0 px-2 py-1 rounded bg-shodh-surface font-mono text-caption text-shodh-text whitespace-pre-wrap break-all">{finding.verify.command}</pre>
            <div className="flex items-center gap-1.5">
              <button
                type="button"
                onClick={() => onCheck(finding, index)}
                disabled={problem !== null || status.status === 'pending'}
                title={problem ?? 'Sends the command to the Code conversation: it runs only as the conversation’s mode and Shodh’s command rules allow, asking first where they say so.'}
                className={QUIET_BUTTON}
              >
                <Play className="w-3.5 h-3.5" aria-hidden="true" />
                {run ? 'Run the check again' : 'Run this check'}
              </button>
            </div>
            {run && run.output && (
              <details>
                <summary className="cursor-pointer text-caption text-shodh-text-secondary">What it printed</summary>
                <pre className="m-0 mt-1 px-2 py-1 max-h-48 overflow-auto scrollbar-thin rounded bg-shodh-surface font-mono text-caption text-shodh-text whitespace-pre-wrap break-words">{run.output.slice(-6000)}</pre>
              </details>
            )}
          </>
        )}
      </div>
    </li>
  );
}

/**
 * The code pane: the code or diff with the selection highlighted, the checks, and the
 * fresh-eyes review with its ranked findings, citations that jump to their lines,
 * fixes that go to the Code conversation as edits, and checks that run there.
 */
export function ReviewPane() {
  const pane = useCodeReview();
  const session = useChatSession();
  const defaultMode = useDefaultAgentMode();
  const { theme } = useTheme();
  const [check, setCheck] = useState<ReviewCheck>('algorithm');
  const [question, setQuestion] = useState('');
  const [flows, setFlows] = useState('');
  const [context, setContext] = useState('');
  const [folder, setFolder] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const closeRef = useRef<HTMLButtonElement>(null);

  const subject = pane?.subject ?? null;
  const review = pane?.review ?? null;
  const conversation = session.activeConversation;
  const conversationMode = conversation?.mode ?? 'research';
  const agentMode = conversation ? resolveAgentMode(conversation.agentMode, conversation.messages.length > 0, defaultMode) : null;
  const workspaceId = subject?.workspaceId ?? (conversationMode === 'code' ? conversation?.workspaceId ?? null : null);
  const problem = conversationProblem(
    conversation
      ? { mode: conversationMode, agentMode, workspaceId: conversation.workspaceId ?? null, running: session.isStreaming }
      : null,
    review?.workspaceId ?? workspaceId,
  );
  const canVerify = conversationMode === 'code' && workspaceId !== null && conversation?.workspaceId === workspaceId;

  // A new subject starts from its own defaults.
  useEffect(() => {
    if (!subject) return;
    setError(null);
    if (subject.kind === 'folder') setCheck('architecture');
    setFolder(subject.kind === 'folder' ? subject.folder : subject.kind === 'file' ? folderOf(subject.path) : '.');
    closeRef.current?.focus({ preventScroll: true });
  }, [subject?.title, subject?.kind]);

  const run = useCallback(async () => {
    if (!pane || !subject || busy) return;
    const request = requestFor(subject, check, { question, flows, context, folder }, canVerify);
    if (typeof request === 'string') {
      setError(request);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const found = await codeApi.review(request, workspaceId, conversation?.id ?? null);
      pane.setReview(found);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [pane, subject, busy, check, question, flows, context, folder, canVerify, workspaceId, conversation?.id]);

  // The shortcut pressed while the pane is open runs the review.
  const runRequest = pane?.runRequest ?? 0;
  useEffect(() => {
    if (runRequest > 0) void run();
    // Only a new request runs it.
  }, [runRequest]);

  const send = useCallback(
    (text: string) => session.send(text, { spaceId: null, spaceName: null }, { composed: true }),
    [session],
  );

  const onCite = useCallback(
    (finding: Finding) => {
      if (!pane || !finding.citation) return;
      const selection = citationSelection(finding.citation);
      const shown = subject && subject.kind !== 'folder' && (subject.kind === 'file' ? subject.path === selection.path : subject.changes.some(c => c.path === selection.path));
      if (shown) {
        pane.setSelection(selection);
        return;
      }
      // A file of the project the review cites (an architecture finding): opened at its lines.
      if (workspaceId) {
        const current = pane.review;
        codeApi
          .file(workspaceId, selection.path)
          .then(file => {
            pane.open({ kind: 'file', title: selection.path, path: selection.path, text: file.text, language: languageOf(selection.path), firstLine: 1, workspaceId, selection });
            if (current) pane.setReview(current);
          })
          .catch(e => notify.error('The file could not be opened', { description: String(e) }));
      }
    },
    [pane, subject, workspaceId],
  );

  const onApply = useCallback(
    (finding: Finding) => {
      if (!finding.replace || problem) return;
      if (send(applyFixMessage(finding, finding.replace))) {
        notify.info('Fix sent to the conversation', { description: 'It asks for approval there like any other edit.' });
      }
    },
    [send, problem],
  );

  const onCheck = useCallback(
    (finding: Finding, index: number) => {
      if (!pane || !review || !finding.verify || problem) return;
      if (send(checkMessage(finding, finding.verify))) pane.awaitCheck(`${review.id}:${index}`, finding.verify.command);
    },
    [pane, review, send, problem],
  );

  const changes = useMemo(() => (subject?.kind === 'diff' ? subject.changes : null), [subject]);

  if (!pane || !subject) return null;
  const selection = subject.kind === 'folder' ? null : subject.selection;
  const built = requestFor(subject, check, { question, flows, context, folder }, canVerify);
  const pending = typeof built === 'string' ? null : built;

  return (
    <aside
      aria-label="Code review"
      onKeyDown={e => {
        if (e.key === 'Escape') {
          e.stopPropagation();
          pane.close();
        }
      }}
      className="fixed top-0 right-0 bottom-0 z-40 flex flex-col w-[min(680px,100vw)] border-l border-shodh-border bg-shodh-ground shadow-2xl"
    >
      <header className="flex items-center gap-2 px-4 h-12 shrink-0 border-b border-shodh-border-subtle">
        <ScanEye className="w-4 h-4 text-shodh-text-secondary" aria-hidden="true" />
        <h2 className="m-0 text-small font-semibold text-shodh-text">Code review</h2>
        <p className="m-0 min-w-0 truncate font-mono text-caption text-shodh-text-muted">{subject.title}</p>
        <button ref={closeRef} type="button" onClick={pane.close} className={cn('ml-auto', QUIET_BUTTON)}>
          <X className="w-3.5 h-3.5" aria-hidden="true" />
          Close
        </button>
      </header>
      <div className="flex-1 min-h-0 overflow-y-auto scrollbar-thin px-4 py-3 flex flex-col gap-3">
        {subject.kind === 'diff' && changes && (
          <CodeDiff changes={changes} label={subject.title} selection={selection} onSelect={pane.setSelection} language={subject.language} maxHeight={360} />
        )}
        {subject.kind === 'file' && (
          <CodeFileView
            path={subject.path}
            text={subject.text}
            language={subject.language}
            firstLine={subject.firstLine}
            label={subject.title}
            selection={selection}
            onSelect={pane.setSelection}
            maxHeight={360}
          />
        )}
        {subject.kind === 'folder' && (
          <p className="m-0 text-small text-shodh-text-secondary">
            {`The reviewer gets the structure of ${subject.folder}: its files, modules, imports and public items, as Shodh reads them. No function bodies.`}
          </p>
        )}
        {selection && (
          <p className="m-0 text-caption text-shodh-text-muted">
            {`Selected: ${selection.path}, ${selection.from === selection.to ? `line ${selection.from}` : `lines ${selection.from}–${selection.to}`}. Click a line number to select another, Shift+click to extend.`}
          </p>
        )}

        <section aria-label="Check" className="flex flex-col gap-2">
          <div role="radiogroup" aria-label="What to check" className="flex flex-wrap gap-1.5">
            {REVIEW_CHECKS.map(option => (
              <button
                key={option.id}
                type="button"
                role="radio"
                aria-checked={check === option.id}
                title={option.hint}
                onClick={() => setCheck(option.id)}
                className={cn(
                  'h-7 px-2.5 rounded-md text-caption font-medium border transition-colors duration-micro',
                  check === option.id
                    ? 'border-shodh-accent bg-shodh-accent-soft text-shodh-text'
                    : 'border-shodh-border-subtle text-shodh-text-secondary hover:bg-shodh-raised-2',
                  FOCUS_RING,
                )}
              >
                {option.label}
              </button>
            ))}
          </div>
          {check === 'custom' && (
            <label className="flex flex-col gap-1 text-caption text-shodh-text-secondary">
              What should the reviewer look at?
              <textarea value={question} onChange={e => setQuestion(e.target.value)} rows={2} maxLength={4000} className="rounded-md border border-shodh-border bg-shodh-surface px-2 py-1.5 text-small text-shodh-text" />
            </label>
          )}
          {check === 'architecture' && (
            <>
              {subject.kind !== 'folder' && (
                <label className="flex flex-col gap-1 text-caption text-shodh-text-secondary">
                  Folder to review (in the project; “.” is all of it)
                  <input value={folder} onChange={e => setFolder(e.target.value)} className="h-8 rounded-md border border-shodh-border bg-shodh-surface px-2 font-mono text-small text-shodh-text" />
                </label>
              )}
              <label className="flex flex-col gap-1 text-caption text-shodh-text-secondary">
                Data flows to follow (optional)
                <textarea value={flows} onChange={e => setFlows(e.target.value)} rows={2} maxLength={4000} placeholder="A question goes from the composer to the search and back" className="rounded-md border border-shodh-border bg-shodh-surface px-2 py-1.5 text-small text-shodh-text" />
              </label>
            </>
          )}
          <details>
            <summary className="cursor-pointer text-caption text-shodh-text-secondary">Add context for the reviewer (optional)</summary>
            <textarea
              value={context}
              onChange={e => setContext(e.target.value)}
              rows={3}
              maxLength={4000}
              aria-label="Context for the reviewer"
              placeholder="Only what you type here is added: the reviewer never sees the conversation."
              className="mt-1 w-full rounded-md border border-shodh-border bg-shodh-surface px-2 py-1.5 text-small text-shodh-text"
            />
          </details>
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => void run()}
              disabled={busy}
              aria-keyshortcuts={ariaKeyShortcuts('reviewCode', IS_MAC_PLATFORM)}
              className={cn(
                'inline-flex items-center gap-2 h-8 px-3 rounded-lg text-small font-semibold bg-shodh-accent text-shodh-on-accent hover:bg-shodh-accent-hover disabled:opacity-60',
                FOCUS_RING,
              )}
            >
              {busy ? <Loader2 className="w-4 h-4 animate-spin" aria-hidden="true" /> : <ScanEye className="w-4 h-4" aria-hidden="true" />}
              {busy ? 'Reviewing…' : 'Review this'}
              <Kbd shortcut="reviewCode" onAccent />
            </button>
            <p className="m-0 text-caption text-shodh-text-muted">
              {pending ? `The reviewer sees ${reviewedWords(pending)}, never this conversation.` : 'The reviewer sees only what is selected here, never this conversation.'}
            </p>
          </div>
          {error && (
            <p role="alert" className="m-0 text-small text-shodh-error">
              {error}
            </p>
          )}
        </section>

        {review && (
          <section aria-label="Review" className="flex flex-col gap-2">
            <h3 className="m-0 text-small font-semibold text-shodh-text">
              {`${review.report.findings.length} ${review.report.findings.length === 1 ? 'finding' : 'findings'} · ${REVIEW_CHECKS.find(c => c.id === review.report.check)?.label ?? review.report.check}`}
            </h3>
            {review.report.summary && <p className="m-0 text-small text-shodh-text-secondary">{review.report.summary}</p>}
            <p className="m-0 text-caption text-shodh-text-muted">
              {`${review.byAgent ? 'Asked for by the assistant. ' : ''}The reviewer saw ${reviewedWords(review.request)}${review.request.question || review.request.context || review.request.flows ? ' and your notes' : ''}, nothing else.`}
              {review.report.dropped > 0 ? ` ${review.report.dropped} of its claims pointed at code it was not shown and were left out.` : ''}
            </p>
            {problem && review.report.findings.some(f => f.verify || f.replace) && (
              <p className="m-0 text-caption text-shodh-text-muted">{problem}</p>
            )}
            {review.report.diagram && (
              <figure className="m-0 flex flex-col gap-1">
                <MermaidBlock source={review.report.diagram} dark={theme === 'dark'} />
                <figcaption className="text-caption text-shodh-text-muted">Modules and their imports. Thick red edges labelled “problem” or “cycle” are the ones the findings and Shodh’s cycle check point at.</figcaption>
              </figure>
            )}
            <ol className="m-0 p-0 list-none flex flex-col gap-2">
              {review.report.findings.map((finding, index) => (
                <FindingCard
                  key={`${review.id}-${index}`}
                  finding={finding}
                  index={index}
                  review={review}
                  problem={problem}
                  onCite={onCite}
                  onApply={onApply}
                  onCheck={onCheck}
                />
              ))}
            </ol>
          </section>
        )}
      </div>
    </aside>
  );
}

export default ReviewPane;

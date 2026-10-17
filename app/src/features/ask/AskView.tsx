import React, { Suspense, lazy, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { ArrowUpRight, Check, Copy, CornerLeftUp, FileDown, FileText, FolderPlus, Globe, MessagesSquare, RotateCcw } from 'lucide-react';
import { useTheme } from '../../contexts/ThemeContext';
import { invoke } from '@tauri-apps/api/core';
import { cn } from '../../lib/utils';
import type { ViewTab } from '../../lib/viewTabs';
import { useMediaQuery } from '../../hooks/useMediaQuery';
import { AgentComposer } from '../agent/AgentComposer';
import type { AgentComposerHandle, PastedImage } from '../agent/AgentComposer';
import { passageHits } from '../agent/citations';
import { PlanPanel } from '../agent/PlanPanel';
import { answerText, pendingApproval, isLive } from '../agent/reducer';
import type { ClaimCheck } from '../agent/events';
import type { CorrectionOutcome } from '../agent/grounding';
import { CORRECTION_BUSY, CORRECTION_NOT_RECORDED, CorrectionGuard, applyAuditedCorrection, citationCorrection, citeSuggested, sameCorrection } from '../agent/grounding';
import { recordCitationCorrection } from '../audit/api';
import type { TranscriptState } from '../agent/reducer';
import { RuntimeCard } from '../agent/RuntimeCard';
import { SearchSetupCard } from '../setup/SearchSetupCard';
import { StatusLine } from '../agent/StatusLine';
import { SuggestionsBadge } from '../memory/SuggestionsBadge';
import { requestSettingsSection } from '../memory/navigation';
import { Transcript } from '../agent/Transcript';
import { FocusAnchorProvider, useFocus } from '../focus/FocusContext';
import type { FocusThread } from '../focus/focusTypes';
import { sourceTarget } from '../focus/targets';
import { activeRootThreads, descendantCount, nodeLabel } from '../focus/threadTree';
import { FOCUS_OVERLAY_SELECTOR } from '../focus/focusDom';
import type { SideSummaryRef } from '../focus/summary';
import { UserText } from './UserText';
import { useChatSession } from './ChatSessionContext';
import { useDraft } from './useDraft';
import { useSendOptions } from './useSendOptions';
import { AttachmentChips, useImageAttachments } from './AttachmentChips';
import { attachedContextOf, attachmentLabel, composerAttachments } from './imageAttachments';
import type { FallbackRun } from './ChatSessionContext';
import type { ProviderErrorActions } from '../modelPicker/ProviderErrorCard';
import { MessageContentRenderer } from './MessageContentRenderer';
import { RunChip } from './RunChip';
import { SourcePreview } from './SourcePreview';
import { useNavigationTarget } from '../agent/useNavigationTarget';
import { appRecordKind, citedNumbers, documentHit, fileExtensionOf, formatLocation, groupSources, shownPath, sourceLabel, toSearchHits, webHost } from './searchResults';
import type { SourceGroup } from './searchResults';
import { openExternal } from '../agent/WebSources';
import { notify } from '../../lib/notify';
import { clearReveal, pendingReveal, subscribeReveal } from '../visuals/reveal';
import type { MessageReveal } from '../visuals/reveal';
import { scrollBehavior } from './viewer/sourceAccess';
import type { ChatMessage, SearchHit, SendOptions } from './types';
import { useWorkspaces } from '../workspaces/WorkspaceContext';
import { scopeChip } from '../workspaces/model';
import { librarySourcesChip } from '../library/reconcile';
import type { SearchedSources } from '../library/reconcile';
import { SnippetDropZone } from '../research/SnippetDropZone';
import { exportPdf } from '../print/exportPdf';
import { answerTitle, buildPrintDocument } from '../print/printModel';
import { COMPOSER_INSERT_EVENT, onWindowEvent, takePendingInserts } from '../research/snippetBus';
import { appendToDraft } from '../research/snippetModel';
import { ModelChip } from '../modelPicker/ModelChip';
import { CodeModeSwitch } from '../agent/CodeModeSwitch';
import { DiagramHostContext } from './visual/diagramHost';
import type { ConversationLogHandle } from './ConversationLog';
import { CONVERSATION_TOP_PX, MESSAGE_INDEX_ATTR } from './windowLayout';
import { ToolsChip } from '../tools/ToolsChip';
import type { ConversationMode } from '../../hooks/useConversations';
import { readLibraryKind, starterPrompts } from './starterPrompts';
import { stripTextMarks } from './visual/marks';
import type { LibraryKind } from './starterPrompts';

// The artifact side panel (editors, previews) loads when an artifact is first opened.
// The windowed conversation (and its virtualizer) loads with the first
// conversation that has messages, not at startup.
const ConversationLog = lazy(() => import('./ConversationLog'));
// Find in the conversation loads with the first Ctrl+F.
const ConversationFind = lazy(() => import('./ConversationFind').then(m => ({ default: m.ConversationFind })));
const EnhancedArtifactPanel = lazy(() => import('../../components/EnhancedArtifactPanel').then(m => ({ default: m.EnhancedArtifactPanel })));

/** Room kept below the conversation before the composer is measured. */
const COMPOSER_SPACE_FALLBACK = 190;
/** Space between the last answer's actions and the composer's fade. */
const COMPOSER_GAP = 16;

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

const VISIBLE_SOURCE_CHIPS = 6;

/** Width from which the task list docks to the right of the conversation. */
const WIDE_LAYOUT_QUERY = '(min-width: 1440px)';

/** Citation targets of a message: its run's passages, or legacy search results. */
export function messageHits(message: ChatMessage): SearchHit[] {
  return message.transcript ? passageHits(message.transcript.passages) : toSearchHits(message.searchResults);
}

/** The latest agent transcript in a conversation. */
export function latestTranscript(messages: readonly ChatMessage[]): TranscriptState | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const t = messages[i].transcript;
    if (t) return t;
  }
  return null;
}

export interface AskSource {
  id: string;
  name: string;
  selected: boolean;
  status: string;
}

export interface AskViewProps {
  sources: readonly AskSource[];
  /** What answers search, in indexed files (`searchedSources`, shared with the top bar). */
  searched: SearchedSources;
  llmStatus: { connected: boolean; model: string; provider: string };
  onNavigate: (tab: ViewTab) => void;
  /** Pick an image whose text is read (OCR) and attached to the next question. */
  onPickImage?: () => void;
  /** An image was pasted into the composer: its text is read and attached to the next question. */
  onPasteImage?: (image: PastedImage) => void;
  /** A file is being dragged over the window. */
  isDraggingFile?: boolean;
  dropHandlers?: Pick<React.HTMLAttributes<HTMLElement>, 'onDrop' | 'onDragOver' | 'onDragEnter' | 'onDragLeave'>;
  /** Text another view wants in the composer; applied once per `seq`. */
  draftRequest?: { text: string; seq: number } | null;
  /** The draft request was placed in the composer. */
  onDraftApplied?: () => void;
}

interface PreviewTarget {
  messageId: string;
  hit: SearchHit;
}

/** Preview key for documents the agent opened (not tied to a message). */
const AGENT_DOCUMENT_PREVIEW = 'agent-document';

const FILE_BADGE_CLASS: Record<string, string> = {
  pdf: 'bg-shodh-raised-2 text-shodh-error',
  doc: 'bg-shodh-raised-2 text-shodh-info',
  docx: 'bg-shodh-raised-2 text-shodh-info',
  xls: 'bg-shodh-raised-2 text-shodh-success',
  xlsx: 'bg-shodh-raised-2 text-shodh-success',
  csv: 'bg-shodh-raised-2 text-shodh-success',
  ppt: 'bg-shodh-raised-2 text-shodh-warning',
  pptx: 'bg-shodh-raised-2 text-shodh-warning',
};

function FileBadge({ path }: { path: string }) {
  const record = appRecordKind(path);
  const ext = record ?? fileExtensionOf(path);
  if (!ext) return null;
  const badge = record === 'calendar' ? 'cal' : record ?? ext.slice(0, 4);
  return (
    <span
      className={cn(
        'px-1 py-0.5 rounded text-[9.5px] font-bold uppercase leading-none tracking-wide',
        FILE_BADGE_CLASS[ext] ?? 'bg-shodh-raised-2 text-shodh-text-muted',
      )}
      aria-hidden="true"
    >
      {badge}
    </span>
  );
}

function SourceChips({
  groups,
  activeFile,
  onOpen,
}: {
  groups: SourceGroup[];
  activeFile: string | null;
  onOpen: (hit: SearchHit, trigger: HTMLElement) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  if (groups.length === 0) return null;
  const visible = expanded ? groups : groups.slice(0, VISIBLE_SOURCE_CHIPS);
  const hidden = groups.length - visible.length;

  return (
    <ul className="flex flex-wrap gap-2" aria-label="Sources">
      {visible.map(group => {
        const where = formatLocation(group.primary);
        const isActive = activeFile === group.sourceFile;
        return (
          <li key={group.sourceFile} className="min-w-0 max-w-full">
            <button
              type="button"
              onClick={e => onOpen(group.primary, e.currentTarget)}
              aria-pressed={isActive}
              title={group.primary.url ? `${sourceLabel(group.primary)} — ${group.primary.url} (opens in your browser)` : appRecordKind(group.sourceFile) ? sourceLabel(group.primary) : shownPath(group.primary)}
              className={cn(
                'inline-flex items-center gap-2 h-8 max-w-full pl-2 pr-3 rounded-[10px] border text-shodh-text transition-colors duration-micro',
                isActive
                  ? 'bg-shodh-accent-soft border-shodh-accent/60'
                  : 'bg-shodh-surface border-shodh-border hover:bg-shodh-raised',
                FOCUS_RING,
              )}
            >
              {group.primary.url ? (
                <Globe className="w-3.5 h-3.5 shrink-0 text-shodh-info" aria-hidden="true" />
              ) : (
                <FileBadge path={group.sourceFile} />
              )}
              <span className="text-[12.5px] truncate">{sourceLabel(group.primary)}</span>
              {group.primary.url && (
                <span className="text-[11.5px] text-shodh-text-faint whitespace-nowrap inline-flex items-center gap-0.5">
                  {webHost(group.primary.url)}
                  <ArrowUpRight className="w-3 h-3" aria-hidden="true" />
                </span>
              )}
              {where && <span className="text-[11.5px] text-shodh-text-faint whitespace-nowrap">{where}</span>}
              {group.hits.length > 1 && (
                <span className="text-[11.5px] text-shodh-text-faint whitespace-nowrap">{`· ${group.hits.length} passages`}</span>
              )}
            </button>
          </li>
        );
      })}
      {hidden > 0 && (
        <li>
          <button
            type="button"
            onClick={() => setExpanded(true)}
            className={cn(
              'h-8 px-3 rounded-[10px] text-[12.5px] text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
              FOCUS_RING,
            )}
          >
            {`${hidden} more`}
          </button>
        </li>
      )}
    </ul>
  );
}

function CopyAnswerButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const timerRef = useRef<number | null>(null);
  useEffect(() => () => {
    if (timerRef.current !== null) window.clearTimeout(timerRef.current);
  }, []);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      if (timerRef.current !== null) window.clearTimeout(timerRef.current);
      timerRef.current = window.setTimeout(() => setCopied(false), 2000);
    } catch (error) {
      console.error('Failed to copy answer:', error);
    }
  };
  return (
    <button
      type="button"
      onClick={copy}
      aria-label={copied ? 'Answer copied' : 'Copy answer'}
      title={copied ? 'Copied' : 'Copy'}
      className={cn(
        'w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
        FOCUS_RING,
      )}
    >
      {copied ? <Check className="w-[15px] h-[15px] text-shodh-success" aria-hidden="true" /> : <Copy className="w-[15px] h-[15px]" aria-hidden="true" />}
    </button>
  );
}

/** Export → PDF of one answer, printed as the app shows it, with its sources. */
function ExportPdfButton({ message, question, hits }: { message: ChatMessage; question: string | null; hits: readonly SearchHit[] }) {
  const [busy, setBusy] = useState(false);
  const run = async () => {
    setBusy(true);
    try {
      const fromQuestion = question ? answerTitle(question, '') : '';
      await exportPdf(buildPrintDocument({
        title: fromQuestion || answerTitle(stripTextMarks(message.content)),
        subtitle: fromQuestion ? null : 'Answer from Shodh',
        createdAt: message.timestamp,
        markdown: message.content,
        hits,
      }));
    } finally {
      setBusy(false);
    }
  };
  return (
    <button
      type="button"
      onClick={() => void run()}
      disabled={busy}
      aria-label="Export this answer as PDF"
      title="Export as PDF"
      className={cn(
        'w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-40 disabled:cursor-wait transition-colors duration-micro',
        FOCUS_RING,
      )}
    >
      <FileDown className="w-[15px] h-[15px]" aria-hidden="true" />
    </button>
  );
}

/** "2 replies about Revenue by quarter": reopens that side discussion. */
function ThreadChips({ threads, onOpen }: { threads: readonly FocusThread[]; onOpen: (thread: FocusThread, trigger: HTMLElement) => void }) {
  // Nested (drill-down) discussions open from their root; the chip counts them.
  const withReplies = activeRootThreads(threads);
  if (withReplies.length === 0) return null;
  return (
    <ul className="flex flex-wrap gap-2" aria-label="Side discussions">
      {withReplies.map(thread => (
        <li key={thread.id} className="min-w-0 max-w-full">
          <button
            type="button"
            onClick={e => onOpen(thread, e.currentTarget)}
            title="Reopen this side discussion"
            className={cn(
              'inline-flex items-center gap-1.5 h-7 max-w-full px-2.5 rounded-full border border-shodh-border bg-shodh-surface text-[12px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
              FOCUS_RING,
            )}
          >
            <MessagesSquare className="w-3.5 h-3.5 shrink-0 text-shodh-accent-text" aria-hidden="true" />
            <span className="truncate">{nodeLabel(thread)}</span>
            {(() => {
              const deeper = descendantCount(threads, thread.id);
              return deeper > 0 ? <span className="shrink-0 text-shodh-text-muted">{` · ${deeper} deeper`}</span> : null;
            })()}
          </button>
        </li>
      ))}
    </ul>
  );
}

interface AssistantMessageProps {
  /** Conversation the message belongs to (anchors side discussions). */
  conversationId: string | null;
  message: ChatMessage;
  /** The question this answers (the user message before it), for the PDF title. */
  question: string | null;
  activeCitation: number | null;
  activeFile: string | null;
  canRetry: boolean;
  /** The newest message of the conversation: a provider failure may retry or fall back by itself. */
  latest: boolean;
  /** Task list shown inline above the answer (narrow layouts). */
  inlinePlan: boolean;
  onOpenSource: (messageId: string, hit: SearchHit, trigger: HTMLElement) => void;
  onOpenArtifact: (artifactId: string) => void;
  onRetry: (message: ChatMessage, fallback?: FallbackRun | null) => void;
  onDecide: (stepId: string, approved: boolean) => void;
  onRuntimeInstalled: () => void;
  onOpenSettings: () => void;
  onOpenThread: (messageId: string, thread: FocusThread, trigger: HTMLElement) => void;
  /** "Cite N instead" on a miscited claim of this answer. */
  onCiteInstead: (messageId: string, check: ClaimCheck) => Promise<boolean>;
}

/** How long an answer reached with "Go to message" stays highlighted. */
const REVEAL_HIGHLIGHT_MS = 2_400;

function AssistantMessageView({
  conversationId,
  message,
  question,
  activeCitation,
  activeFile,
  canRetry,
  latest,
  inlinePlan,
  onOpenSource,
  onOpenArtifact,
  onRetry,
  onDecide,
  onRuntimeInstalled,
  onOpenSettings,
  onOpenThread,
  onCiteInstead,
}: AssistantMessageProps) {
  const transcript = message.transcript;
  const passages = transcript?.passages;
  const searchResults = message.searchResults;
  const hits = useMemo(
    () => (passages ? passageHits(passages) : toSearchHits(searchResults)),
    [passages, searchResults],
  );
  const groups = useMemo(() => groupSources(hits, citedNumbers(message.content)), [hits, message.content]);
  const running = transcript ? isLive(transcript) : message.run?.status === 'running';
  // Depend on the id only: the message object changes on every streamed
  // frame, and a new callback would rebuild the markdown component map.
  const messageId = message.id;
  // Web sources open in the browser; documents open in the source viewer.
  const handleOpen = useCallback(
    (hit: SearchHit, trigger: HTMLElement) => {
      if (hit.url) {
        void openExternal(hit.url);
        return;
      }
      onOpenSource(messageId, hit, trigger);
    },
    [messageId, onOpenSource],
  );

  const openThread = useCallback(
    (thread: FocusThread, trigger: HTMLElement) => onOpenThread(messageId, thread, trigger),
    [messageId, onOpenThread],
  );

  const citeInstead = useCallback(
    (check: ClaimCheck) => onCiteInstead(messageId, check),
    [messageId, onCiteInstead],
  );

  // Retry and fallback after a provider failure, on the newest answer only
  // (older answers never retry by themselves). Stable while the answer stays
  // the newest, so a countdown is not restarted by re-renders.
  const messageRef = useRef(message);
  messageRef.current = message;
  const providerActions = useMemo<ProviderErrorActions | null>(
    () => (latest && canRetry
      ? {
          questionKey: `${conversationId ?? ''}|${question ?? messageId}`,
          onRetry: (override, from) =>
            onRetry(messageRef.current, override && from ? { override, from } : null),
        }
      : null),
    [latest, canRetry, conversationId, question, messageId, onRetry],
  );

  const article = (
    <article
      className="group/msg flex flex-col gap-4 rounded-xl outline-none data-[revealed=true]:ring-2 data-[revealed=true]:ring-shodh-accent data-[revealed=true]:ring-offset-4 data-[revealed=true]:ring-offset-shodh-ground"
      aria-busy={running}
      // Selected text in a finished answer can be asked about (features/focus/SelectionAsk).
      data-ask-scope={!running && conversationId ? 'answer' : undefined}
      data-conversation-id={conversationId ?? undefined}
      data-message-id={messageId}
    >
      {transcript ? (
        <>
          {inlinePlan && transcript.plan && <PlanPanel items={transcript.plan} live={running} variant="inline" />}
          <Transcript
            transcript={transcript}
            hits={hits}
            activeCitation={activeCitation}
            onOpenCitation={handleOpen}
            onDecide={onDecide}
            onRuntimeInstalled={onRuntimeInstalled}
            onOpenSettings={onOpenSettings}
            providerActions={providerActions}
            onCiteInstead={conversationId ? citeInstead : undefined}
          />
        </>
      ) : (
        <>
          {message.run && <RunChip run={message.run} metadata={message.metadata} passageCount={hits.length} />}

          {message.image && (
            <img
              src={message.image}
              alt="Image you added"
              className="max-w-full max-h-[400px] object-contain rounded-xl border border-shodh-border"
            />
          )}

          {message.content.length > 0 && (
            <div>
              <MessageContentRenderer
                content={message.content}
                hits={hits}
                artifacts={running ? undefined : message.artifacts}
                activeCitation={activeCitation}
                onOpenCitation={handleOpen}
                onOpenArtifact={onOpenArtifact}
              />
            </div>
          )}

          {message.run?.status === 'failed' && (
            <p role="alert" className="text-[13.5px] leading-relaxed text-shodh-error">
              {`The answer could not be completed: ${message.run.error ?? 'unknown error'}`}
            </p>
          )}

          {message.run?.status === 'cancelled' && message.content.length === 0 && (
            <p className="text-[13.5px] text-shodh-text-muted">You stopped this answer before any text arrived.</p>
          )}
        </>
      )}

      {!running && <SourceChips groups={groups} activeFile={activeFile} onOpen={handleOpen} />}

      {!running && message.threads && <ThreadChips threads={message.threads} onOpen={openThread} />}

      {!running && (
        <div className="flex items-center gap-0.5 opacity-60 group-hover/msg:opacity-100 focus-within:opacity-100 transition-opacity duration-micro">
          {message.content.length > 0 && <CopyAnswerButton text={stripTextMarks(message.content)} />}
          {message.content.trim().length > 0 && <ExportPdfButton message={message} question={question} hits={hits} />}
          {(message.run || transcript) && (
          <button
            type="button"
            onClick={() => onRetry(message)}
            disabled={!canRetry}
            aria-label="Retry: ask the same question again"
            title="Retry"
            className={cn(
              'w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-40 disabled:cursor-not-allowed transition-colors duration-micro',
              FOCUS_RING,
            )}
          >
            <RotateCcw className="w-[15px] h-[15px]" aria-hidden="true" />
          </button>
          )}
        </div>
      )}
    </article>
  );

  // Visuals in a finished answer can open the focus pop-out, anchored here.
  return (
    <FocusAnchorProvider conversationId={conversationId} messageId={messageId} enabled={!running}>
      {article}
    </FocusAnchorProvider>
  );
}

// Answers re-render only when their own props change: the composer's draft
// lives in AskView, and re-rendering every answer (markdown, math, citations)
// on each keystroke made typing lag in long conversations.
const AssistantMessage = React.memo(AssistantMessageView);

/**
 * A summary brought back from a side discussion: a card (not a plain
 * bubble) that renders its equations and diagrams and reopens the
 * discussion it came from.
 */
const SideSummaryCard = React.memo(function SideSummaryCard({
  summary,
  content,
  canOpen,
  onOpen,
}: {
  summary: SideSummaryRef;
  content: string;
  canOpen: boolean;
  onOpen: (summary: SideSummaryRef, trigger: HTMLElement) => void;
}) {
  return (
    <div className="flex justify-end">
      <section
        aria-label={`From a side discussion about ${summary.label}`}
        className="w-full max-w-[560px] min-w-0 rounded-[18px] border border-shodh-border bg-shodh-surface px-4 py-3 flex flex-col gap-2"
      >
        <header className="flex items-center gap-2 min-w-0 text-[12px] text-shodh-text-muted">
          <CornerLeftUp className="w-3.5 h-3.5 shrink-0 text-shodh-accent-text" aria-hidden="true" />
          <span className="truncate">
            From a side discussion about <span className="text-shodh-text-secondary">{summary.label}</span>
          </span>
          <button
            type="button"
            onClick={e => onOpen(summary, e.currentTarget)}
            disabled={!canOpen}
            title={canOpen ? 'Reopen the side discussion' : 'This side discussion is no longer available'}
            className={cn(
              'ml-auto shrink-0 inline-flex items-center gap-1 h-6 px-2 rounded-full border border-shodh-border text-[11.5px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-45 disabled:cursor-not-allowed transition-colors duration-micro',
              FOCUS_RING,
            )}
          >
            <MessagesSquare className="w-3 h-3" aria-hidden="true" />
            Open discussion
          </button>
        </header>
        <div className="text-[14.5px] leading-[1.55] text-shodh-text">
          <UserText text={content} markdown />
        </div>
      </section>
    </div>
  );
});

function SystemNotice({ message }: { message: ChatMessage }) {
  return (
    <div className="mx-auto max-w-[560px] w-full rounded-xl border border-shodh-border-subtle bg-shodh-surface px-4 py-3 text-[13.5px] flex flex-col gap-3">
      {message.image && (
        <img
          src={message.image}
          alt="Image you added"
          className="max-w-full max-h-[400px] object-contain rounded-lg border border-shodh-border self-start"
        />
      )}
      <MessageContentRenderer content={message.content} hits={[]} onOpenCitation={() => undefined} />
    </div>
  );
}

/**
 * The Ask screen: empty state with a centered composer, or the conversation
 * (centered 700px column) with a floating composer and a source preview
 * slide-over.
 */
export function AskView({ sources, searched, llmStatus, onNavigate, onPickImage, onPasteImage, isDraggingFile = false, dropHandlers, draftRequest = null, onDraftApplied }: AskViewProps) {
  const { theme } = useTheme();
  const session = useChatSession();
  const { messages, isStreaming, streamingConversationId, sideRun, send, retry, cancel, steer, approve, runtimeInstalled, setRuntimeInstalled, updateMessage } = session;
  const focus = useFocus();
  const wide = useMediaQuery(WIDE_LAYOUT_QUERY);

  // Kept per conversation outside this view, which unmounts on every view switch.
  const [draft, setDraft] = useDraft(session.activeConversationId);
  // Text read from images, sent with the next question as the user's own context.
  const attachments = useImageAttachments(session.activeConversationId);
  const [preview, setPreview] = useState<PreviewTarget | null>(null);
  const [openArtifactId, setOpenArtifactId] = useState<string | null>(null);
  const previewTriggerRef = useRef<HTMLElement | null>(null);
  const composerRef = useRef<AgentComposerHandle>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);
  // The composer floats over the end of the conversation; the conversation
  // keeps that much room below its last answer (the composer grows with a
  // wrapped toolbar, notices and long drafts), so nothing hides behind it.
  // A callback ref: the composer mounts after the first render (while the
  // conversation loads), so measuring starts whenever it appears.
  const [composerArea, setComposerArea] = useState<HTMLDivElement | null>(null);
  const [composerSpace, setComposerSpace] = useState(COMPOSER_SPACE_FALLBACK);
  useLayoutEffect(() => {
    if (!composerArea || typeof ResizeObserver === 'undefined') return;
    const measure = () => setComposerSpace(Math.ceil(composerArea.getBoundingClientRect().height) + COMPOSER_GAP);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(composerArea);
    return () => observer.disconnect();
  }, [composerArea]);

  // Start the agent runtime while the question is being typed (it is not
  // started with the app); once per draft, and again after a mode or
  // conversation change since those use another session.
  const { prewarm } = session;
  const hasDraft = draft.trim().length > 0;
  useEffect(() => {
    if (hasDraft) prewarm();
  }, [hasDraft, prewarm]);

  // "Include this source when answering": the same options as the dock's replies.
  const sendOptions: SendOptions = useSendOptions(sources);

  // Research or Code, per conversation (Code works in the workspace's code folder).
  const mode = session.activeConversation?.mode ?? 'research';
  const [codeProblem, setCodeProblem] = useState<string | null>(null);
  const codeWorkspaceId = mode === 'code' ? session.activeConversation?.workspaceId ?? null : null;
  const diagramHost = useMemo(() => ({ codeWorkspaceId }), [codeWorkspaceId]);
  const { updateConversationMeta } = session;
  const changeMode = useCallback((next: ConversationMode) => {
    const id = session.activeConversationId;
    if (id) updateConversationMeta(id, { mode: next });
  }, [session.activeConversationId, updateConversationMeta]);

  const busyElsewhere = (streamingConversationId !== null && !isStreaming) || sideRun !== null;
  const blockedReason = sideRun
    ? `Answering your side question about “${sideRun.label}”. You can send once it finishes; to stop it, reopen that discussion and press Esc.`
    : busyElsewhere
      ? 'An answer is still being written in another conversation. You can send once it finishes.'
      : mode === 'code' && codeProblem && !isStreaming
        ? `${codeProblem} Or switch this conversation to Research.`
        : null;

  // Whether there is anything to search, as the sources chip reads it
  // (`librarySourcesChip`), so the empty state never shows beside "All sources · N".
  const hasSources = searched.hasSources;
  // Starter questions fit the library: papers get methods, results and
  // comparisons. Read again when the sources change (a folder added or
  // finished indexing); the general questions show until it is known.
  const [libraryKind, setLibraryKind] = useState<LibraryKind | null>(null);
  const readySourcesKey = sources.filter(s => s.status === 'ready').map(s => s.id).join('|');
  const showStarters = messages.length === 0 && hasSources;
  useEffect(() => {
    if (!showStarters) return;
    let cancelled = false;
    invoke<unknown>('library_profile')
      .then(profile => {
        if (!cancelled) setLibraryKind(readLibraryKind(profile));
      })
      .catch(() => {
        // The index is not open yet or could not be read: keep the general questions.
      });
    return () => {
      cancelled = true;
    };
  }, [showStarters, readySourcesKey]);
  // In a workspace chat, answers search only the workspace's sources unless "Search all my
  // library" is on for the next question (it switches off again once that is sent).
  const { byId: workspaceById, openWorkspace } = useWorkspaces();
  const workspace = workspaceById(session.activeConversation?.workspaceId);
  const [searchAll, setSearchAll] = useState(false);
  const chip = scopeChip(workspace, searchAll, librarySourcesChip(searched));

  // Reset per-conversation UI when the conversation changes.
  const conversationId = session.activeConversationId;
  useEffect(() => {
    setPreview(null);
    setOpenArtifactId(null);
    setSearchAll(false);
    setFindOpen(false);
    setFindIndex(null);
    setRevealIndex(null);
  }, [conversationId]);

  // The conversation is windowed (ConversationLog): only the messages in
  // view are mounted. These keep a message mounted wherever the view is.
  const [findOpen, setFindOpen] = useState(false);
  const [findIndex, setFindIndex] = useState<number | null>(null);
  const [revealIndex, setRevealIndex] = useState<number | null>(null);
  const [focusedIndex, setFocusedIndex] = useState<number | null>(null);
  const previewIndex = useMemo(
    () => (preview ? messages.findIndex(m => m.id === preview.messageId) : -1),
    [preview, messages],
  );
  const keepMounted = useMemo(
    () => [focusedIndex, previewIndex, findIndex, revealIndex],
    [focusedIndex, previewIndex, findIndex, revealIndex],
  );
  // The scroller as state: the window attaches to it once it is in the document.
  const [scroller, setScroller] = useState<HTMLDivElement | null>(null);
  const setScrollerRef = useCallback((el: HTMLDivElement | null) => {
    scrollerRef.current = el;
    setScroller(el);
  }, []);
  const windowRef = useRef<ConversationLogHandle>(null);

  // The message holding keyboard focus stays mounted while it is scrolled away.
  const onFocusIn = useCallback((e: React.FocusEvent<HTMLDivElement>) => {
    const item = (e.target as HTMLElement).closest<HTMLElement>(`[${MESSAGE_INDEX_ATTR}]`);
    setFocusedIndex(item ? Number(item.getAttribute(MESSAGE_INDEX_ATTR)) : null);
  }, []);
  const onFocusOut = useCallback((e: React.FocusEvent<HTMLDivElement>) => {
    if (!(e.relatedTarget instanceof Node) || !e.currentTarget.contains(e.relatedTarget)) setFocusedIndex(null);
  }, []);

  /** Bring the message at `index` into view and return its element once mounted. */
  const revealMessage = useCallback(async (index: number): Promise<HTMLElement | null> => {
    // The window may still be loading (first conversation with messages).
    for (let frame = 0; frame < 120 && !windowRef.current; frame++) {
      await new Promise(resolve => requestAnimationFrame(resolve));
    }
    return windowRef.current ? windowRef.current.reveal(index) : null;
  }, []);

  // Ctrl+F finds in the whole conversation (unmounted answers included), in
  // place of the browser's find, which only sees what is mounted. Dialogs
  // over the conversation (source preview, palette, pop-out) keep their own.
  useEffect(() => {
    if (messages.length === 0) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey || e.key.toLowerCase() !== 'f') return;
      const target = e.target instanceof Element ? e.target : null;
      if (target?.closest(`[role="dialog"], [aria-modal="true"], ${FOCUS_OVERLAY_SELECTOR}`)) return;
      e.preventDefault();
      setFindOpen(true);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [messages.length]);
  const firstVisibleIndex = useCallback(() => windowRef.current?.firstVisibleIndex() ?? 0, []);
  const closeFind = useCallback(() => {
    setFindOpen(false);
    setFindIndex(null);
    composerRef.current?.focus();
  }, []);

  // Messages already shown do not rise again when they remount (window
  // scrolls); new ones, and those of a conversation as it opens, do.
  const shownRef = useRef<{ conversationId: string | null; ids: Set<string> }>({ conversationId: null, ids: new Set() });
  const wasShown = useCallback((id: string) => shownRef.current.ids.has(id), []);
  useEffect(() => {
    if (shownRef.current.conversationId !== conversationId) {
      shownRef.current = { conversationId, ids: new Set() };
    }
    for (const m of messages) shownRef.current.ids.add(m.id);
  }, [messages, conversationId]);

  // "Go to message" from the gallery: scroll to that answer once it is shown.
  const [reveal, setReveal] = useState<MessageReveal | null>(() => pendingReveal());
  const revealTimerRef = useRef<{ timer: number; target: HTMLElement } | null>(null);
  useEffect(() => subscribeReveal(setReveal), []);
  useEffect(() => () => {
    const shown = revealTimerRef.current;
    if (shown) {
      window.clearTimeout(shown.timer);
      delete shown.target.dataset.revealed;
    }
  }, []);
  useEffect(() => {
    if (!reveal || reveal.conversationId !== conversationId) return;
    const index = messages.findIndex(m => m.id === reveal.messageId);
    if (index < 0) {
      // The view may still show the previous conversation's messages: wait
      // while the stored conversation has the answer.
      const stored = session.activeConversation?.id === reveal.conversationId ? session.activeConversation.messages : null;
      if (!stored || stored.some(m => m.id === reveal.messageId)) return;
      clearReveal(reveal);
      setReveal(null);
      notify.info('That answer is no longer in this conversation');
      return;
    }
    clearReveal(reveal);
    setReveal(null);
    setRevealIndex(index);
    void revealMessage(index).then(item => {
      const target = item?.querySelector<HTMLElement>(`[data-message-id="${CSS.escape(reveal.messageId)}"]`) ?? item;
      if (!target) return;
      target.scrollIntoView({ block: 'start', behavior: scrollBehavior() });
      target.setAttribute('tabindex', '-1');
      target.focus({ preventScroll: true });
      const previous = revealTimerRef.current;
      if (previous) {
        window.clearTimeout(previous.timer);
        delete previous.target.dataset.revealed;
      }
      target.dataset.revealed = 'true';
      revealTimerRef.current = {
        target,
        timer: window.setTimeout(() => {
          delete target.dataset.revealed;
          revealTimerRef.current = null;
          setRevealIndex(null);
        }, REVEAL_HIGHLIGHT_MS),
      };
    });
  }, [reveal, conversationId, messages, session.activeConversation, revealMessage]);

  const latest = useMemo(() => latestTranscript(messages), [messages]);
  const liveTranscript = isStreaming && latest && isLive(latest) ? latest : null;
  const waitingStep = liveTranscript ? pendingApproval(liveTranscript) : null;
  const waitingStepId = waitingStep?.id ?? null;

  // Esc denies a pending approval, otherwise interrupts the running answer.
  // The source preview handles Esc first (capture phase) and marks it handled.
  useEffect(() => {
    if (!isStreaming) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.defaultPrevented) return;
      e.preventDefault();
      if (waitingStepId) approve(waitingStepId, false);
      else cancel();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [isStreaming, cancel, approve, waitingStepId]);

  const submit = useCallback(() => {
    const text = draft.trim();
    if (!text || isStreaming || busyElsewhere) return;
    // The typed words and the attached image text go out separately; both are kept
    // until the question is accepted.
    const conversationId = session.activeConversationId;
    const attached = composerAttachments.get(conversationId);
    const accepted = send(
      text,
      { ...sendOptions, searchAll: workspace !== null && searchAll },
      { attachedContext: attachedContextOf(attached) },
    );
    if (!accepted) return;
    setDraft('');
    composerAttachments.consume(conversationId, attached);
    setSearchAll(false);
    composerRef.current?.focus();
  }, [draft, setDraft, isStreaming, busyElsewhere, send, sendOptions, workspace, searchAll, session.activeConversationId]);

  const submitSteer = useCallback(() => {
    const text = draft.trim();
    if (!text || !isStreaming) return;
    setDraft('');
    steer(text);
    composerRef.current?.focus();
  }, [draft, setDraft, isStreaming, steer]);

  const approveWaiting = useCallback(() => {
    if (waitingStepId) approve(waitingStepId, true);
  }, [approve, waitingStepId]);

  // Edited artifacts are stored with the message that produced them.
  const saveArtifact = useCallback((artifactId: string, content: string) => {
    const owner = messages.find(m => Array.isArray(m.artifacts) && m.artifacts.some(a => a.id === artifactId));
    if (!owner || !owner.artifacts) return;
    updateMessage(owner.id, {
      artifacts: owner.artifacts.map(a => (a.id === artifactId ? { ...a, content } : a)),
    });
  }, [messages, updateMessage]);

  // "Cite N instead": the user moves a miscited claim's citation to the passage the check found
  // supporting it; the answer is saved with the conversation like any other edit. Read through a
  // ref so the callback stays the same while answers stream (every answer is memoised on it).
  const messagesRef = useRef(messages);
  messagesRef.current = messages;
  const correctionsRef = useRef(new CorrectionGuard());
  const citeInstead = useCallback(async (messageId: string, check: ClaimCheck): Promise<boolean> => {
    const rewriteOf = () => {
      const owner = messagesRef.current.find(m => m.id === messageId);
      return owner?.transcript && !isLive(owner.transcript) ? citeSuggested(owner.transcript, check) : null;
    };
    const rewrite = rewriteOf();
    const conversationId = session.activeConversationId;
    if (!rewrite || !conversationId) {
      notify.error('This citation could not be changed', { description: 'The statement is no longer in the answer as it was checked.' });
      return false;
    }
    const guard = correctionsRef.current;
    if (!guard.claim(messageId)) {
      notify.info(CORRECTION_BUSY);
      return false;
    }
    let outcome: CorrectionOutcome;
    try {
      // Audit first: the answer changes only once Activity has the record, and then from its
      // latest state, never over a change made in between.
      outcome = await applyAuditedCorrection(
        () => recordCitationCorrection(citationCorrection(rewrite, conversationId)),
        () => {
          const latest = rewriteOf();
          if (!latest || !sameCorrection(rewrite, latest)) return false;
          updateMessage(messageId, { content: answerText(latest.transcript), transcript: latest.transcript });
          return true;
        },
      );
    } finally {
      guard.release(messageId);
    }
    if (outcome === 'not_recorded') {
      notify.error(CORRECTION_NOT_RECORDED);
      return false;
    }
    if (outcome !== 'applied') {
      notify.error('This citation could not be changed', { description: 'The answer changed while the correction was recorded.' });
      return false;
    }
    // The flag and its button are gone; say what changed (toasts are a live region).
    notify.success(`Now cites passage ${check.suggested}`);
    return true;
  }, [updateMessage, session.activeConversationId]);

  const markRuntimeInstalled = useCallback(() => setRuntimeInstalled(true), [setRuntimeInstalled]);
  const openSettings = useCallback(() => onNavigate('settings'), [onNavigate]);
  const openMemorySettings = useCallback(() => {
    requestSettingsSection('memory');
    onNavigate('settings');
  }, [onNavigate]);

  const applyStarter = useCallback((prompt: string) => {
    setDraft(prompt);
    composerRef.current?.focus();
  }, [setDraft]);

  // A snippet (or other context) sent to the composer, e.g. "Add to chat":
  // appended to the draft, also when Ask mounts after the request.
  const insertContext = useCallback((text: string) => {
    setDraft(prev => appendToDraft(prev, text));
    requestAnimationFrame(() => composerRef.current?.focus());
  }, [setDraft]);
  useEffect(() => {
    const drain = () => {
      const pending = takePendingInserts();
      if (pending.length > 0) insertContext(pending.join(''));
    };
    drain();
    return onWindowEvent<string>(COMPOSER_INSERT_EVENT, drain);
  }, [insertContext]);

  // e.g. "Ask about this file" from the Library: prefill, then let the user finish the question.
  const appliedDraftSeq = useRef(0);
  useEffect(() => {
    if (!draftRequest || draftRequest.seq === appliedDraftSeq.current) return;
    appliedDraftSeq.current = draftRequest.seq;
    applyStarter(draftRequest.text);
    onDraftApplied?.();
  }, [draftRequest, applyStarter, onDraftApplied]);

  const openSource = useCallback((messageId: string, hit: SearchHit, trigger: HTMLElement) => {
    previewTriggerRef.current = trigger;
    setPreview({ messageId, hit });
  }, []);

  // The agent asked to show a document (show_document).
  useNavigationTarget('document', target => {
    previewTriggerRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setPreview({ messageId: AGENT_DOCUMENT_PREVIEW, hit: documentHit(target.path, target.page, target.passage) });
  });

  const openThread = useCallback((messageId: string, thread: FocusThread, trigger: HTMLElement) => {
    focus?.openFocus({
      target: thread.anchor.target,
      conversationId: thread.anchor.conversationId,
      parentMessageId: messageId,
      threadId: thread.id,
      trigger,
    });
  }, [focus]);

  const focusPreview = useCallback((trigger: HTMLElement) => {
    if (!focus || !preview) return;
    focus.openFocus({
      target: sourceTarget(preview.hit, sourceLabel(preview.hit)),
      conversationId,
      // A document the agent showed belongs to no answer: kept on the conversation.
      parentMessageId: preview.messageId === AGENT_DOCUMENT_PREVIEW ? null : preview.messageId,
      trigger,
    });
  }, [focus, preview, conversationId]);

  const summaryThreadExists = useCallback((summary: SideSummaryRef): boolean =>
    Boolean(conversationId && focus?.findThread(conversationId, summary.parentMessageId, summary.threadId)),
  [focus, conversationId]);

  const openSummaryThread = useCallback((summary: SideSummaryRef, trigger: HTMLElement) => {
    if (!focus || !conversationId) return;
    const thread = focus.findThread(conversationId, summary.parentMessageId, summary.threadId);
    if (!thread) return;
    focus.openFocus({
      target: thread.anchor.target,
      conversationId,
      parentMessageId: summary.parentMessageId,
      threadId: thread.id,
      trigger,
    });
  }, [focus, conversationId]);

  const closePreview = useCallback(() => {
    setPreview(null);
    const trigger = previewTriggerRef.current;
    previewTriggerRef.current = null;
    if (trigger && trigger.isConnected) trigger.focus();
  }, []);

  const handleRetry = useCallback((message: ChatMessage, fallback: FallbackRun | null = null) => {
    retry(message.id, sendOptions, fallback);
  }, [retry, sendOptions]);

  const previewSiblings = useMemo(() => {
    if (!preview) return [];
    const message = messages.find(m => m.id === preview.messageId);
    return message ? messageHits(message).filter(h => h.sourceFile === preview.hit.sourceFile) : [];
  }, [preview, messages]);

  const allArtifacts = useMemo(
    () => messages.flatMap(m => (Array.isArray(m.artifacts) ? m.artifacts : [])),
    [messages],
  );

  // The latest answer shows its own install card when it failed for this reason.
  const runtimeCard = runtimeInstalled === false && latest?.errorCode !== 'runtime_missing' && (
    <RuntimeCard reason="missing" onInstalled={markRuntimeInstalled} />
  );

  const composer = (autoFocus: boolean) => (
    <SnippetDropZone onInsert={insertContext}>
      <AgentComposer
        id="ask-composer"
        ref={composerRef}
        value={draft}
        onChange={setDraft}
        onSubmit={submit}
        onSteer={submitSteer}
        onStop={cancel}
        onApprove={waitingStep?.tier === 'write' ? approveWaiting : undefined}
        running={isStreaming}
        canSteer={liveTranscript?.status === 'running'}
        approvalPending={waitingStepId !== null}
        blockedReason={blockedReason}
        placeholder={messages.length === 0 ? 'Ask about your files…' : 'Ask a follow-up…'}
        modelChip={(
          <>
            <ModelChip answerRunning={isStreaming} onOpenSettings={() => onNavigate('settings')} />
            <CodeModeSwitch
              conversationId={conversationId}
              workspaceId={session.activeConversation?.workspaceId ?? null}
              mode={mode}
              onChange={changeMode}
              answerRunning={isStreaming}
              onProblem={setCodeProblem}
            />
            <ToolsChip
              workspaceId={session.activeConversation?.workspaceId ?? null}
              mode={mode}
              answerRunning={isStreaming}
              onOpenSettings={() => {
                requestSettingsSection('tools');
                onNavigate('settings');
              }}
            />
          </>
        )}
        scopeLabel={chip.label}
        scopeTitle={chip.title}
        onOpenLibrary={() => (workspace ? openWorkspace(workspace.id, 'sources') : onNavigate('library'))}
        scopeAction={workspace && (
          <button
            type="button"
            aria-pressed={searchAll}
            onClick={() => setSearchAll(on => !on)}
            title="For the next question only: search your whole library instead of this workspace's sources"
            className={cn(
              'h-7 px-2.5 shrink-0 whitespace-nowrap inline-flex items-center gap-1 rounded-full text-[11.5px] font-medium border transition-colors duration-micro focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
              searchAll
                ? 'bg-shodh-accent-soft border-shodh-accent text-shodh-accent-text'
                : 'border-shodh-border text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised',
            )}
          >
            <Globe className="w-3 h-3" aria-hidden="true" />
            Search all my library
          </button>
        )}
        onPickImage={onPickImage}
        onPasteImage={onPasteImage}
        attachments={<AttachmentChips conversationId={session.activeConversationId} attachments={attachments} />}
        autoFocus={autoFocus}
      />
    </SnippetDropZone>
  );

  const dropOverlay = isDraggingFile && (
    <div
      className="ask-fade-in absolute inset-3 z-30 flex items-center justify-center rounded-[18px] border-2 border-dashed border-shodh-accent bg-shodh-ground/80 pointer-events-none"
      aria-hidden="true"
    >
      <div className="text-center">
        <p className="text-[16px] font-semibold text-shodh-text">Drop to index</p>
        <p className="mt-1 text-[13px] text-shodh-text-muted">PDF, Word, Excel, PowerPoint, text, Markdown or images</p>
      </div>
    </div>
  );

  const canRetry = streamingConversationId === null && sideRun === null;
  const renderMessage = (message: ChatMessage, index: number): React.ReactNode => {
    if (message.role === 'user') {
      const summary = message.sideSummary;
      if (summary) {
        return (
          <SideSummaryCard
            summary={summary}
            content={message.content}
            canOpen={summaryThreadExists(summary)}
            onOpen={openSummaryThread}
          />
        );
      }
      return (
        <div className="flex justify-end">
          <div className="max-w-[520px] min-w-0 px-4 py-[11px] rounded-[18px] bg-shodh-raised-2 text-[15px] leading-[1.55] text-shodh-text">
            <UserText text={message.content} />
            {message.attachedContext && message.attachedContext.length > 0 && (
              <ul aria-label="Sent with this question" className="m-0 mt-2 p-0 list-none flex flex-wrap gap-1.5">
                {message.attachedContext.map((item, i) => (
                  <li
                    key={i}
                    title={item.text.slice(0, 400)}
                    className="inline-flex items-center gap-1.5 h-6 max-w-full px-2 rounded-full bg-shodh-surface text-[11.5px] text-shodh-text-secondary"
                  >
                    <FileText className="w-3 h-3 shrink-0 text-shodh-accent-text" aria-hidden="true" />
                    <span className="truncate">{attachmentLabel(item)}</span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      );
    }
    if (message.role === 'system') {
      return <SystemNotice message={message} />;
    }
    const isPreviewed = preview?.messageId === message.id;
    return (
      <AssistantMessage
        conversationId={conversationId}
        message={message}
        question={index > 0 && messages[index - 1].role === 'user' ? messages[index - 1].content : null}
        activeCitation={isPreviewed ? preview.hit.number : null}
        activeFile={isPreviewed ? preview.hit.sourceFile : null}
        canRetry={canRetry}
        latest={index === messages.length - 1}
        inlinePlan={!wide}
        onOpenSource={openSource}
        onOpenArtifact={setOpenArtifactId}
        onRetry={handleRetry}
        onDecide={approve}
        onRuntimeInstalled={markRuntimeInstalled}
        onOpenSettings={openSettings}
        onOpenThread={openThread}
        onCiteInstead={citeInstead}
      />
    );
  };

  if (messages.length === 0) {
    return (
      <section aria-label="Ask" className="relative h-full overflow-y-auto bg-shodh-ground" {...dropHandlers}>
        {dropOverlay}
        <div className="min-h-full flex items-center justify-center px-7 py-12">
          <div className="ask-rise w-full max-w-[700px] flex flex-col gap-7">
            <h1 className="text-center text-[28px] font-semibold tracking-[-0.01em] text-shodh-text">
              Ask anything about your files
            </h1>
            <SearchSetupCard />
            {runtimeCard}
            {composer(true)}
            {!hasSources ? (
              <div className="flex flex-col items-center gap-3 text-center">
                <p className="text-[13.5px] text-shodh-text-muted">
                  Shodh answers from the files you add. Add a folder to start asking about it.
                </p>
                <button
                  type="button"
                  onClick={() => onNavigate('library')}
                  className={cn(
                    'inline-flex items-center gap-2 h-9 px-4 rounded-[9px] bg-shodh-accent text-shodh-on-accent text-[13px] font-semibold hover:bg-shodh-accent-hover transition-colors duration-micro',
                    FOCUS_RING,
                  )}
                >
                  <FolderPlus className="w-4 h-4" aria-hidden="true" />
                  Add a folder
                </button>
              </div>
            ) : (
              <ul className="grid grid-cols-1 sm:grid-cols-2 gap-2.5" aria-label="Starter questions">
                {starterPrompts(libraryKind).map(prompt => (
                  <li key={prompt}>
                    <button
                      type="button"
                      onClick={() => applyStarter(prompt)}
                      className={cn(
                        'w-full h-full text-left px-4 py-3 rounded-[14px] border border-shodh-border bg-shodh-surface text-[13.5px] leading-snug text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
                        FOCUS_RING,
                      )}
                    >
                      {prompt}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      </section>
    );
  }

  return (
    <section aria-label="Ask" className="relative h-full flex flex-col bg-shodh-ground" {...dropHandlers}>
      {dropOverlay}
      <div
        ref={setScrollerRef}
        onFocus={onFocusIn}
        onBlur={onFocusOut}
        className="flex-1 overflow-y-auto scrollbar-thin"
        style={{ paddingTop: CONVERSATION_TOP_PX, paddingBottom: composerSpace }}
      >
        <DiagramHostContext.Provider value={diagramHost}>
          <Suspense fallback={null}>
            <ConversationLog
              ref={windowRef}
              scroller={scroller}
              conversationId={conversationId}
              messages={messages}
              keep={keepMounted}
              bottomSpace={composerSpace}
              wasShown={wasShown}
              renderMessage={renderMessage}
            />
          </Suspense>
        </DiagramHostContext.Provider>
      </div>

      {findOpen && (
        <div className="absolute top-3 right-6 z-20">
          <Suspense fallback={null}>
            <ConversationFind
              messages={messages}
              firstVisibleIndex={firstVisibleIndex}
              reveal={revealMessage}
              onTarget={setFindIndex}
              onClose={closeFind}
            />
          </Suspense>
        </div>
      )}

      <div ref={setComposerArea} className="absolute left-0 right-0 bottom-0 px-7 pt-10 pb-[22px] bg-gradient-to-b from-transparent via-shodh-ground via-[38%] to-shodh-ground pointer-events-none">
        <div className="max-w-[700px] mx-auto pointer-events-auto flex flex-col gap-2">
          <SearchSetupCard compact />
          {runtimeCard}
          {composer(true)}
          <div className="min-h-[18px] px-2 flex items-center justify-between gap-2">
            <div className="min-w-0">
              {latest && <StatusLine transcript={latest} fallbackModel={llmStatus.connected ? llmStatus.model : null} />}
            </div>
            <SuggestionsBadge onOpen={openMemorySettings} />
          </div>
        </div>
      </div>

      {wide && latest?.plan && (
        <aside className="absolute right-6 top-[18px] z-10" aria-label="Plan of the latest answer">
          <PlanPanel items={latest.plan} live={isLive(latest)} variant="docked" />
        </aside>
      )}

      {preview && (
        // Text selected in the document is asked about in a side discussion on its answer.
        <div className="contents" data-ask-message-id={preview.messageId === AGENT_DOCUMENT_PREVIEW ? undefined : preview.messageId}>
          <SourcePreview
            key={preview.messageId}
            hit={preview.hit}
            siblings={previewSiblings}
            onSelectHit={hit => setPreview(p => (p ? { ...p, hit } : p))}
            onClose={closePreview}
            onOpenView={onNavigate}
            onFocus={focus ? focusPreview : undefined}
          />
        </div>
      )}

      {openArtifactId && allArtifacts.length > 0 && (
        <>
          <div
            className="ask-fade-in absolute inset-0 z-30 bg-black/35"
            onClick={() => setOpenArtifactId(null)}
            aria-hidden="true"
          />
          <div className="ask-slide-in absolute right-0 top-0 bottom-0 w-[55%] min-w-[360px] z-40 shadow-[-8px_0_30px_rgba(0,0,0,0.18)]">
            <Suspense fallback={<div className="h-full bg-shodh-surface" aria-busy="true" />}>
              <EnhancedArtifactPanel
                artifacts={allArtifacts}
                theme={theme}
                selectedArtifactId={openArtifactId}
                onSave={saveArtifact}
                onClose={() => setOpenArtifactId(null)}
              />
            </Suspense>
          </div>
        </>
      )}
    </section>
  );
}

export default AskView;

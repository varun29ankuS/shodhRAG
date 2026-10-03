import React, { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, CheckCircle2, Info, Loader2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { appRecordKind, isWebUrl } from './searchResults';
import type { SearchHit } from './types';
import { ImageViewer } from './viewer/ImageViewer';
import { PdfViewer } from './viewer/PdfViewer';
import { TableViewer } from './viewer/TableViewer';
import { TextViewer } from './viewer/TextViewer';
import { getSourceFileInfo, toSourceError, type SourceAccessError, type SourceFileInfo } from './viewer/sourceAccess';
import type { LocateResult } from './viewer/viewerTypes';
import { VIEWER_FOCUS_RING as FOCUS_RING } from './viewer/viewerTypes';

/** Extensions shown as prose (proportional font); other text is code. */
const PROSE_EXTENSIONS = new Set(['pdf', 'docx', 'pptx', 'txt', 'text', 'md', 'markdown', 'mdx', 'rst', 'log', 'html', 'htm']);

type ViewerMode = 'pdf' | 'text' | 'table' | 'image';

export type PanelState =
  | { status: 'loading' }
  | { status: 'web' }
  | { status: 'record' }
  | { status: 'unavailable'; error: SourceAccessError; info: SourceFileInfo | null }
  | { status: 'view'; info: SourceFileInfo; viewer: ViewerMode; reason: string | null; retryPdf: boolean };

function unavailableMessage(error: SourceAccessError, info: SourceFileInfo | null): string {
  switch (error.kind) {
    case 'notIndexed':
      return 'This file is not in your index any more, so it cannot be shown here.';
    case 'notFound':
      return 'This file is no longer at its indexed location, so it cannot be shown.';
    case 'unsupported':
      return info ? `Shodh cannot display .${info.extension} files here.` : error.message;
    default:
      return error.message;
  }
}

/** Catches render-time failures in a viewer and shows the passage instead. */
class ViewerBoundary extends React.Component<
  { fallback: (error: Error) => React.ReactNode; children: React.ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    return this.state.error ? this.props.fallback(this.state.error) : this.props.children;
  }
}

function PassageBlock({ passage }: { passage: string }) {
  return (
    <blockquote className="m-0 rounded-xl border border-shodh-border bg-shodh-surface-2 px-4 py-3.5 text-[14px] leading-[1.7] text-shodh-text-secondary whitespace-pre-wrap break-words">
      {passage || 'No text was stored for this passage.'}
    </blockquote>
  );
}

/** Shown whenever the document itself cannot be displayed. */
function PassageFallback({ message, passage, tone = 'warning' }: { message: string; passage: string; tone?: 'warning' | 'info' }) {
  return (
    <div tabIndex={0} aria-label="Cited passage" className={cn('flex-1 min-h-0 overflow-y-auto scrollbar-thin p-5 flex flex-col gap-4', FOCUS_RING)}>
      <p role={tone === 'warning' ? 'alert' : undefined} className="flex items-start gap-2 text-[13px] leading-relaxed text-shodh-text-secondary">
        {tone === 'warning' ? (
          <AlertTriangle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
        ) : (
          <Info className="w-4 h-4 mt-0.5 shrink-0 text-shodh-info" aria-hidden="true" />
        )}
        <span>{message}</span>
      </p>
      <figure className="m-0 flex flex-col gap-2">
        <figcaption className="text-[11.5px] font-medium uppercase tracking-wider text-shodh-text-faint">Retrieved passage</figcaption>
        <PassageBlock passage={passage} />
      </figure>
    </div>
  );
}

/** A dynamic import() of a viewer bundle failed (network or stale dev bundle), as opposed to the file. */
function isModuleLoadFailure(error: unknown): boolean {
  return (
    error instanceof TypeError &&
    /dynamically imported module|Importing a module script failed|error loading dynamically imported module/i.test(error.message)
  );
}

function LocateNotice({
  result,
  reason,
  passage,
  onRetry,
}: {
  result: LocateResult | null;
  reason: string | null;
  passage: string;
  onRetry?: () => void;
}) {
  const showPassage = result !== null && (result.status === 'notFound' || result.status === 'approximate');
  const Icon =
    result?.status === 'found' ? CheckCircle2 : result?.status === 'searching' ? Loader2 : result ? AlertTriangle : Info;
  return (
    <div className="px-4 py-2 border-b border-shodh-border-subtle flex flex-col gap-1.5">
      {reason && (
        <p className="flex items-start gap-2 text-[12.5px] leading-snug text-shodh-text-secondary">
          <Info className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-info" aria-hidden="true" />
          <span>
            {reason}
            {onRetry && (
              <>
                {' '}
                <button
                  type="button"
                  onClick={onRetry}
                  className="font-medium text-shodh-accent-text hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring rounded-sm"
                >
                  Try again
                </button>
              </>
            )}
          </span>
        </p>
      )}
      <p role="status" aria-live="polite" className="flex items-start gap-2 text-[12.5px] leading-snug text-shodh-text-secondary min-h-[18px]">
        {result && (
          <>
            <Icon
              className={cn(
                'w-3.5 h-3.5 mt-0.5 shrink-0',
                result.status === 'found' && 'text-shodh-success',
                result.status === 'searching' && 'animate-spin motion-reduce:animate-none text-shodh-text-muted',
                (result.status === 'notFound' || result.status === 'approximate') && 'text-shodh-warning',
              )}
              aria-hidden="true"
            />
            <span>{result.message}</span>
          </>
        )}
      </p>
      {showPassage && passage && (
        <details>
          <summary className={cn('cursor-pointer w-fit rounded text-[12px] text-shodh-text-muted hover:text-shodh-text', FOCUS_RING)}>
            Show the retrieved passage
          </summary>
          <div className="mt-2 max-h-48 overflow-y-auto scrollbar-thin">
            <PassageBlock passage={passage} />
          </div>
        </details>
      )}
    </div>
  );
}


export interface SourceDocumentOptions {
  /** The PDF page the reader is on changed. */
  onPageChange?: (page: number) => void;
}

export interface SourceDocument {
  panel: PanelState;
  /** Resolved file details, once known. */
  info: SourceFileInfo | null;
  /** The cited passage text. */
  passage: string;
  /** The document viewer (or the passage fallback) to place in a panel. */
  body: React.ReactNode;
}

/**
 * Opens a cited source for display: resolves the file (access-checked by
 * the backend), picks the viewer (PDF pages, text, spreadsheet grid or
 * image) and degrades to the retrieved passage with an explanation on
 * every failure path. Used by the source preview and the focus pop-out.
 */
export function useSourceDocument(hit: SearchHit, { onPageChange }: SourceDocumentOptions = {}): SourceDocument {
  const [panel, setPanel] = useState<PanelState>({ status: 'loading' });
  const [locate, setLocate] = useState<LocateResult | null>(null);
  const isUrl = isWebUrl(hit.sourceFile);
  const record = appRecordKind(hit.sourceFile);
  const passage = hit.text.trim() || hit.snippet.trim();

  // Resolve the file (access-checked by the backend) and pick a viewer.
  useEffect(() => {
    setLocate(null);
    if (isUrl) {
      setPanel({ status: 'web' });
      return;
    }
    if (record) {
      setPanel({ status: 'record' });
      return;
    }
    let cancelled = false;
    setPanel({ status: 'loading' });
    getSourceFileInfo(hit.sourceFile)
      .then(info => {
        if (cancelled) return;
        if (info.kind === 'unsupported') {
          setPanel({
            status: 'unavailable',
            info,
            error: toSourceError({ kind: 'unsupported', message: `Shodh cannot display .${info.extension} files here.` }),
          });
        } else {
          setPanel({ status: 'view', info, viewer: info.kind, reason: null, retryPdf: false });
        }
      })
      .catch(error => {
        if (!cancelled) setPanel({ status: 'unavailable', error: toSourceError(error), info: null });
      });
    return () => {
      cancelled = true;
    };
  }, [hit.sourceFile, isUrl, record]);

  const handleViewerError = useCallback((error: unknown) => {
    setLocate(null);
    setPanel(prev => ({
      status: 'unavailable',
      error: toSourceError(error),
      info: prev.status === 'view' || prev.status === 'unavailable' ? prev.info : null,
    }));
  }, []);

  // pdf.js could not open the file: fall back to the indexer's extracted text.
  // When the page viewer itself failed to load (not the file), offer a retry.
  const handlePdfFatal = useCallback((error: unknown) => {
    const viewerMissing = isModuleLoadFailure(error);
    const sourceError = toSourceError(error);
    const detail = viewerMissing
      ? 'the page viewer did not load'
      : error instanceof Error && error.name === 'PasswordException'
        ? 'it is password-protected'
        : sourceError.kind === 'unknown'
          ? 'the file could not be rendered'
          : sourceError.message;
    setLocate(null);
    setPanel(prev =>
      prev.status === 'view'
        ? {
            ...prev,
            viewer: 'text',
            reason: `The PDF pages are not shown because ${detail}. Showing its extracted text instead.`,
            retryPdf: viewerMissing,
          }
        : prev,
    );
  }, []);

  const retryPdf = useCallback(() => {
    setLocate(null);
    setPanel(prev => (prev.status === 'view' ? { ...prev, viewer: 'pdf', reason: null, retryPdf: false } : prev));
  }, []);

  const info = panel.status === 'view' || panel.status === 'unavailable' ? panel.info : null;

  let body: React.ReactNode;
  if (panel.status === 'loading') {
    body = (
      <div className="flex-1 flex items-center justify-center gap-2 text-[13px] text-shodh-text-muted">
        <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        Opening document…
      </div>
    );
  } else if (panel.status === 'web') {
    body = <PassageFallback message="This source is a web page. Use Open in browser to view it." passage={passage} />;
  } else if (panel.status === 'record') {
    body = (
      <PassageFallback
        message={
          record === 'note'
            ? 'This source is a note saved in Shodh. Its indexed text is shown below.'
            : 'This source is an item in your Shodh calendar. Its indexed details are shown below; use Open in Calendar to see or edit it.'
        }
        passage={passage}
        tone="info"
      />
    );
  } else if (panel.status === 'unavailable') {
    body = <PassageFallback message={unavailableMessage(panel.error, panel.info)} passage={passage} />;
  } else {
    const { info: fileInfo, viewer } = panel;
    let viewerNode: React.ReactNode;
    if (viewer === 'pdf') {
      viewerNode = (
        <PdfViewer
          key={fileInfo.path}
          filePath={hit.sourceFile}
          fileSize={fileInfo.sizeBytes}
          fileModifiedMs={fileInfo.modifiedMs}
          passage={passage}
          citedPages={hit.page}
          regions={hit.regions ?? null}
          onLocate={setLocate}
          onFatal={handlePdfFatal}
          onPageChange={onPageChange}
        />
      );
    } else if (viewer === 'table') {
      viewerNode = <TableViewer key={fileInfo.path} filePath={hit.sourceFile} passage={passage} onLocate={setLocate} onError={handleViewerError} />;
    } else if (viewer === 'image') {
      viewerNode = (
        <ImageViewer
          key={fileInfo.path}
          filePath={hit.sourceFile}
          fileName={fileInfo.fileName}
          mimeType={fileInfo.mimeType ?? 'application/octet-stream'}
          passage={passage}
          onLocate={setLocate}
          onError={handleViewerError}
        />
      );
    } else {
      viewerNode = (
        <TextViewer
          key={`${fileInfo.path}:text`}
          filePath={hit.sourceFile}
          passage={passage}
          code={!PROSE_EXTENSIONS.has(fileInfo.extension)}
          onLocate={setLocate}
          onError={handleViewerError}
        />
      );
    }
    body = (
      <>
        <LocateNotice result={locate} reason={panel.reason} passage={passage} onRetry={panel.retryPdf ? retryPdf : undefined} />
        {/* Selected text in the document can be asked about ("Ask about this", features/focus/SelectionAsk). */}
        <div
          className="flex-1 min-h-0 min-w-0 flex flex-col"
          data-ask-scope="document"
          data-source-file={hit.sourceFile}
          data-file-name={fileInfo.fileName}
        >
          <ViewerBoundary
            key={`${fileInfo.path}:${viewer}`}
            fallback={error => <PassageFallback message={`The document viewer failed (${error.message}).`} passage={passage} />}
          >
            {viewerNode}
          </ViewerBoundary>
        </div>
      </>
    );
  }

  return { panel, info, passage, body };
}

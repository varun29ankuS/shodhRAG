import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { AlertTriangle, Loader2 } from 'lucide-react';
import { ImageViewer } from '../ask/viewer/ImageViewer';
import { PdfViewer } from '../ask/viewer/LazyPdfViewer';
import { TableViewer } from '../ask/viewer/TableViewer';
import { TextViewer } from '../ask/viewer/TextViewer';
import { getSourceFileInfo, toSourceError } from '../ask/viewer/sourceAccess';
import type { SourceAccessError, SourceFileInfo } from '../ask/viewer/sourceAccess';
import { extensionOf } from './fileTree';
import type { PdfRegion } from '../ask/types';
import type { SnippetRect } from '../research/types';

/** Extensions shown as prose (proportional font); other text is code. */
const PROSE_EXTENSIONS = new Set(['pdf', 'docx', 'pptx', 'txt', 'text', 'md', 'markdown', 'mdx', 'rst', 'log', 'html', 'htm']);

/** A quick file switch shows nothing rather than flashing a spinner. */
const SPINNER_DELAY_MS = 180;

type ViewerMode = 'pdf' | 'text' | 'table' | 'image';

/** Every state names the file it belongs to, so a stale one is never shown for a new path. */
type State =
  | { status: 'loading'; path: string }
  | { status: 'unavailable'; path: string; error: SourceAccessError; info: SourceFileInfo | null }
  | { status: 'view'; path: string; info: SourceFileInfo; viewer: ViewerMode; note: string | null };

function unavailableMessage(error: SourceAccessError, info: SourceFileInfo | null): string {
  switch (error.kind) {
    case 'notIndexed':
      return 'This file is not in your index, so Shodh cannot show it here. Open it in its default app instead.';
    case 'notFound':
      return 'This file is no longer at its indexed location.';
    case 'tooLarge':
      return 'This file is too large to show here. Open it in its default app instead.';
    case 'unsupported':
      return info ? `Shodh cannot display .${info.extension} files here. Open it in its default app instead.` : error.message;
    default:
      return error.message;
  }
}

/** Catches render-time failures of a viewer. */
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

/** Viewers report where a cited passage is; a browsed file has none. */
const ignoreLocate = () => {};

interface FileViewerProps {
  /** Path of an indexed file. */
  path: string;
  /** Called once the first page of a PDF is on screen (prefetch neighbours then). */
  onFirstPageVisible?: () => void;
  /** Workspace (source id) snippets made in this file belong to. */
  workspace?: string | null;
  /** A place to show with its box outlined (a result cell, a snippet); PDFs only. */
  highlight?: FileHighlight | null;
}

/** A page of a PDF with boxes to outline. */
export interface FileHighlight {
  page: number;
  regions?: PdfRegion[] | null;
  rects?: { page: number; rect: SnippetRect }[] | null;
}

const NO_PAGES = null;

/**
 * An indexed file shown in the app with the same viewers as cited sources
 * (PDF pages, text, spreadsheet grid, image), opened from the start (or where
 * the reader left it) with no passage highlighted. A PDF's viewer and its
 * remembered page skeleton appear at once, while its size is looked up.
 * Every failure says why, never a blank pane.
 */
export function FileViewer({ path, onFirstPageVisible, workspace = null, highlight = null }: FileViewerProps) {
  const cited = useMemo(() => (highlight ? { start: highlight.page, end: highlight.page } : NO_PAGES), [highlight]);
  const [state, setState] = useState<State>({ status: 'loading', path });

  useEffect(() => {
    let cancelled = false;
    setState({ status: 'loading', path });
    getSourceFileInfo(path)
      .then(info => {
        if (cancelled) return;
        if (info.kind === 'unsupported') {
          setState({
            status: 'unavailable',
            path,
            info,
            error: toSourceError({ kind: 'unsupported', message: `Shodh cannot display .${info.extension} files here.` }),
          });
        } else {
          setState({ status: 'view', path, info, viewer: info.kind, note: null });
        }
      })
      .catch(error => {
        if (!cancelled) setState({ status: 'unavailable', path, error: toSourceError(error), info: null });
      });
    return () => {
      cancelled = true;
    };
  }, [path]);

  const handleError = useCallback(
    (error: unknown) => {
      setState(prev => ({
        status: 'unavailable',
        path,
        error: toSourceError(error),
        info: prev.path === path && (prev.status === 'view' || prev.status === 'unavailable') ? prev.info : null,
      }));
    },
    [path],
  );

  // pdf.js could not render the file: show the indexer's extracted text.
  const handlePdfFatal = useCallback(
    (error: unknown) => {
      setState(prev => {
        if (prev.path !== path) return prev;
        if (prev.status === 'view') {
          return { ...prev, viewer: 'text', note: 'The PDF pages could not be rendered, so its extracted text is shown instead.' };
        }
        // Failed before the size lookup finished: report it plainly.
        return prev.status === 'loading' ? { status: 'unavailable', path, error: toSourceError(error), info: null } : prev;
      });
    },
    [path],
  );

  const current: State = state.path === path ? state : { status: 'loading', path };

  if (current.status === 'unavailable') {
    return <Unavailable message={unavailableMessage(current.error, current.info)} />;
  }

  const guessedPdf = current.status === 'loading' && extensionOf(path) === 'pdf';
  if (current.status === 'loading' && !guessedPdf) return <DelayedLoading />;

  const viewer: ViewerMode = current.status === 'view' ? current.viewer : 'pdf';
  const info = current.status === 'view' ? current.info : null;
  const note = current.status === 'view' ? current.note : null;

  let body: React.ReactNode;
  if (viewer === 'pdf') {
    body = (
      <PdfViewer
        key={path}
        filePath={path}
        fileSize={info ? info.sizeBytes : 'pending'}
        fileModifiedMs={info?.modifiedMs ?? null}
        passage=""
        citedPages={cited}
        regions={highlight?.regions ?? null}
        rects={highlight?.rects ?? null}
        onLocate={ignoreLocate}
        onFatal={handlePdfFatal}
        rememberView={!highlight}
        onFirstPageVisible={onFirstPageVisible}
        workspace={workspace}
      />
    );
  } else if (viewer === 'table') {
    body = <TableViewer key={path} filePath={path} passage="" onLocate={ignoreLocate} onError={handleError} />;
  } else if (viewer === 'image') {
    body = (
      <ImageViewer
        key={path}
        filePath={path}
        fileName={info!.fileName}
        mimeType={info!.mimeType ?? 'image/png'}
        passage=""
        onLocate={ignoreLocate}
        onError={handleError}
      />
    );
  } else {
    body = (
      <TextViewer
        key={path}
        filePath={path}
        passage=""
        code={!PROSE_EXTENSIONS.has(info!.extension)}
        onLocate={ignoreLocate}
        onError={handleError}
        rememberView
      />
    );
  }

  return (
    <div className="h-full flex flex-col min-h-0">
      {note && (
        <p role="status" className="shrink-0 px-4 py-2 text-[12px] text-shodh-text-muted border-b border-shodh-border-subtle">
          {note}
        </p>
      )}
      <div className="flex-1 min-h-0 flex flex-col">
        <ViewerBoundary key={`${path}-${viewer}`} fallback={error => <Unavailable message={`The viewer failed: ${error.message}`} />}>
          {body}
        </ViewerBoundary>
      </div>
    </div>
  );
}

function DelayedLoading() {
  const [shown, setShown] = useState(false);
  useEffect(() => {
    const timer = window.setTimeout(() => setShown(true), SPINNER_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, []);
  return (
    <div className="h-full flex items-center justify-center gap-2 text-[12.5px] text-shodh-text-muted" role="status" aria-busy="true">
      {shown && (
        <>
          <Loader2 className="w-4 h-4 agent-spin" aria-hidden="true" />
          Opening…
        </>
      )}
    </div>
  );
}

function Unavailable({ message }: { message: string }) {
  return (
    <div className="h-full flex items-center justify-center p-6">
      <div className="max-w-sm flex flex-col items-center gap-2 text-center">
        <AlertTriangle className="w-5 h-5 text-shodh-warning" aria-hidden="true" />
        <p className="text-[13px] text-shodh-text-secondary">{message}</p>
      </div>
    </div>
  );
}

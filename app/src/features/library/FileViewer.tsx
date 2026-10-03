import React, { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, Loader2 } from 'lucide-react';
import { ImageViewer } from '../ask/viewer/ImageViewer';
import { PdfViewer } from '../ask/viewer/PdfViewer';
import { TableViewer } from '../ask/viewer/TableViewer';
import { TextViewer } from '../ask/viewer/TextViewer';
import { getSourceFileInfo, toSourceError } from '../ask/viewer/sourceAccess';
import type { SourceAccessError, SourceFileInfo } from '../ask/viewer/sourceAccess';

/** Extensions shown as prose (proportional font); other text is code. */
const PROSE_EXTENSIONS = new Set(['pdf', 'docx', 'pptx', 'txt', 'text', 'md', 'markdown', 'mdx', 'rst', 'log', 'html', 'htm']);

type ViewerMode = 'pdf' | 'text' | 'table' | 'image';

type State =
  | { status: 'loading' }
  | { status: 'unavailable'; error: SourceAccessError; info: SourceFileInfo | null }
  | { status: 'view'; info: SourceFileInfo; viewer: ViewerMode; note: string | null };

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
}

/**
 * An indexed file shown in the app with the same viewers as cited sources
 * (PDF pages, text, spreadsheet grid, image), opened from the start with no
 * passage highlighted. Every failure says why, never a blank pane.
 */
export function FileViewer({ path }: FileViewerProps) {
  const [state, setState] = useState<State>({ status: 'loading' });

  useEffect(() => {
    let cancelled = false;
    setState({ status: 'loading' });
    getSourceFileInfo(path)
      .then(info => {
        if (cancelled) return;
        if (info.kind === 'unsupported') {
          setState({
            status: 'unavailable',
            info,
            error: toSourceError({ kind: 'unsupported', message: `Shodh cannot display .${info.extension} files here.` }),
          });
        } else {
          setState({ status: 'view', info, viewer: info.kind, note: null });
        }
      })
      .catch(error => {
        if (!cancelled) setState({ status: 'unavailable', error: toSourceError(error), info: null });
      });
    return () => {
      cancelled = true;
    };
  }, [path]);

  const handleError = useCallback((error: unknown) => {
    setState(prev => ({
      status: 'unavailable',
      error: toSourceError(error),
      info: prev.status === 'view' || prev.status === 'unavailable' ? prev.info : null,
    }));
  }, []);

  // pdf.js could not render the file: show the indexer's extracted text.
  const handlePdfFatal = useCallback(() => {
    setState(prev =>
      prev.status === 'view'
        ? { ...prev, viewer: 'text', note: 'The PDF pages could not be rendered, so its extracted text is shown instead.' }
        : prev,
    );
  }, []);

  if (state.status === 'loading') {
    return (
      <div className="h-full flex items-center justify-center gap-2 text-[12.5px] text-shodh-text-muted" role="status">
        <Loader2 className="w-4 h-4 agent-spin" aria-hidden="true" />
        Opening…
      </div>
    );
  }

  if (state.status === 'unavailable') {
    return <Unavailable message={unavailableMessage(state.error, state.info)} />;
  }

  const { info, viewer, note } = state;
  let body: React.ReactNode;
  if (viewer === 'pdf') {
    body = <PdfViewer key={info.path} filePath={path} passage="" citedPages={null} onLocate={ignoreLocate} onFatal={handlePdfFatal} />;
  } else if (viewer === 'table') {
    body = <TableViewer key={info.path} filePath={path} passage="" onLocate={ignoreLocate} onError={handleError} />;
  } else if (viewer === 'image') {
    body = (
      <ImageViewer
        key={info.path}
        filePath={path}
        fileName={info.fileName}
        mimeType={info.mimeType ?? 'image/png'}
        passage=""
        onLocate={ignoreLocate}
        onError={handleError}
      />
    );
  } else {
    body = (
      <TextViewer
        key={info.path}
        filePath={path}
        passage=""
        code={!PROSE_EXTENSIONS.has(info.extension)}
        onLocate={ignoreLocate}
        onError={handleError}
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
      <div className="flex-1 min-h-0">
        <ViewerBoundary key={`${info.path}-${viewer}`} fallback={error => <Unavailable message={`The viewer failed: ${error.message}`} />}>
          {body}
        </ViewerBoundary>
      </div>
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

import React, { useCallback, useEffect, useId, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { AlertTriangle, CheckCircle2, ExternalLink, Info, Loader2, Maximize2, Minimize2, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { formatLocation, isWebUrl, jumpToSourceArgs } from './searchResults';
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

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';

type ViewerMode = 'pdf' | 'text' | 'table' | 'image';

type PanelState =
  | { status: 'loading' }
  | { status: 'web' }
  | { status: 'unavailable'; error: SourceAccessError; info: SourceFileInfo | null }
  | { status: 'view'; info: SourceFileInfo; viewer: ViewerMode; reason: string | null };

function shortFolder(path: string): string | null {
  const idx = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  const folder = idx > 0 ? path.slice(0, idx) : path;
  const parts = folder.split(/[/\\]/).filter(Boolean);
  return parts.slice(-2).join(' › ') || null;
}

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
function PassageFallback({ message, passage }: { message: string; passage: string }) {
  return (
    <div tabIndex={0} aria-label="Cited passage" className={cn('flex-1 min-h-0 overflow-y-auto scrollbar-thin p-5 flex flex-col gap-4', FOCUS_RING)}>
      <p role="alert" className="flex items-start gap-2 text-[13px] leading-relaxed text-shodh-text-secondary">
        <AlertTriangle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
        <span>{message}</span>
      </p>
      <figure className="m-0 flex flex-col gap-2">
        <figcaption className="text-[11.5px] font-medium uppercase tracking-wider text-shodh-text-faint">Retrieved passage</figcaption>
        <PassageBlock passage={passage} />
      </figure>
    </div>
  );
}

function LocateNotice({ result, reason, passage }: { result: LocateResult | null; reason: string | null; passage: string }) {
  const showPassage = result !== null && (result.status === 'notFound' || result.status === 'approximate');
  const Icon =
    result?.status === 'found' ? CheckCircle2 : result?.status === 'searching' ? Loader2 : result ? AlertTriangle : Info;
  return (
    <div className="px-4 py-2 border-b border-shodh-border-subtle flex flex-col gap-1.5">
      {reason && (
        <p className="flex items-start gap-2 text-[12.5px] leading-snug text-shodh-text-secondary">
          <Info className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-info" aria-hidden="true" />
          <span>{reason}</span>
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

interface SourcePreviewProps {
  hit: SearchHit;
  /** Other passages from the same file in this answer, for quick switching. */
  siblings: readonly SearchHit[];
  onSelectHit: (hit: SearchHit) => void;
  onClose: () => void;
}

/**
 * Slide-over document viewer for a cited source: the actual document (PDF
 * pages, full text, spreadsheet grid or image) opened at the citation with
 * the cited passage highlighted. Every failure path degrades to the retrieved
 * passage with an explanation, never an empty panel.
 */
export function SourcePreview({ hit, siblings, onSelectHit, onClose }: SourcePreviewProps) {
  const titleId = useId();
  const panelRef = useRef<HTMLElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const [panel, setPanel] = useState<PanelState>({ status: 'loading' });
  const [locate, setLocate] = useState<LocateResult | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [opening, setOpening] = useState(false);
  const isUrl = isWebUrl(hit.sourceFile);
  const passage = hit.text.trim() || hit.snippet.trim();

  // Move focus into the panel when it opens or switches to another passage.
  useEffect(() => {
    closeRef.current?.focus();
  }, [hit.number, hit.sourceFile]);

  // Esc closes the viewer. Capture phase + preventDefault so the Ask view's
  // Esc-to-stop handler (which checks defaultPrevented) leaves a streaming
  // answer alone; propagation continues for other listeners.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.defaultPrevented) return;
      e.preventDefault();
      onClose();
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, [onClose]);

  // Resolve the file (access-checked by the backend) and pick a viewer.
  useEffect(() => {
    setLocate(null);
    if (isUrl) {
      setPanel({ status: 'web' });
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
          setPanel({ status: 'view', info, viewer: info.kind, reason: null });
        }
      })
      .catch(error => {
        if (!cancelled) setPanel({ status: 'unavailable', error: toSourceError(error), info: null });
      });
    return () => {
      cancelled = true;
    };
  }, [hit.sourceFile, isUrl]);

  const handleViewerError = useCallback((error: unknown) => {
    setLocate(null);
    setPanel(prev => ({
      status: 'unavailable',
      error: toSourceError(error),
      info: prev.status === 'view' || prev.status === 'unavailable' ? prev.info : null,
    }));
  }, []);

  // pdf.js could not open the file: fall back to the indexer's extracted text.
  const handlePdfFatal = useCallback((error: unknown) => {
    const sourceError = toSourceError(error);
    const detail =
      error instanceof Error && error.name === 'PasswordException'
        ? 'it is password-protected'
        : sourceError.kind === 'unknown'
          ? 'it could not be rendered'
          : sourceError.message;
    setLocate(null);
    setPanel(prev =>
      prev.status === 'view'
        ? { ...prev, viewer: 'text', reason: `The PDF pages are not shown because ${detail}. Showing its extracted text instead.` }
        : prev,
    );
  }, []);

  const openSource = async () => {
    if (isUrl) {
      window.open(hit.sourceFile, '_blank', 'noopener,noreferrer');
      return;
    }
    setOpening(true);
    try {
      await invoke('jump_to_source', jumpToSourceArgs(hit));
    } catch (error) {
      notify.error('Could not open file', { description: String(error) });
    } finally {
      setOpening(false);
    }
  };

  // Keep keyboard focus inside the panel while it is open. Focusable elements
  // are queried on every Tab because viewer content renders asynchronously.
  const trapFocus = (e: React.KeyboardEvent<HTMLElement>) => {
    if (e.key !== 'Tab' || !panelRef.current) return;
    const focusables = Array.from(panelRef.current.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
      el => el.getClientRects().length > 0,
    );
    if (focusables.length === 0) return;
    const firstEl = focusables[0];
    const lastEl = focusables[focusables.length - 1];
    const activeEl = document.activeElement;
    if (e.shiftKey && (activeEl === firstEl || !panelRef.current.contains(activeEl))) {
      e.preventDefault();
      lastEl.focus();
    } else if (!e.shiftKey && activeEl === lastEl) {
      e.preventDefault();
      firstEl.focus();
    }
  };

  const info = panel.status === 'view' || panel.status === 'unavailable' ? panel.info : null;
  const fileName = info?.fileName ?? hit.fileName;
  const location = formatLocation(hit);
  const folder = isUrl ? null : shortFolder(info?.path ?? hit.sourceFile);
  const subtitle = [location, folder].filter(Boolean).join(' · ');
  const otherPassages = siblings.filter(s => s.number !== hit.number);
  const fileMissing = panel.status === 'unavailable' && panel.error.kind === 'notFound';

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
  } else if (panel.status === 'unavailable') {
    body = <PassageFallback message={unavailableMessage(panel.error, panel.info)} passage={passage} />;
  } else {
    const { info: fileInfo, viewer } = panel;
    let viewerNode: React.ReactNode;
    if (viewer === 'pdf') {
      viewerNode = (
        <PdfViewer key={fileInfo.path} filePath={hit.sourceFile} passage={passage} citedPages={hit.page} onLocate={setLocate} onFatal={handlePdfFatal} />
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
        <LocateNotice result={locate} reason={panel.reason} passage={passage} />
        <ViewerBoundary
          key={`${fileInfo.path}:${viewer}`}
          fallback={error => <PassageFallback message={`The document viewer failed (${error.message}).`} passage={passage} />}
        >
          {viewerNode}
        </ViewerBoundary>
      </>
    );
  }

  return (
    <aside
      ref={panelRef}
      role="dialog"
      aria-modal="true"
      aria-labelledby={titleId}
      onKeyDown={trapFocus}
      className={cn(
        'ask-slide-in absolute top-3 right-3 bottom-3 z-20 flex flex-col overflow-hidden rounded-[18px] border border-shodh-border-strong bg-shodh-surface shadow-[-20px_0_60px_rgba(0,0,0,0.35)]',
        expanded ? 'left-3' : 'w-[min(720px,max(55vw,420px),calc(100%-24px))]',
      )}
    >
      <header className="flex items-center gap-2.5 pl-[18px] pr-3.5 pt-3.5 pb-3 border-b border-shodh-border-subtle">
        <span
          className="w-[22px] h-[22px] shrink-0 rounded-md bg-shodh-accent text-shodh-on-accent inline-flex items-center justify-center text-[11px] font-bold tabular-nums"
          aria-label={`Source ${hit.number}`}
        >
          {hit.number}
        </span>
        <div className="flex flex-col min-w-0 flex-1">
          <h2 id={titleId} className="text-[13.5px] font-semibold text-shodh-text truncate" title={info?.path ?? hit.sourceFile}>
            {fileName}
          </h2>
          {subtitle && <span className="text-[11.5px] text-shodh-text-muted truncate">{subtitle}</span>}
        </div>
        <button
          type="button"
          onClick={openSource}
          disabled={opening || fileMissing}
          className={cn(
            'h-[30px] px-2.5 shrink-0 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border-strong text-[12px] text-shodh-text hover:bg-shodh-raised disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
            FOCUS_RING,
          )}
        >
          {opening ? <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" />}
          {isUrl ? 'Open in browser' : 'Open in default app'}
        </button>
        <button
          type="button"
          onClick={() => setExpanded(v => !v)}
          aria-label={expanded ? 'Collapse viewer' : 'Expand viewer to full width'}
          aria-pressed={expanded}
          className={cn(
            'w-[30px] h-[30px] shrink-0 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
            FOCUS_RING,
          )}
        >
          {expanded ? <Minimize2 className="w-4 h-4" aria-hidden="true" /> : <Maximize2 className="w-4 h-4" aria-hidden="true" />}
        </button>
        <button
          ref={closeRef}
          type="button"
          onClick={onClose}
          aria-label="Close source viewer"
          className={cn(
            'w-[30px] h-[30px] shrink-0 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
            FOCUS_RING,
          )}
        >
          <X className="w-4 h-4" strokeWidth={2.4} aria-hidden="true" />
        </button>
      </header>

      {otherPassages.length > 0 && (
        <nav aria-label="Other passages from this file" className="flex items-center gap-1.5 px-4 py-2 border-b border-shodh-border-subtle overflow-x-auto scrollbar-thin">
          <span className="shrink-0 text-[11.5px] text-shodh-text-faint">Also cited:</span>
          {otherPassages.map(s => {
            const where = formatLocation(s);
            return (
              <button
                key={s.number}
                type="button"
                onClick={() => onSelectHit(s)}
                aria-label={`Show passage ${s.number}${where ? `, ${where}` : ''}`}
                className={cn(
                  'shrink-0 h-7 px-2 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border bg-shodh-surface-2 text-[12px] text-shodh-text-secondary hover:bg-shodh-raised transition-colors duration-micro',
                  FOCUS_RING,
                )}
              >
                <span className="font-bold tabular-nums">{s.number}</span>
                {where && <span className="text-shodh-text-muted">{where}</span>}
              </button>
            );
          })}
        </nav>
      )}

      {body}
    </aside>
  );
}

export default SourcePreview;

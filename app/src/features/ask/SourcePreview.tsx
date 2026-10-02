import React, { useEffect, useId, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ExternalLink, Loader2, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { fileExtensionOf, formatLocation, isWebUrl, jumpToSourceArgs } from './searchResults';
import type { SearchHit } from './types';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

/** Return shape of the `get_document_preview` command (rag_commands.rs). */
interface DocumentPreviewInfo {
  file_name?: string;
  file_type?: string;
  file_size?: string;
  file_path?: string;
}

type PreviewState =
  | { status: 'loading' }
  | { status: 'ready'; info: DocumentPreviewInfo }
  | { status: 'missing'; message: string };

interface SourcePreviewProps {
  hit: SearchHit;
  /** Other passages from the same file in this answer, for quick switching. */
  siblings: readonly SearchHit[];
  onSelectHit: (hit: SearchHit) => void;
  onClose: () => void;
}

function folderOf(path: string): string | null {
  const idx = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  if (idx <= 0) return null;
  const folder = path.slice(0, idx);
  const parts = folder.split(/[/\\]/).filter(Boolean);
  return parts.slice(-2).join(' › ') || null;
}

/**
 * Slide-over showing a cited passage: the exact text retrieved for the answer,
 * file details from `get_document_preview`, and an action to open the file.
 */
export function SourcePreview({ hit, siblings, onSelectHit, onClose }: SourcePreviewProps) {
  const titleId = useId();
  const closeRef = useRef<HTMLButtonElement>(null);
  const [preview, setPreview] = useState<PreviewState>({ status: 'loading' });
  const [opening, setOpening] = useState(false);
  const isUrl = isWebUrl(hit.sourceFile);

  // Move focus into the panel when it opens or switches to another passage.
  useEffect(() => {
    closeRef.current?.focus();
  }, [hit.number, hit.sourceFile]);

  // Esc closes the preview. Capture phase + preventDefault so the Ask view's
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

  useEffect(() => {
    if (isUrl) {
      setPreview({ status: 'ready', info: {} });
      return;
    }
    let cancelled = false;
    setPreview({ status: 'loading' });
    invoke<DocumentPreviewInfo>('get_document_preview', {
      filePath: hit.sourceFile,
      pageNumber: hit.page ? hit.page.start : null,
      lineRange: hit.lineRange,
    })
      .then(info => {
        if (!cancelled) setPreview({ status: 'ready', info: info ?? {} });
      })
      .catch(error => {
        if (!cancelled) setPreview({ status: 'missing', message: String(error) });
      });
    return () => {
      cancelled = true;
    };
  }, [hit.sourceFile, hit.page, hit.lineRange, isUrl]);

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

  const location = formatLocation(hit);
  const folder = isUrl ? null : folderOf(hit.sourceFile);
  const info = preview.status === 'ready' ? preview.info : null;
  const typeLabel = (info?.file_type || fileExtensionOf(hit.sourceFile)).toUpperCase();
  const subtitle = [location, folder].filter(Boolean).join(' · ');
  const passage = hit.text.trim() || hit.snippet.trim();
  const otherPassages = siblings.filter(s => s.number !== hit.number);

  return (
    <aside
      role="dialog"
      aria-modal="false"
      aria-labelledby={titleId}
      className="ask-slide-in absolute top-3 right-3 bottom-3 z-20 w-[min(420px,calc(100%-24px))] flex flex-col rounded-[18px] border border-shodh-border-strong bg-shodh-surface shadow-[-20px_0_60px_rgba(0,0,0,0.35)]"
    >
      <header className="flex items-center gap-2.5 pl-[18px] pr-3.5 pt-3.5 pb-3 border-b border-shodh-border-subtle">
        <span
          className="w-[22px] h-[22px] shrink-0 rounded-md bg-shodh-accent text-shodh-on-accent inline-flex items-center justify-center text-[11px] font-bold tabular-nums"
          aria-label={`Source ${hit.number}`}
        >
          {hit.number}
        </span>
        <div className="flex flex-col min-w-0 flex-1">
          <h2 id={titleId} className="text-[13.5px] font-semibold text-shodh-text truncate" title={hit.fileName}>
            {hit.fileName}
          </h2>
          {subtitle && <span className="text-[11.5px] text-shodh-text-muted truncate">{subtitle}</span>}
        </div>
        <button
          type="button"
          onClick={openSource}
          disabled={opening || preview.status === 'missing'}
          className={cn(
            'h-[30px] px-2.5 shrink-0 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border-strong text-[12px] text-shodh-text hover:bg-shodh-raised disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
            FOCUS_RING,
          )}
        >
          {opening ? <Loader2 className="w-3.5 h-3.5 animate-spin" aria-hidden="true" /> : <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" />}
          Open file
        </button>
        <button
          ref={closeRef}
          type="button"
          onClick={onClose}
          aria-label="Close source preview"
          className={cn(
            'w-[30px] h-[30px] shrink-0 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
            FOCUS_RING,
          )}
        >
          <X className="w-4 h-4" strokeWidth={2.4} aria-hidden="true" />
        </button>
      </header>

      <div className="flex-1 overflow-y-auto scrollbar-thin p-5 flex flex-col gap-4">
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[11.5px] text-shodh-text-muted">
          {typeLabel && (
            <span className="px-1.5 py-0.5 rounded bg-shodh-raised-2 font-semibold text-[10px] tracking-wide text-shodh-text-secondary">
              {typeLabel}
            </span>
          )}
          {info?.file_size && <span>{info.file_size}</span>}
          {preview.status === 'loading' && (
            <span className="inline-flex items-center gap-1">
              <Loader2 className="w-3 h-3 animate-spin" aria-hidden="true" />
              Checking file…
            </span>
          )}
        </div>

        {preview.status === 'missing' && (
          <p role="alert" className="text-[12.5px] leading-relaxed text-shodh-error">
            This file is no longer at its indexed location, so it cannot be opened. The passage below is the text that was retrieved.
          </p>
        )}

        <figure className="m-0 flex flex-col gap-2">
          <figcaption className="text-[11.5px] font-medium uppercase tracking-wider text-shodh-text-faint">
            Retrieved passage
          </figcaption>
          <blockquote className="m-0 rounded-xl border border-shodh-border bg-shodh-surface-2 px-4 py-3.5 text-[14px] leading-[1.7] text-shodh-text-secondary whitespace-pre-wrap break-words">
            {passage || 'No text was stored for this passage.'}
          </blockquote>
        </figure>

        <p className="text-[12.5px] leading-relaxed text-shodh-text-muted">
          Retrieved as passage {hit.number} for this question.
        </p>

        {otherPassages.length > 0 && (
          <div className="flex flex-col gap-2">
            <h3 className="text-[11.5px] font-medium uppercase tracking-wider text-shodh-text-faint">
              Other passages from this file
            </h3>
            <ul className="flex flex-wrap gap-1.5">
              {otherPassages.map(s => {
                const where = formatLocation(s);
                return (
                  <li key={s.number}>
                    <button
                      type="button"
                      onClick={() => onSelectHit(s)}
                      className={cn(
                        'h-7 px-2 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border bg-shodh-surface-2 text-[12px] text-shodh-text-secondary hover:bg-shodh-raised transition-colors duration-micro',
                        FOCUS_RING,
                      )}
                    >
                      <span className="font-bold tabular-nums">{s.number}</span>
                      {where && <span className="text-shodh-text-muted">{where}</span>}
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>
        )}
      </div>
    </aside>
  );
}

export default SourcePreview;

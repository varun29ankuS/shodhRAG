import React, { useEffect, useId, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ExternalLink, Loader2, Maximize2, MessageSquareText, Minimize2, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { FOCUS_OVERLAY_SELECTOR } from '../focus/focusDom';
import { appRecordKind, formatLocation, isWebUrl, jumpToSourceArgs, sourceLabel } from './searchResults';
import type { ViewTab } from '../../lib/viewTabs';
import type { SearchHit } from './types';
import { useSourceDocument } from './useSourceDocument';
import { VIEWER_FOCUS_RING as FOCUS_RING } from './viewer/viewerTypes';

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';

function shortFolder(path: string): string | null {
  const idx = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  const folder = idx > 0 ? path.slice(0, idx) : path;
  const parts = folder.split(/[/\\]/).filter(Boolean);
  return parts.slice(-2).join(' › ') || null;
}

interface SourcePreviewProps {
  hit: SearchHit;
  /** Other passages from the same file in this answer, for quick switching. */
  siblings: readonly SearchHit[];
  onSelectHit: (hit: SearchHit) => void;
  onClose: () => void;
  /** Switch to an app view; used to open a cited task or event in Calendar. */
  onOpenView?: (tab: ViewTab) => void;
  /** Open this source in the focus pop-out to zoom and ask about it. */
  onFocus?: (trigger: HTMLElement) => void;
}

/**
 * Slide-over document viewer for a cited source: the actual document (PDF
 * pages, full text, spreadsheet grid or image) opened at the citation with
 * the cited passage highlighted. Every failure path degrades to the retrieved
 * passage with an explanation, never an empty panel.
 */
export function SourcePreview({ hit, siblings, onSelectHit, onClose, onOpenView, onFocus }: SourcePreviewProps) {
  const titleId = useId();
  const panelRef = useRef<HTMLElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [opening, setOpening] = useState(false);
  const isUrl = isWebUrl(hit.sourceFile);
  const record = appRecordKind(hit.sourceFile);
  const { panel, info, body } = useSourceDocument(hit);

  // Move focus into the panel when it opens or switches to another passage.
  useEffect(() => {
    closeRef.current?.focus();
  }, [hit.number, hit.sourceFile]);

  // Esc closes the viewer. Capture phase + preventDefault so the Ask view's
  // Esc-to-stop handler (which checks defaultPrevented) leaves a streaming
  // answer alone; propagation continues for other listeners.
  // Esc inside a focus pop-out opened from here belongs to the pop-out.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.defaultPrevented) return;
      if (e.target instanceof Element && e.target.closest(FOCUS_OVERLAY_SELECTOR)) return;
      e.preventDefault();
      onClose();
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, [onClose]);

  const openSource = async () => {
    if (record) {
      if (record !== 'note') {
        onOpenView?.('tasks');
        onClose();
      }
      return;
    }
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

  const fileName = record ? sourceLabel(hit) : info?.fileName ?? hit.fileName;
  const location = formatLocation(hit);
  const folder = isUrl || record ? null : shortFolder(info?.path ?? hit.sourceFile);
  const subtitle = [location, folder].filter(Boolean).join(' · ');
  const otherPassages = siblings.filter(s => s.number !== hit.number);
  const fileMissing = panel.status === 'unavailable' && panel.error.kind === 'notFound';

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
          <h2 id={titleId} className="text-[13.5px] font-semibold text-shodh-text truncate" title={record ? fileName : info?.path ?? hit.sourceFile}>
            {fileName}
          </h2>
          {subtitle && <span className="text-[11.5px] text-shodh-text-muted truncate">{subtitle}</span>}
        </div>
        <button
          type="button"
          onClick={openSource}
          disabled={opening || fileMissing || record === 'note' || (record !== null && !onOpenView)}
          className={cn(
            'h-[30px] px-2.5 shrink-0 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border-strong text-[12px] text-shodh-text hover:bg-shodh-raised disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
            FOCUS_RING,
          )}
        >
          {opening ? <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" />}
          {record ? 'Open in Calendar' : isUrl ? 'Open in browser' : 'Open in default app'}
        </button>
        {onFocus && (
          <button
            type="button"
            onClick={e => onFocus(e.currentTarget)}
            className={cn(
              'h-[30px] px-2.5 shrink-0 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border-strong text-[12px] text-shodh-text hover:bg-shodh-raised transition-colors duration-micro',
              FOCUS_RING,
            )}
            title="Open in the focus view to zoom and ask about this source"
          >
            <MessageSquareText className="w-3.5 h-3.5" aria-hidden="true" />
            Ask about this
          </button>
        )}
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

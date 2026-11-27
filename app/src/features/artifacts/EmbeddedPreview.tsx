/**
 * The creation's page, live inside Artifacts (`creations/preview_embed.rs`). The page runs in a
 * locked child webview of its own; this component renders the empty box it is placed over and
 * reports that box (and whether anything of this page is above it) to Rust once a frame at
 * most. Rust shows the page only while the latest report says it is on screen and uncovered;
 * leaving the pane reports it gone. While it shows, the chat floats over it in a layer of its
 * own (`features/dock`). Pop out moves the page to its own window; Show here brings it back.
 *
 * The box takes all the height its parent leaves (the page is the main thing on screen), and
 * the status line says what the page is doing, or, when something of the app's own hides it,
 * what that is.
 */
import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { AppWindow, EyeOff, Loader2, PanelBottomOpen, RefreshCw, ShieldAlert, ExternalLink, X } from 'lucide-react';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { useTheme } from '../../contexts/ThemeContext';
import { OVERLAY_SELECTORS, coveredBy, coveringOverlay, dockCornerBox, layerVisible, overlayName } from '../../lib/previewPlacement';
import type { Box, NamedBox } from '../../lib/previewPlacement';
import { sidePane } from '../../lib/sidePane';
import { PANEL_EDGE, PANEL_HEIGHT, PANEL_WIDTH } from '../../components/shell/DockPanel';
import { useCodeReview } from '../code/codeReviewHost';
import { dockLayer } from '../dock/layerState';
import { creationsApi, errorText, onPreviewEvent } from './api';
import { PREVIEW_BLOCKED_TEXT, previewLoadAfter, previewLoadAtEmbed, previewStateLine } from './model';
import type { PreviewEnd, PreviewEvent, PreviewInfo, PreviewLoad, PreviewState } from './model';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';
const QUIET_BUTTON = cn(
  'h-8 px-2.5 shrink-0 inline-flex items-center gap-1.5 rounded-lg text-small text-shodh-text-secondary whitespace-nowrap hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro disabled:opacity-40',
  FOCUS_RING,
);
const OUTLINE_BUTTON = cn(
  'h-8 px-3 shrink-0 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border text-small font-medium text-shodh-text whitespace-nowrap hover:bg-shodh-raised transition-colors duration-micro disabled:opacity-40',
  FOCUS_RING,
);

/**
 * In compact width (the artifact page's `data-compact`, set when it is narrow): a button's
 * words are for screen readers only; its icon and title still show, and the row stays one row.
 */
export const COMPACT_LABEL = 'group-data-[compact=true]/artifact:sr-only';

/** Labels whose reports are kept while the pane does not know its label yet, at most. */
const MAX_EARLY_LABELS = 4;

function boxOf(rect: DOMRect): Box {
  return { x: rect.left, y: rect.top, width: rect.width, height: rect.height };
}

/** Where the floating chat's webview sits: the bottom-right corner, left of a docked pane. */
function dockCorner(): Box {
  return dockCornerBox(
    { width: window.innerWidth, height: window.innerHeight },
    { width: PANEL_WIDTH, height: PANEL_HEIGHT },
    PANEL_EDGE,
    sidePane.get(),
  );
}

/** The app's own overlays now, each with the name the status line says it by. */
function overlaysNow(box: HTMLElement): NamedBox[] {
  return Array.from(document.querySelectorAll<HTMLElement>(OVERLAY_SELECTORS.join(',')))
    .filter(element => !box.contains(element))
    .map(element => ({
      name: overlayName({
        label: element.getAttribute('aria-label'),
        role: element.getAttribute('role'),
        toast: element.hasAttribute('data-sonner-toast'),
        palette: element.hasAttribute('cmdk-root'),
      }),
      box: boxOf(element.getBoundingClientRect()),
    }));
}

/** The open modal dialog's name, if one is open. */
function modalNow(): string | null {
  const modal = document.querySelector<HTMLElement>('[role="dialog"][aria-modal="true"], [role="alertdialog"]');
  return modal ? overlayName({ label: modal.getAttribute('aria-label'), role: modal.getAttribute('role'), toast: false, palette: false }) : null;
}

interface EmbeddedPreviewProps {
  creationId: string;
  version: number | null;
  network: boolean;
  /** Increases when the files changed and the page should load again. */
  reloadSeq: number;
  /** Increases when the page was asked for again (Preview, the assistant): a popped-out window comes to the front. */
  focusSeq: number;
  /** The start of the toolbar row (the page's panes and version), so the page gets the rest of the height. */
  leading?: React.ReactNode;
  /** The internet switch, at the toolbar's end. */
  networkControl?: React.ReactNode;
}

export function EmbeddedPreview({ creationId, version, network, reloadSeq, focusSeq, leading, networkControl }: EmbeddedPreviewProps) {
  const { theme } = useTheme();
  const review = useCodeReview();
  const [label, setLabel] = useState<string | null>(null);
  const [poppedOut, setPoppedOut] = useState(false);
  const [status, setStatus] = useState<PreviewLoad>('opening');
  const [end, setEnd] = useState<PreviewEnd | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  // What of the app's own hides the page now, by name (null: nothing).
  const [hiddenBy, setHiddenBy] = useState<string | null>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const labelRef = useRef<string | null>(null);
  labelRef.current = label;
  // Labels this page closed or moved itself (their "closed" report needs no note).
  const leaving = useRef(new Set<string>());
  // Reports about a page whose label this pane has not learnt yet: a small local page can
  // finish loading before the embed answers.
  const early = useRef(new Map<string, PreviewState[]>());

  /** The page `info` describes is the one shown now: its state so far, from Rust's word and any early report. */
  const adopt = useCallback((info: PreviewInfo) => {
    labelRef.current = info.label;
    const before = early.current.get(info.label) ?? [];
    early.current.clear();
    setLabel(info.label);
    setStatus(previewLoadAtEmbed(info.loaded, before));
  }, []);

  // Embed (again) for this version and switch; Rust keeps the same page when nothing changed.
  useEffect(() => {
    if (poppedOut) return;
    let active = true;
    setStatus('opening');
    setError(null);
    creationsApi
      .embedPreview(creationId, version, network, theme === 'dark')
      .then(info => {
        if (!active) return;
        adopt(info);
        setEnd(null);
      })
      .catch(e => {
        if (active) setError(errorText(e));
      });
    return () => {
      active = false;
    };
  }, [creationId, version, network, poppedOut, attempt, theme, adopt]);

  // Rust's reports on the page.
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    onPreviewEvent((event: PreviewEvent) => {
      if (event.label !== labelRef.current) {
        // Kept until the embed says which page is this pane's (a few labels at most).
        const kept = early.current.get(event.label) ?? [];
        if (!early.current.has(event.label) && early.current.size >= MAX_EARLY_LABELS) {
          const oldest = early.current.keys().next().value;
          if (oldest !== undefined) early.current.delete(oldest);
        }
        early.current.set(event.label, [...kept, event.state]);
        return;
      }
      if (event.state === 'closed') {
        const ours = leaving.current.delete(event.label);
        setLabel(null);
        if (!ours) setEnd('closed');
        if (event.place === 'window') setPoppedOut(false);
        return;
      }
      const state = event.state;
      setStatus(prev => previewLoadAfter(prev, state));
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(e => console.error('Failed to listen for the preview:', e));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // The box's place, once a frame at most, on every change that could move or cover it.
  const embedded = label !== null && !poppedOut && end === null;
  useLayoutEffect(() => {
    if (!embedded || !label) {
      dockLayer.set(false);
      setHiddenBy(null);
      return;
    }
    let frame: number | null = null;
    // A report that says the same as the last one is not sent again.
    let lastSent: string | null = null;
    const report = () => {
      frame = null;
      const box = boxRef.current;
      if (!box) return;
      const pane = boxOf(box.getBoundingClientRect());
      const named = overlaysNow(box);
      const overlays = named.map(overlay => overlay.box);
      const modal = modalNow();
      const modalOpen = modal !== null;
      const covered = coveredBy(pane, overlays, modalOpen, dockCorner());
      setHiddenBy(covered ? coveringOverlay(pane, named, modal, dockCorner()) : null);
      const visible = layerVisible({
        onArtifacts: true,
        paneShown: true,
        poppedOut: false,
        covered,
        windowVisible: document.visibilityState === 'visible',
      });
      dockLayer.set(visible);
      const placement = visible ? { rect: pane, covered: false, rightInset: sidePane.get() } : { rect: null, covered: true };
      // The window's size too: the chat's corner follows it even when the pane stays put.
      const key = JSON.stringify([placement, window.innerWidth, window.innerHeight]);
      if (key === lastSent) return;
      lastSent = key;
      creationsApi
        .placePreview(label, placement)
        .catch(e => console.error('The preview could not be placed:', e));
    };
    const schedule = () => {
      if (frame === null) frame = window.requestAnimationFrame(report);
    };
    schedule();
    const resize = new ResizeObserver(schedule);
    if (boxRef.current) resize.observe(boxRef.current);
    const mutations = new MutationObserver(schedule);
    mutations.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['role', 'aria-modal', 'data-state', 'class', 'style'] });
    // A pane docking or leaving at the right moves the chat's corner.
    const unsubscribe = sidePane.subscribe(schedule);
    window.addEventListener('scroll', schedule, true);
    window.addEventListener('resize', schedule);
    document.addEventListener('visibilitychange', schedule);
    return () => {
      if (frame !== null) window.cancelAnimationFrame(frame);
      resize.disconnect();
      mutations.disconnect();
      unsubscribe();
      window.removeEventListener('scroll', schedule, true);
      window.removeEventListener('resize', schedule);
      document.removeEventListener('visibilitychange', schedule);
      // Leaving the pane (another pane, view or creation): the page hides at once.
      dockLayer.set(false);
      creationsApi.placePreview(label, { rect: null, covered: true }).catch(e => console.error('The preview could not be hidden:', e));
    };
  }, [embedded, label]);

  // While it is built, the page loads again as files change.
  const reloadedAt = useRef(reloadSeq);
  useEffect(() => {
    if (reloadSeq === reloadedAt.current) return;
    reloadedAt.current = reloadSeq;
    if (!label || version !== null) return;
    creationsApi.reloadPreview(label).catch(e => console.error('The preview could not reload:', e));
  }, [reloadSeq, label, version]);

  // Asked for again while popped out: its window comes to the front.
  const focusedAt = useRef(focusSeq);
  useEffect(() => {
    if (focusSeq === focusedAt.current) return;
    focusedAt.current = focusSeq;
    if (poppedOut && label) creationsApi.focusPreview(label).catch(e => setError(errorText(e)));
  }, [focusSeq, poppedOut, label]);

  const reload = useCallback(async () => {
    if (!label || status === 'stopped') {
      // A stopped page is a dead webview: a new one replaces it.
      if (label) {
        leaving.current.add(label);
        await creationsApi.closePreview(label).catch(e => console.error('The stopped preview could not close:', e));
        setLabel(null);
      }
      setEnd(null);
      setAttempt(n => n + 1);
      return;
    }
    try {
      setStatus('opening');
      await creationsApi.reloadPreview(label);
    } catch (e) {
      setError(errorText(e));
    }
  }, [label, status]);

  const popOut = useCallback(async () => {
    if (!label) return;
    leaving.current.add(label);
    try {
      const info = await creationsApi.popOutPreview(label);
      setPoppedOut(true);
      adopt(info);
    } catch (e) {
      leaving.current.delete(label);
      notify.error('Could not pop out', { description: errorText(e) });
    }
  }, [label, adopt]);

  const showHere = useCallback(async () => {
    const current = label;
    if (current) {
      leaving.current.add(current);
      try {
        await creationsApi.closePreview(current);
      } catch (e) {
        console.error('The preview window could not close:', e);
      }
    }
    setLabel(null);
    setEnd(null);
    setPoppedOut(false);
  }, [label]);

  const shownHidden = embedded ? hiddenBy : null;
  const stateLine = previewStateLine({ error, end, load: status, hiddenBy: shownHidden, version, poppedOut });
  // The pane over the page is one this page can close itself (the design check, the code review).
  const closable = shownHidden !== null && review !== null && (review.design !== null || review.subject !== null)
    && review.paneRef.current?.getAttribute('aria-label') === shownHidden;

  return (
    <div className="flex-1 min-h-0 flex flex-col gap-2">
      <div className="flex items-center gap-2 flex-nowrap min-w-0">
        {leading}
        <p
          role="status" aria-live="polite"
          title={error ?? undefined}
          className={cn(
            'm-0 min-w-0 truncate inline-flex items-center gap-1.5 text-small',
            shownHidden ? 'text-shodh-warning' : 'text-shodh-text-secondary',
          )}
        >
          {status === 'opening' && !error && !end && !shownHidden && <Loader2 className="w-3.5 h-3.5 shrink-0 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
          {shownHidden && <EyeOff className="w-3.5 h-3.5 shrink-0" aria-hidden="true" />}
          <span className="truncate">{stateLine}</span>
        </p>
        {closable && (
          <button type="button" onClick={() => review?.close()} className={QUIET_BUTTON}>
            <X className="w-3.5 h-3.5" aria-hidden="true" />
            Close {shownHidden}
          </button>
        )}
        {status === 'blocked' && (
          <span title={PREVIEW_BLOCKED_TEXT} className="shrink-0 inline-flex items-center gap-1 text-caption text-shodh-warning">
            <ShieldAlert className="w-3.5 h-3.5" aria-hidden="true" />
            Blocked
          </span>
        )}
        <div className="ms-auto flex items-center gap-1 shrink-0">
          {networkControl}
          <button type="button" onClick={() => void reload()} title={status === 'stopped' || end ? 'Open again' : 'Reload'} className={QUIET_BUTTON}>
            <RefreshCw className="w-3.5 h-3.5" aria-hidden="true" />
            <span className={COMPACT_LABEL}>{status === 'stopped' || end ? 'Open again' : 'Reload'}</span>
          </button>
          {poppedOut ? (
            <>
              <button type="button" onClick={() => label && void creationsApi.focusPreview(label).catch(e => setError(errorText(e)))} title="Bring to front" className={QUIET_BUTTON}>
                <AppWindow className="w-3.5 h-3.5" aria-hidden="true" />
                <span className={COMPACT_LABEL}>
                  Bring to front
                </span>
              </button>
              <button type="button" onClick={() => void showHere()} title="Show here" className={OUTLINE_BUTTON}>
                <PanelBottomOpen className="w-3.5 h-3.5" aria-hidden="true" />
                <span className={COMPACT_LABEL}>
                  Show here
                </span>
              </button>
            </>
          ) : (
            <button type="button" onClick={() => void popOut()} disabled={!label} title="Open in its own window" className={QUIET_BUTTON}>
              <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" />
              <span className={COMPACT_LABEL}>
                Pop out
              </span>
            </button>
          )}
        </div>
      </div>
      {poppedOut ? null : (
        <div
          ref={boxRef}
          aria-label="Live preview"
          className="flex-1 min-h-0 rounded-xl border border-shodh-border bg-shodh-surface"
        />
      )}
    </div>
  );
}

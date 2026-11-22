/**
 * The creation's page, live inside Artifacts (`creations/preview_embed.rs`). The page runs in a
 * locked child webview of its own; this component renders the empty box it is placed over and
 * reports that box (and whether anything of this page is above it) to Rust once a frame at
 * most. Rust shows the page only while the latest report says it is on screen and uncovered;
 * leaving the pane reports it gone. While it shows, the chat floats over it in a layer of its
 * own (`features/dock`). Pop out moves the page to its own window; Show here brings it back.
 */
import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { AppWindow, Loader2, PanelBottomOpen, RefreshCw, ShieldAlert, ExternalLink } from 'lucide-react';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { useTheme } from '../../contexts/ThemeContext';
import { OVERLAY_SELECTORS, coveredBy, layerVisible } from '../../lib/previewPlacement';
import type { Box } from '../../lib/previewPlacement';
import { PANEL_EDGE, PANEL_HEIGHT, PANEL_WIDTH } from '../../components/shell/DockPanel';
import { dockLayer } from '../dock/layerState';
import { creationsApi, errorText, onPreviewEvent } from './api';
import { PREVIEW_BLOCKED_TEXT, PREVIEW_END_TEXT, previewShowingText } from './model';
import type { PreviewEnd, PreviewEvent } from './model';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';
const QUIET_BUTTON = cn(
  'h-8 px-2.5 inline-flex items-center gap-1.5 rounded-lg text-small text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro disabled:opacity-40',
  FOCUS_RING,
);
const OUTLINE_BUTTON = cn(
  'h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border text-small font-medium text-shodh-text hover:bg-shodh-raised transition-colors duration-micro disabled:opacity-40',
  FOCUS_RING,
);

function boxOf(rect: DOMRect): Box {
  return { x: rect.left, y: rect.top, width: rect.width, height: rect.height };
}

/** Where the floating chat's webview sits: the bottom-right corner, at most the open panel. */
function dockCorner(): Box {
  const width = Math.min(PANEL_WIDTH, window.innerWidth - 2 * PANEL_EDGE);
  const height = Math.min(PANEL_HEIGHT, window.innerHeight - 2 * PANEL_EDGE);
  return { x: window.innerWidth - PANEL_EDGE - width, y: window.innerHeight - PANEL_EDGE - height, width, height };
}

type Status = 'opening' | 'loaded' | 'blocked' | 'stopped';

interface EmbeddedPreviewProps {
  creationId: string;
  version: number | null;
  network: boolean;
  /** Increases when the files changed and the page should load again. */
  reloadSeq: number;
  /** Increases when the page was asked for again (Preview, the assistant): a popped-out window comes to the front. */
  focusSeq: number;
}

export function EmbeddedPreview({ creationId, version, network, reloadSeq, focusSeq }: EmbeddedPreviewProps) {
  const { theme } = useTheme();
  const [label, setLabel] = useState<string | null>(null);
  const [poppedOut, setPoppedOut] = useState(false);
  const [status, setStatus] = useState<Status>('opening');
  const [end, setEnd] = useState<PreviewEnd | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const boxRef = useRef<HTMLDivElement>(null);
  const labelRef = useRef<string | null>(null);
  labelRef.current = label;
  // Labels this page closed or moved itself (their "closed" report needs no note).
  const leaving = useRef(new Set<string>());

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
        setLabel(info.label);
        setEnd(null);
      })
      .catch(e => {
        if (active) setError(errorText(e));
      });
    return () => {
      active = false;
    };
  }, [creationId, version, network, poppedOut, attempt, theme]);

  // Rust's reports on the page.
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    onPreviewEvent((event: PreviewEvent) => {
      if (event.label !== labelRef.current) return;
      if (event.state === 'closed') {
        const ours = leaving.current.delete(event.label);
        setLabel(null);
        if (!ours) setEnd('closed');
        if (event.place === 'window') setPoppedOut(false);
        return;
      }
      if (event.state === 'loaded') setStatus(prev => (prev === 'blocked' ? prev : 'loaded'));
      else if (event.state === 'blocked') setStatus('blocked');
      else setStatus('stopped');
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
      const overlays = Array.from(document.querySelectorAll<HTMLElement>(OVERLAY_SELECTORS.join(',')))
        .filter(element => !box.contains(element))
        .map(element => boxOf(element.getBoundingClientRect()));
      const modalOpen = document.querySelector('[role="dialog"][aria-modal="true"], [role="alertdialog"]') !== null;
      const covered = coveredBy(pane, overlays, modalOpen, dockCorner());
      const visible = layerVisible({
        onArtifacts: true,
        paneShown: true,
        poppedOut: false,
        covered,
        windowVisible: document.visibilityState === 'visible',
      });
      dockLayer.set(visible);
      const placement = { rect: visible ? pane : null, covered: !visible };
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
    window.addEventListener('scroll', schedule, true);
    window.addEventListener('resize', schedule);
    document.addEventListener('visibilitychange', schedule);
    return () => {
      if (frame !== null) window.cancelAnimationFrame(frame);
      resize.disconnect();
      mutations.disconnect();
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
      setLabel(info.label);
      setStatus('opening');
    } catch (e) {
      leaving.current.delete(label);
      notify.error('Could not pop out', { description: errorText(e) });
    }
  }, [label]);

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

  const stateLine = error
    ? 'Could not open'
    : end
      ? PREVIEW_END_TEXT[end]
      : status === 'stopped'
        ? 'Stopped'
        : status === 'opening'
          ? 'Opening'
          : previewShowingText(version, poppedOut ? 'window' : 'embedded');

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2 flex-wrap">
        <p role="status" aria-live="polite" title={error ?? undefined} className="m-0 inline-flex items-center gap-1.5 text-small text-shodh-text-secondary">
          {status === 'opening' && !error && !end && <Loader2 className="w-3.5 h-3.5 animate-spin" aria-hidden="true" />}
          {stateLine}
        </p>
        {status === 'blocked' && (
          <span title={PREVIEW_BLOCKED_TEXT} className="inline-flex items-center gap-1 text-caption text-shodh-warning">
            <ShieldAlert className="w-3.5 h-3.5" aria-hidden="true" />
            Blocked
          </span>
        )}
        <div className="ml-auto flex items-center gap-1">
          <button type="button" onClick={() => void reload()} className={QUIET_BUTTON}>
            <RefreshCw className="w-3.5 h-3.5" aria-hidden="true" />
            {status === 'stopped' || end ? 'Open again' : 'Reload'}
          </button>
          {poppedOut ? (
            <>
              <button type="button" onClick={() => label && void creationsApi.focusPreview(label).catch(e => setError(errorText(e)))} className={QUIET_BUTTON}>
                <AppWindow className="w-3.5 h-3.5" aria-hidden="true" />
                Bring to front
              </button>
              <button type="button" onClick={() => void showHere()} className={OUTLINE_BUTTON}>
                <PanelBottomOpen className="w-3.5 h-3.5" aria-hidden="true" />
                Show here
              </button>
            </>
          ) : (
            <button type="button" onClick={() => void popOut()} disabled={!label} title="Open in its own window" className={QUIET_BUTTON}>
              <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" />
              Pop out
            </button>
          )}
        </div>
      </div>
      {poppedOut ? null : (
        <div
          ref={boxRef}
          aria-label="Live preview"
          className="h-[min(72vh,820px)] min-h-[320px] rounded-xl border border-shodh-border bg-shodh-surface"
        />
      )}
    </div>
  );
}

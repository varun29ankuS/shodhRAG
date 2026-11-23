/**
 * The launch test's hand in the main window (`launch_probe.rs`, run only by the release
 * builds' launch jobs, on a separate profile and by name). Rust sends `launch-probe`, which no
 * webview can send (none may emit events), and this window acts on it through its real code:
 *
 * - `follow`: the assistant's moves of a Creator build, as its run sends them (`navigated`
 *   events): each is parsed like any other and followed the way the session follows one (its
 *   target handed to its view, then the view opened);
 * - `openDesign`: the user clicks the design check's badge, and its findings open.
 *
 * Rust then checks, from this window's own reports, that the page stayed shown.
 */
import { useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { navigationFromEvent, publishTarget } from '../agent/navigation';
import type { NavigatedEvent } from '../agent/events';
import { normalizeViewTab } from '../../lib/viewTabs';
import { useCodeReview } from '../code/codeReviewHost';
import { designApi } from '../code/api';

/** Rust → main only (`launch_probe::LAUNCH_PROBE_EVENT`). */
export const LAUNCH_PROBE_EVENT = 'launch-probe';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** A `navigated` event's fields, as the run sends them, or null. */
function navigatedOf(value: unknown): NavigatedEvent | null {
  if (!isRecord(value) || typeof value.view !== 'string') return null;
  return {
    type: 'navigated',
    runId: 'launch-probe',
    view: value.view,
    focus: typeof value.focus === 'string' ? value.focus : null,
    target: (value.target ?? null) as NavigatedEvent['target'],
    label: typeof value.label === 'string' ? value.label : null,
  };
}

export function LaunchProbeHost(): null {
  const review = useCodeReview();
  const reviewRef = useRef(review);
  reviewRef.current = review;

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<unknown>(LAUNCH_PROBE_EVENT, event => {
      const payload = event.payload;
      if (!isRecord(payload)) return;
      if (payload.kind === 'follow' && Array.isArray(payload.navigations)) {
        for (const raw of payload.navigations) {
          const navigated = navigatedOf(raw);
          if (!navigated) continue;
          const nav = navigationFromEvent(navigated);
          if (nav.target) publishTarget(nav.target);
          const tab = normalizeViewTab(nav.view);
          if (tab) window.dispatchEvent(new CustomEvent('switchTab', { detail: tab }));
        }
      } else if (payload.kind === 'openDesign' && typeof payload.checkId === 'string') {
        designApi
          .get(payload.checkId)
          .then(found => reviewRef.current?.openDesign(found))
          .catch(error => console.error('Launch probe: the design check could not be opened:', error));
      }
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(error => console.error('Launch probe: could not listen:', error));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return null;
}

/**
 * The launch test's hand in the main window (`launch_probe.rs`, run only by the release
 * builds' launch jobs, on a separate profile and by name). Rust sends `launch-probe`, which no
 * webview can send (none may emit events), and this window acts on it through its real code:
 *
 * - `follow`: the assistant's moves of a Creator build, as its run sends them (`navigated`
 *   events): each is parsed like any other and followed the way the session follows one (its
 *   target handed to its view, then the view opened);
 * - `openDesign`: the user clicks the design check's badge, and its findings open;
 * - `show`: the views tour, after every check: one view opened as a click in the sidebar
 *   opens it, in one theme, with the findings pane closed, for the job's picture of it.
 *   A scene (`conversation`) is Ask with the sample conversation the tour wrote open and
 *   the sidebar folded, so the reading column and the question rail have the room a wide
 *   window gives them; the view after it gets back the chat and sidebar it had before;
 * - `measure`: the tour's check of the view now shown: every text on it that does not fit
 *   its box (`textFit.ts`), and how many chat rows the sidebar shows whole (`chatRoom.ts`),
 *   reported with `launch_probe_text_fit`.
 *
 * Rust then checks, from this window's own reports, that the page stayed shown.
 */
import { useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { clippedText } from './textFit';
import { chatRoom } from './chatRoom';
import { rebaseRun } from './runScene';
import { navigationFromEvent, publishTarget } from '../agent/navigation';
import type { NavigatedEvent } from '../agent/events';
import { normalizeViewTab } from '../../lib/viewTabs';
import { useCodeReview } from '../code/codeReviewHost';
import { designApi } from '../code/api';
import { useTheme } from '../../contexts/ThemeContext';
import { useSidebar } from '../../contexts/SidebarContext';
import { useChatSession } from '../ask/ChatSessionContext';
import { fromPersisted, reduceAll, toPersisted } from '../agent/reducer';
import type { TranscriptAction, TranscriptState } from '../agent/reducer';
import { RUN_PANEL_TOGGLE } from '../agent/RunPanel';

/** Rust → main only (`launch_probe::LAUNCH_PROBE_EVENT`). */
export const LAUNCH_PROBE_EVENT = 'launch-probe';

/** The tour's scenes (`launch_probe::TOUR_SCENES`): Ask with a sample conversation open. */
const SCENES = new Set(['conversation', 'run-plan', 'run-progress', 'run-stopped', 'run-summary', 'build-card']);

/** How long the window waits for a scene's conversation to be open before setting its run. */
const SCENE_OPEN_TRIES = 30;
const SCENE_OPEN_INTERVAL_MS = 50;

/**
 * A `run-*` scene's run: the answer it is set on, its transcript's seed and actions, whether
 * its view opens, and whether Shodh closed there (shown as the app restores it).
 */
interface SceneRun {
  answerId: string;
  seed: unknown;
  actions: unknown[];
  open: boolean;
  closed: boolean;
}

function sceneRunOf(value: unknown): SceneRun | null {
  if (!isRecord(value) || typeof value.answerId !== 'string' || !isRecord(value.seed) || !Array.isArray(value.actions)) return null;
  return { answerId: value.answerId, seed: value.seed, actions: value.actions, open: value.open === true, closed: value.closed === true };
}

/** A live answer as the app shows it after Shodh closed during it: saved, then restored. */
function closedDuring(transcript: TranscriptState): TranscriptState {
  return fromPersisted(JSON.parse(JSON.stringify(toPersisted(transcript)))) ?? transcript;
}

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
  const theme = useTheme();
  const themeRef = useRef(theme);
  themeRef.current = theme;
  const sidebar = useSidebar();
  const sidebarRef = useRef(sidebar);
  sidebarRef.current = sidebar;
  const session = useChatSession();
  const sessionRef = useRef(session);
  sessionRef.current = session;
  // What a scene changed, given back to the view after it: the chat open and the sidebar.
  const beforeSceneRef = useRef<{ conversationId: string | null; collapsed: boolean } | null>(null);

  /**
   * A `run-*` scene: once its conversation is open, its answer is set to the run as the
   * reducer builds it from the actions sent (the session's own update of a message), and
   * a finished run's view is opened by its button, as a click opens it.
   */
  const showRun = async (conversationId: string, run: SceneRun) => {
    for (let tries = 0; tries < SCENE_OPEN_TRIES; tries++) {
      const current = sessionRef.current;
      if (current.activeConversationId === conversationId && current.messages.some(m => m.id === run.answerId)) {
        // The recorded run, its clock moved to now, so a live timer reads seconds.
        const timed = rebaseRun(structuredClone(run.seed), run.actions, Date.now());
        const transcript = reduceAll(timed.seed as TranscriptState, timed.actions as TranscriptAction[]);
        if (run.closed) {
          sessionRef.current.updateMessage(run.answerId, { transcript: closedDuring(transcript) });
          return;
        }
        sessionRef.current.updateMessage(run.answerId, { transcript });
        if (run.open) {
          await new Promise(resolve => window.setTimeout(resolve, SCENE_OPEN_INTERVAL_MS * 4));
          document.querySelector<HTMLButtonElement>(`[data-testid="${RUN_PANEL_TOGGLE}"][aria-expanded="false"]`)?.click();
        }
        return;
      }
      await new Promise(resolve => window.setTimeout(resolve, SCENE_OPEN_INTERVAL_MS));
    }
    console.error(`Launch probe: the run's conversation ${conversationId} did not open`);
  };
  const showRunRef = useRef(showRun);
  showRunRef.current = showRun;

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
      } else if (payload.kind === 'show' && typeof payload.view === 'string' && SCENES.has(payload.view) && typeof payload.conversation === 'string') {
        reviewRef.current?.close();
        if ((payload.theme === 'light' || payload.theme === 'dark') && payload.theme !== themeRef.current.theme) {
          themeRef.current.toggleTheme();
        }
        beforeSceneRef.current ??= {
          conversationId: sessionRef.current.activeConversationId,
          collapsed: sidebarRef.current.collapsed,
        };
        sidebarRef.current.setSidebarCollapsed(true);
        const conversationId = payload.conversation;
        sessionRef.current.switchConversation(conversationId);
        window.dispatchEvent(new CustomEvent('switchTab', { detail: 'ask' }));
        const run = sceneRunOf(payload.run);
        if (run) void showRunRef.current(conversationId, run);
      } else if (payload.kind === 'show' && typeof payload.view === 'string') {
        const tab = normalizeViewTab(payload.view);
        if (!tab) return;
        reviewRef.current?.close();
        if ((payload.theme === 'light' || payload.theme === 'dark') && payload.theme !== themeRef.current.theme) {
          themeRef.current.toggleTheme();
        }
        const before = beforeSceneRef.current;
        if (before) {
          beforeSceneRef.current = null;
          sidebarRef.current.setSidebarCollapsed(before.collapsed);
          if (before.conversationId) sessionRef.current.switchConversation(before.conversationId);
        }
        window.dispatchEvent(new CustomEvent('switchTab', { detail: tab }));
      } else if (payload.kind === 'measure' && typeof payload.view === 'string' && typeof payload.theme === 'string') {
        const { count, lines } = clippedText(document);
        invoke('launch_probe_text_fit', {
          view: payload.view,
          theme: payload.theme,
          width: window.innerWidth,
          height: window.innerHeight,
          count,
          clipped: lines,
          chats: chatRoom(document),
        }).catch(error => console.error('Launch probe: the text measure was not delivered:', error));
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

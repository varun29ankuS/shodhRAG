import { lazy } from 'react';
import { ViewPreloader, settledThenable } from '../../lib/viewPreload';
import type { IdleScheduler } from '../../lib/viewPreload';
import type { ViewTab } from '../../lib/viewTabs';

/**
 * The views opened less often load on first use, keeping them out of the
 * startup bundle. Their code is fetched while the app is idle after startup
 * (and on hover or focus of their sidebar item), so a first visit renders in
 * the same commit as the click instead of waiting on the chunk.
 */
type LazyTab = Exclude<ViewTab, 'ask'>;

const loaders = {
  workspaces: () => import('../../features/workspaces/WorkspacesView'),
  library: () => import('../../features/library/LibraryView'),
  connectors: () => import('../../features/connectors/ConnectorsView'),
  tasks: () => import('../../features/tasks/TasksView'),
  activity: () => import('../../features/activity/ActivityView'),
  settings: () => import('./SettingsView'),
} satisfies Record<LazyTab, () => Promise<unknown>>;

type ModuleOf<K extends LazyTab> = Awaited<ReturnType<(typeof loaders)[K]>>;

const preloader = new ViewPreloader<LazyTab>(loaders);

/**
 * `key`'s module for `React.lazy`: synchronously when it was preloaded, so
 * the view never suspends; otherwise once it has loaded (a failure reaches
 * the error boundary, as any lazy import's does).
 */
function viewModule<K extends LazyTab, R>(key: K, pick: (module: ModuleOf<K>) => R): Promise<R> {
  if (preloader.isLoaded(key)) return settledThenable(pick(preloader.loaded(key) as ModuleOf<K>));
  return preloader.load(key).then(module => pick(module as ModuleOf<K>));
}

export const WorkspacesView = lazy(() => viewModule('workspaces', m => m));
export const LibraryView = lazy(() => viewModule('library', m => ({ default: m.LibraryView })));
export const ConnectorsView = lazy(() => viewModule('connectors', m => m));
export const TasksView = lazy(() => viewModule('tasks', m => m));
export const ActivityView = lazy(() => viewModule('activity', m => m));
export const SettingsView = lazy(() => viewModule('settings', m => m));

/**
 * Whether `view` renders without waiting on its code. A view that is not
 * ready is shown through the deferred value in the shell, which keeps the
 * current view on screen until the code has loaded.
 */
export function isViewReady(view: ViewTab): boolean {
  return view === 'ask' || preloader.isLoaded(view);
}

/** Idle order after startup: the sidebar's order. */
const IDLE_ORDER: readonly LazyTab[] = ['workspaces', 'library', 'connectors', 'tasks', 'activity', 'settings'];

/** Longest wait for an idle period before a view's code is fetched anyway. */
const IDLE_TIMEOUT_MS = 4000;
/** Pause between views where the webview has no requestIdleCallback (WKWebView). */
const IDLE_FALLBACK_MS = 250;

function idleScheduler(): IdleScheduler {
  if (typeof window.requestIdleCallback === 'function') {
    return {
      request: callback => window.requestIdleCallback(() => callback(), { timeout: IDLE_TIMEOUT_MS }),
      cancel: handle => window.cancelIdleCallback(handle as number),
    };
  }
  return {
    request: callback => window.setTimeout(callback, IDLE_FALLBACK_MS),
    cancel: handle => window.clearTimeout(handle as number),
  };
}

/** Fetch `view`'s code now (hover or focus of its sidebar item). */
export function preloadView(view: ViewTab): void {
  if (view === 'ask') return;
  void preloader.preload(view);
}

/** Fetch every view's code while the app is idle; returns the function that stops it. */
export function preloadViewsWhenIdle(): () => void {
  return preloader.preloadWhenIdle(IDLE_ORDER, idleScheduler());
}

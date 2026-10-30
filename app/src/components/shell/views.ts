import { lazy } from 'react';
import { PreloadGate, ViewPreloader, settledThenable } from '../../lib/viewPreload';
import type { IdleScheduler } from '../../lib/viewPreload';
import type { ViewTab } from '../../lib/viewTabs';

/**
 * The views opened less often load on first use, keeping them out of the
 * startup bundle. The likely ones (Library, Workspaces) are fetched while the
 * app is idle once the first answer has finished or a while after launch;
 * any view is fetched on hover or focus of its sidebar item, when the command
 * palette opens and when the assistant navigates to it, so a first visit
 * usually renders in the same commit as the click.
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

/** Fetched while idle: the views opened most after Ask. The rest load on intent. */
export const IDLE_PRELOAD_VIEWS: readonly LazyTab[] = ['library', 'workspaces'];

const ALL_LAZY_VIEWS: readonly LazyTab[] = ['workspaces', 'library', 'connectors', 'tasks', 'activity', 'settings'];

/** Pause between views where the webview has no requestIdleCallback (WKWebView). */
const IDLE_FALLBACK_MS = 250;

function idleScheduler(): IdleScheduler {
  if (typeof window.requestIdleCallback === 'function') {
    // No timeout: a busy app (an answer rendering) is never interrupted for this.
    return {
      request: callback => window.requestIdleCallback(() => callback()),
      cancel: handle => window.cancelIdleCallback(handle as number),
    };
  }
  return {
    request: callback => window.setTimeout(callback, IDLE_FALLBACK_MS),
    cancel: handle => window.clearTimeout(handle as number),
  };
}

/** Fetch `view`'s code now (hover or focus of its sidebar item, the assistant navigating to it). */
export function preloadView(view: ViewTab): void {
  if (view === 'ask') return;
  void preloader.preload(view);
}

/** Fetch every view's code now (the command palette opened: any view is one keystroke away). */
export function preloadAllViews(): void {
  for (const view of ALL_LAZY_VIEWS) void preloader.preload(view);
}

/** The gate for fetching the likely views while idle (lib/viewPreload `PreloadGate`). */
export function createIdlePreloadGate(): PreloadGate {
  return new PreloadGate(
    () => preloader.preloadWhenIdle(IDLE_PRELOAD_VIEWS, idleScheduler()),
    { setTimeout: (callback, ms) => window.setTimeout(callback, ms), clearTimeout: handle => window.clearTimeout(handle as number) },
  );
}

/**
 * Startup timing: named `performance` marks for the milestones between the
 * WebView starting and the app being usable, reported once to the console
 * (and visible in the DevTools Performance panel as `shodh:*` marks and
 * measures).
 */

export type StartupMilestone =
  /** Every module of the main bundle has been evaluated. */
  | 'modules-evaluated'
  /** The shell (sidebar, title bar, active view) has committed to the DOM. */
  | 'shell-mounted'
  /** `initialize_rag` returned: the index can be queried. */
  | 'index-ready'
  /** `get_llm_info` answered (configured or not). */
  | 'model-checked'
  /** `get_statistics` answered. */
  | 'stats-loaded'
  /** `load_conversations` answered. */
  | 'conversations-loaded';

const ALL: readonly StartupMilestone[] = [
  'modules-evaluated',
  'shell-mounted',
  'index-ready',
  'model-checked',
  'stats-loaded',
  'conversations-loaded',
];

const PREFIX = 'shodh:';
/** Report whatever has been reached by then, even if a milestone never comes. */
const REPORT_DEADLINE_MS = 30_000;

let reported = false;
let deadline: ReturnType<typeof setTimeout> | null = null;

function hasPerformance(): boolean {
  return typeof performance !== 'undefined' && typeof performance.mark === 'function';
}

function markTime(name: StartupMilestone): number | null {
  const entries = performance.getEntriesByName(PREFIX + name, 'mark');
  return entries.length > 0 ? entries[0].startTime : null;
}

function report() {
  if (reported || !hasPerformance()) return;
  reported = true;
  if (deadline) clearTimeout(deadline);
  const rows: Record<string, string> = {};
  for (const name of ALL) {
    const at = markTime(name);
    if (at === null) {
      rows[name] = 'not reached';
      continue;
    }
    try {
      performance.measure(`${PREFIX}${name}`, { start: 0, end: PREFIX + name });
    } catch {
      // measure() with an options bag is unsupported: the mark is still there.
    }
    rows[name] = `${Math.round(at)} ms`;
  }
  console.info('[shodh] startup (ms since the WebView started loading)', rows);
}

/** Record a startup milestone (first occurrence only). */
export function markStartup(name: StartupMilestone): void {
  if (reported || !hasPerformance()) return;
  try {
    if (markTime(name) === null) performance.mark(PREFIX + name);
  } catch {
    return;
  }
  if (!deadline) deadline = setTimeout(report, REPORT_DEADLINE_MS);
  if (ALL.every(m => markTime(m) !== null)) report();
}

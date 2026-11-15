/**
 * The System status (`indexing_control.rs`): what runs search and how fast,
 * the reranker and scan reader, and the indexing in progress, read from the
 * backend and put in plain words for the status strip and its panel. Every
 * claim is the backend's: GPU or CPU is what loaded, the speed is what the
 * calibration measured, "slower than usual" is what the last batches ran at.
 *
 * Pure (no imports), unit-tested with Node (`app/tests/systemStatus.test.ts`).
 */

/** Emitted by the backend with `{ paused }` when indexing is paused or resumed. */
export const INDEXING_PAUSED_EVENT = 'indexing-paused';

export type Accelerator = 'directml' | 'coreml' | 'cpu';
export type SpeedClass = 'fast' | 'ok' | 'slow';

/** Why the CPU runs search (`compute::provider::CpuReason`). */
export type CpuReason =
  | { kind: 'chosen' }
  | { kind: 'noProvider' }
  | { kind: 'noGpu' }
  | { kind: 'failed'; device: string | null; error: string }
  | { kind: 'slowerThanCpu'; device: string; gpuTokensPerSecond: number; cpuTokensPerSecond: number }
  | { kind: 'cpuModel' }
  | { kind: 'preparing' };

/** `compute::status::EmbedderReport`. */
export interface SearchReport {
  accelerator: Accelerator;
  device: string | null;
  cpuReason: CpuReason | null;
  threads: number | null;
  modelFile: string;
  /** `null`: the speed could not be measured. */
  pagesPerMinute: number | null;
  speed: SpeedClass | null;
  recentPagesPerMinute: number | null;
  degraded: boolean;
  calibratedAt: string | null;
  loaded: boolean;
}

export interface RunningIndex {
  sourceId: string;
  path: string;
}

export type SwitchPhase = 'idle' | 'downloading' | 'building' | 'stopped' | 'ready' | 'failed';

export type SearchModelVariant = 'fp16' | 'int8';

/**
 * The opt-in faster search model (`search_model_switch::SwitchStatus`): fp16
 * is the standard model; int8 is "Faster search (slightly less accurate)".
 */
export interface ModelSwitch {
  /** The library's search model, once recorded. */
  indexVariant: SearchModelVariant | null;
  /** The model a re-index moves to (in progress, or offered). */
  target: SearchModelVariant | null;
  /** A re-index is offered: to the faster model, or back to the standard one. */
  offered: boolean;
  /** The library uses the faster model, which this processor can't run exactly: nothing new is indexed. */
  writesBlocked: boolean;
  phase: SwitchPhase;
  /** The re-index was built, and completing it when Shodh started failed. */
  finishFailed: boolean;
  /** "Restart Shodh now" waits for the last changes to be brought over. */
  restarting: boolean;
  done: number;
  total: number;
  etaSeconds: number | null;
  downloadedBytes: number;
  downloadBytes: number;
  /** The target model's download size when it is not installed (0 when it is). */
  modelBytes: number;
  /** The plain reason of a failure. */
  error: string | null;
  /** Its technical detail, behind "Show details". */
  errorDetail: string | null;
}

/** `indexing_control::SystemStatus`. */
export interface SystemStatus {
  searchInstalled: boolean;
  useGpu: boolean;
  search: SearchReport | null;
  rerankerInstalled: boolean;
  scanReaderInstalled: boolean;
  indexingPaused: boolean;
  running: RunningIndex[];
  /** Code mode code maps reading files now (`code_map_commands::CodeMapRun`). */
  codeMaps: CodeMapRun[];
  /** Recall indexing coding sessions now (`recall_commands::RecallRun`). */
  recall: RecallSyncRun | null;
  modelSwitch: ModelSwitch;
}

/** Recall indexing the user's coding sessions. */
export interface RecallSyncRun {
  sourceId: string;
  done: number;
  total: number;
  /** The project of the session being indexed. */
  current: string | null;
}

/** A code map refresh reading a Code project's files. */
export interface CodeMapRun {
  sourceId: string;
  /** The project folder's name. */
  name: string;
  done: number;
  total: number;
  /** Why it waits (`paused`, `otherIndexing`), when it does. */
  waiting: string | null;
}

/** The colour a state is shown in. */
export type Tone = 'good' | 'warn' | 'bad' | 'idle';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function text(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null;
}

function count(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null;
}

function oneOf<T extends string>(values: readonly T[], value: unknown): T | null {
  return typeof value === 'string' && (values as readonly string[]).includes(value) ? (value as T) : null;
}

export function readCpuReason(value: unknown): CpuReason | null {
  if (!isRecord(value)) return null;
  switch (value.kind) {
    case 'chosen':
    case 'noProvider':
    case 'noGpu':
    case 'cpuModel':
    case 'preparing':
      return { kind: value.kind };
    case 'failed':
      return { kind: 'failed', device: text(value.device), error: text(value.error) ?? 'unknown error' };
    case 'slowerThanCpu':
      return {
        kind: 'slowerThanCpu',
        device: text(value.device) ?? 'The graphics card',
        gpuTokensPerSecond: count(value.gpuTokensPerSecond) ?? 0,
        cpuTokensPerSecond: count(value.cpuTokensPerSecond) ?? 0,
      };
    default:
      return null;
  }
}

export function readSearchReport(value: unknown): SearchReport | null {
  if (!isRecord(value)) return null;
  const accelerator = oneOf(['directml', 'coreml', 'cpu'] as const, value.accelerator);
  const speed = oneOf(['fast', 'ok', 'slow'] as const, value.speed);
  const pagesPerMinute = count(value.pagesPerMinute);
  if (!accelerator) return null;
  return {
    accelerator,
    device: text(value.device),
    cpuReason: readCpuReason(value.cpuReason),
    threads: count(value.threads),
    modelFile: text(value.modelFile) ?? '',
    pagesPerMinute,
    speed,
    recentPagesPerMinute: count(value.recentPagesPerMinute),
    degraded: value.degraded === true,
    calibratedAt: text(value.calibratedAt),
    loaded: value.loaded === true,
  };
}

export function readSystemStatus(value: unknown): SystemStatus | null {
  if (!isRecord(value)) return null;
  const running = Array.isArray(value.running)
    ? value.running.flatMap(r => {
        if (!isRecord(r)) return [];
        const sourceId = text(r.sourceId);
        const path = text(r.path);
        return sourceId && path ? [{ sourceId, path }] : [];
      })
    : [];
  const codeMaps = Array.isArray(value.codeMaps)
    ? value.codeMaps.flatMap(r => {
        if (!isRecord(r)) return [];
        const sourceId = text(r.sourceId);
        const name = text(r.name);
        return sourceId && name
          ? [{ sourceId, name, done: count(r.done) ?? 0, total: count(r.total) ?? 0, waiting: text(r.waiting) }]
          : [];
      })
    : [];
  return {
    searchInstalled: value.searchInstalled === true,
    useGpu: value.useGpu !== false,
    search: readSearchReport(value.search),
    rerankerInstalled: value.rerankerInstalled === true,
    scanReaderInstalled: value.scanReaderInstalled === true,
    indexingPaused: value.indexingPaused === true,
    running,
    codeMaps,
    recall: readRecallRun(value.recall),
    modelSwitch: readModelSwitch(value.modelSwitch),
  };
}

const NO_SWITCH: ModelSwitch = {
  indexVariant: null,
  target: null,
  offered: false,
  writesBlocked: false,
  phase: 'idle',
  finishFailed: false,
  restarting: false,
  done: 0,
  total: 0,
  etaSeconds: null,
  downloadedBytes: 0,
  downloadBytes: 0,
  modelBytes: 0,
  error: null,
  errorDetail: null,
};

export function readModelSwitch(value: unknown): ModelSwitch {
  if (!isRecord(value)) return NO_SWITCH;
  return {
    indexVariant: oneOf(['fp16', 'int8'] as const, value.indexVariant),
    target: oneOf(['fp16', 'int8'] as const, value.target),
    offered: value.offered === true,
    writesBlocked: value.writesBlocked === true,
    phase: oneOf(['idle', 'downloading', 'building', 'stopped', 'ready', 'failed'] as const, value.phase) ?? 'idle',
    finishFailed: value.finishFailed === true,
    restarting: value.restarting === true,
    done: count(value.done) ?? 0,
    total: count(value.total) ?? 0,
    etaSeconds: count(value.etaSeconds),
    downloadedBytes: count(value.downloadedBytes) ?? 0,
    downloadBytes: count(value.downloadBytes) ?? 0,
    modelBytes: count(value.modelBytes) ?? 0,
    error: text(value.error),
    errorDetail: text(value.errorDetail),
  };
}

/** "279 MB" (decimal megabytes, as downloads are usually shown). */
export function megabytesText(bytes: number): string {
  return `${Math.max(1, Math.round(bytes / 1_000_000)).toLocaleString('en-US')} MB`;
}

/** The faster model's name wherever it is offered. */
export const FASTER_SEARCH_NAME = 'Faster search (slightly less accurate)';

/**
 * What the user can do about the switch. `stop` keeps the work for Continue;
 * `discard` throws it away (the panel asks first); `retryRestart` restarts
 * Shodh to complete a switch again.
 */
export type SwitchAction = 'start' | 'continue' | 'stop' | 'discard' | 'restart' | 'retry' | 'retryRestart';

/** The switch in plain words for the Search part of the panel; `null` when there is nothing to show. */
export interface SwitchView {
  title: string;
  detail: string | null;
  /** The technical reason behind a failure, for "Show details". */
  technical: string | null;
  actions: { action: SwitchAction; label: string }[];
  tone: Tone;
}

/** What a discard would throw away, for its confirmation. */
export function discardConfirmText(status: SystemStatus | null): string {
  const s = status?.modelSwitch;
  const work =
    s && s.done > 0
      ? `the ${s.done.toLocaleString('en-US')} ${s.done === 1 ? 'passage' : 'passages'} already re-indexed`
      : 'the re-index';
  return `Discard ${work}? Starting again re-indexes the library from the beginning.`;
}

export function modelSwitchView(status: SystemStatus | null): SwitchView | null {
  const s = status?.modelSwitch;
  if (!s) return null;
  const toFaster = s.target === 'int8';
  const reindexing = toFaster ? 'Re-indexing for faster search' : 'Re-indexing for standard search';
  const modelName = toFaster ? 'faster search model' : 'standard search model';
  const passages = (n: number) => `${n.toLocaleString('en-US')} ${n === 1 ? 'passage' : 'passages'}`;
  const counted = s.total > 0 ? `${s.done.toLocaleString('en-US')} of ${passages(s.total)}` : null;
  const download = s.modelBytes > 0 ? `It downloads the model first (${megabytesText(s.modelBytes)}).` : null;
  const how =
    'Shodh re-indexes your library’s saved text once, in the background; search keeps working meanwhile, and the switch happens the next time Shodh starts.';
  const busy = [
    { action: 'stop' as const, label: 'Stop' },
    { action: 'discard' as const, label: 'Discard' },
  ];
  switch (s.phase) {
    case 'idle':
      if (!s.offered || !s.target) return null;
      if (toFaster) {
        return {
          title: `${FASTER_SEARCH_NAME} is available for this computer.`,
          detail: [
            'It indexes faster on this processor, which runs the faster model’s arithmetic quickly, but may rank some passages lower.',
            how,
            download,
          ]
            .filter(Boolean)
            .join(' '),
          technical: null,
          actions: [{ action: 'start', label: 'Re-index for faster search' }],
          tone: 'idle',
        };
      }
      return {
        title: s.writesBlocked
          ? 'Your library uses the faster search model, which this processor can’t run exactly.'
          : `Your library uses ${FASTER_SEARCH_NAME.toLowerCase()}.`,
        detail: [
          s.writesBlocked
            ? 'Nothing new is indexed until it moves to the standard search model; search keeps working.'
            : 'The standard search model ranks passages more accurately; indexing is slower on this processor.',
          how,
          download,
        ]
          .filter(Boolean)
          .join(' '),
        technical: null,
        actions: [{ action: 'start', label: 'Re-index for standard search' }],
        tone: s.writesBlocked ? 'bad' : 'idle',
      };
    case 'downloading':
      return {
        title: `Downloading the ${modelName}`,
        detail:
          s.downloadBytes > 0
            ? `${megabytesText(s.downloadedBytes)} of ${megabytesText(s.downloadBytes)}`
            : 'Checking the model',
        technical: null,
        actions: busy,
        tone: 'good',
      };
    case 'building': {
      if (s.restarting) {
        return {
          title: 'Getting the switch ready, then restarting Shodh',
          detail: `Bringing over what changed since the re-index${counted ? ` (${counted})` : ''}.`,
          technical: null,
          actions: [],
          tone: 'good',
        };
      }
      const paused = status?.indexingPaused ?? false;
      const left = paused ? null : etaText(s.etaSeconds);
      return {
        title: paused ? `${reindexing} (paused)` : reindexing,
        detail: [counted ?? 'Starting', left].filter(Boolean).join(' · ') + '. Search keeps working meanwhile.',
        technical: null,
        actions: busy,
        tone: paused ? 'warn' : 'good',
      };
    }
    case 'stopped':
      return {
        title: `${reindexing} stopped before it finished.`,
        detail: counted ? `It got to ${counted}; Continue picks up from there.` : 'Continue picks up where it stopped.',
        technical: null,
        actions: [
          { action: 'continue', label: 'Continue' },
          { action: 'discard', label: 'Discard' },
        ],
        tone: 'warn',
      };
    case 'ready':
      return {
        title: `The ${modelName} is ready.`,
        detail:
          'Restart Shodh to switch to it. Shodh first brings over what changed since the re-index, saves your work, then restarts.',
        technical: null,
        actions: [
          { action: 'restart', label: 'Restart Shodh now' },
          { action: 'discard', label: 'Discard' },
        ],
        tone: 'good',
      };
    case 'failed':
      return s.finishFailed
        ? {
            title: `Switching to the ${modelName} did not finish.`,
            detail:
              s.error ?? 'Shodh could not finish the switch when it started, so your library kept its current model.',
            technical: s.errorDetail,
            actions: [
              { action: 'retryRestart', label: 'Try again (restarts Shodh)' },
              { action: 'discard', label: 'Cancel the switch' },
            ],
            tone: 'bad',
          }
        : {
            title: `${reindexing} did not finish.`,
            detail: s.error ? `Reason: ${s.error}` : null,
            technical: null,
            actions: [
              { action: 'retry', label: 'Try again' },
              { action: 'discard', label: 'Discard' },
            ],
            tone: 'bad',
          };
  }
}

/** "≈1,200 pages a minute", rounded to two significant figures. */
export function pagesPerMinuteText(pages: number): string {
  if (pages < 1) return 'under 1 page a minute';
  const magnitude = 10 ** Math.max(0, Math.floor(Math.log10(pages)) - 1);
  const rounded = Math.round(pages / magnitude) * magnitude;
  return `≈${rounded.toLocaleString('en-US')} ${rounded === 1 ? 'page' : 'pages'} a minute`;
}

const SPEED_WORD: Record<SpeedClass, string> = { fast: 'fast', ok: 'moderate', slow: 'slow' };

/** Where search runs, in plain words. */
export function runsOnText(report: SearchReport): string {
  switch (report.accelerator) {
    case 'directml':
      return report.device ? `the graphics card (${report.device})` : 'the graphics card';
    case 'coreml':
      return 'the Apple GPU and Neural Engine';
    case 'cpu':
      return 'the processor (CPU)';
  }
}

/** Why the CPU runs search, in plain words (the technical error is in `cpuReasonDetail`). */
export function cpuReasonText(reason: CpuReason | null): string {
  if (!reason) return 'No reason was reported.';
  switch (reason.kind) {
    case 'chosen':
      return 'You turned off "Use the graphics card for search".';
    case 'noProvider':
      return 'Shodh has no graphics card support on this system.';
    case 'noGpu':
      return 'No graphics card that Shodh can use was found.';
    case 'failed':
      return reason.device
        ? `${reason.device} could not run the search model.`
        : 'The graphics cards could not be checked.';
    case 'slowerThanCpu':
      return `${reason.device} ran the search model slower than the processor.`;
    case 'cpuModel':
      return 'Your library was indexed with the search model made for the processor, which runs only there.';
    case 'preparing':
      return 'The graphics card is being prepared; the processor runs search until it is ready.';
  }
}

/** The technical error behind a failed graphics card, for the panel's details. */
export function cpuReasonDetail(reason: CpuReason | null): string | null {
  return reason?.kind === 'failed' ? reason.error : null;
}

/** What the user can do about search running on the CPU. */
export function cpuAdvice(reason: CpuReason | null): string {
  switch (reason?.kind) {
    case 'chosen':
      return 'Turn on "Use the graphics card for search" to try it again.';
    case 'failed':
      return 'Update the graphics driver; Shodh tries the card again after a driver or app update.';
    case 'preparing':
      return 'Nothing to do: search moves to the graphics card when it is ready. The first time takes a few minutes.';
    case 'noGpu':
    case 'noProvider':
    case 'slowerThanCpu':
    case 'cpuModel':
      return 'Index fewer folders at a time, or let large folders index while you are away.';
    default:
      return 'Restart Shodh to check again.';
  }
}

/** The search line of the status strip and its colour. */
export interface SearchSummary {
  /** Short text for the strip ("GPU · fast"). */
  short: string;
  /** One sentence for the panel and screen readers. */
  sentence: string;
  tone: Tone;
}

/**
 * "Slower than usual" is shown only while the model is in memory and
 * indexing is not paused: an idle or unloaded model is not running slowly.
 */
function showsDegraded(status: SystemStatus, report: SearchReport): boolean {
  return report.degraded && report.loaded && !status.indexingPaused;
}

export function searchSummary(status: SystemStatus | null): SearchSummary {
  if (!status) return { short: 'Checking', sentence: 'Checking what runs search.', tone: 'idle' };
  if (!status.searchInstalled) {
    return { short: 'Not set up', sentence: 'Search is not set up yet.', tone: 'warn' };
  }
  const report = status.search;
  if (!report) {
    return {
      short: 'Not measured yet',
      sentence: 'Search has not run yet; its speed is measured the first time it does.',
      tone: 'idle',
    };
  }
  const where = report.accelerator === 'cpu' ? 'CPU' : 'GPU';
  if (report.pagesPerMinute === null || report.speed === null) {
    return {
      short: `${where} · speed not measured`,
      sentence: `Search runs on ${runsOnText(report)}; its speed could not be measured.`,
      tone: report.accelerator === 'cpu' ? 'warn' : 'good',
    };
  }
  if (showsDegraded(status, report)) {
    const recent = report.recentPagesPerMinute ?? report.pagesPerMinute;
    return {
      short: `${where} · slower than usual`,
      sentence: `The search model runs on ${runsOnText(report)} but slower than usual: ${pagesPerMinuteText(recent)} instead of ${pagesPerMinuteText(report.pagesPerMinute)}. Other programs may be busy, or the computer may be on battery or too warm.`,
      tone: 'warn',
    };
  }
  return {
    short: `${where} · ${SPEED_WORD[report.speed]}`,
    sentence: `The search model runs on ${runsOnText(report)}, ${SPEED_WORD[report.speed]}: ${pagesPerMinuteText(report.pagesPerMinute)}.`,
    tone: report.speed === 'slow' ? 'bad' : report.accelerator === 'cpu' || report.speed === 'ok' ? 'warn' : 'good',
  };
}

/**
 * The notice shown where indexing happens while search runs on the CPU:
 * what it means, why, and what helps. `null` on a GPU or before the first
 * load. The speed is the search model's; reading scans with OCR is slower
 * still.
 */
export interface CpuNotice {
  title: string;
  reason: string;
  advice: string;
  tone: Tone;
}

export function cpuNotice(status: SystemStatus | null): CpuNotice | null {
  const report = status?.search;
  if (!report || report.accelerator !== 'cpu') return null;
  const speed =
    report.pagesPerMinute === null ? 'speed not measured' : `search model speed ${pagesPerMinuteText(report.pagesPerMinute)}`;
  return {
    title:
      report.speed === 'slow'
        ? `Search is using the CPU, so indexing will be slow (${speed}).`
        : `Search is using the CPU (${speed}).`,
    reason: `Reason: ${cpuReasonText(report.cpuReason)}`,
    advice: cpuAdvice(report.cpuReason),
    tone: report.speed === 'slow' ? 'bad' : 'warn',
  };
}

/** The indexing line of the status strip: `items` is the folders and files being indexed. */
export function indexingText(status: SystemStatus | null, items: number): string {
  if (!status) return '';
  if (status.indexingPaused) return items > 0 ? 'Indexing paused' : 'Indexing paused (nothing waiting)';
  if (items === 0) return 'Nothing indexing';
  return items === 1 ? 'Indexing 1 item' : `Indexing ${items} items`;
}

/** "120 of 900 files" for a code map refresh, "Listing files" before it knows. */
export function codeMapRunText(run: CodeMapRun): string {
  const files = run.total === 0
    ? 'Listing files'
    : `${Math.min(run.done, run.total).toLocaleString()} of ${run.total.toLocaleString()} ${run.total === 1 ? 'file' : 'files'}`;
  if (run.waiting === 'otherIndexing') return `${files} · waiting for other indexing`;
  if (run.waiting === 'paused') return `${files} · paused`;
  return files;
}

/** "about 3 min left" from seconds, or null when unknown. */
export function etaText(seconds: number | null | undefined): string | null {
  if (!seconds || !Number.isFinite(seconds) || seconds <= 0) return null;
  if (seconds < 60) return 'under a minute left';
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `about ${minutes} min left`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `about ${hours} h left` : `about ${hours} h ${rest} min left`;
}

function readRecallRun(value: unknown): RecallSyncRun | null {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return null;
  const run = value as Record<string, unknown>;
  const sourceId = typeof run.sourceId === 'string' && run.sourceId.length > 0 ? run.sourceId : null;
  if (!sourceId) return null;
  const n = (v: unknown) => (typeof v === 'number' && Number.isFinite(v) && v >= 0 ? Math.floor(v) : 0);
  return {
    sourceId,
    done: n(run.done),
    total: n(run.total),
    current: typeof run.current === 'string' && run.current.length > 0 ? run.current : null,
  };
}

/** "3 of 40 sessions (lumen-cache)" for Recall, "Looking for new sessions" before it knows. */
export function recallRunText(run: RecallSyncRun): string {
  if (run.total === 0) return 'Looking for new sessions';
  const where = run.current ? ` (${run.current})` : '';
  return `${Math.min(run.done, run.total).toLocaleString()} of ${run.total.toLocaleString()} ${run.total === 1 ? 'session' : 'sessions'}${where}`;
}

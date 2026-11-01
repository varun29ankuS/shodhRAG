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
  | { kind: 'slowerThanCpu'; device: string; gpuTokensPerSecond: number; cpuTokensPerSecond: number };

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

/** `indexing_control::SystemStatus`. */
export interface SystemStatus {
  searchInstalled: boolean;
  useGpu: boolean;
  search: SearchReport | null;
  rerankerInstalled: boolean;
  scanReaderInstalled: boolean;
  indexingPaused: boolean;
  running: RunningIndex[];
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
  return {
    searchInstalled: value.searchInstalled === true,
    useGpu: value.useGpu !== false,
    search: readSearchReport(value.search),
    rerankerInstalled: value.rerankerInstalled === true,
    scanReaderInstalled: value.scanReaderInstalled === true,
    indexingPaused: value.indexingPaused === true,
    running,
  };
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
    case 'noGpu':
    case 'noProvider':
    case 'slowerThanCpu':
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

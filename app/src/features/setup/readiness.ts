/**
 * "Getting Shodh ready": one view of the two one-time downloads a new install
 * needs before it can answer, the search models (find passages in the user's
 * files) and the answer engine (the pinned agent runtime that writes answers),
 * and whether a question sent now has to wait for them.
 *
 * Pure (type-only imports, runtime imports with their extension) so it is
 * unit-tested directly with Node (`app/tests/readiness.test.ts`).
 */
import type { InstallProgress, SearchModelsStatus } from './searchModels';
import { formatMegabytes, installFraction, summarizeSetup } from './searchModels.ts';

/** Where each part is downloaded from, as setup names it. */
export const SEARCH_MODELS_SOURCE = 'Hugging Face';
export const DEFAULT_RUNTIME_SOURCE = 'GitHub';

export type PartId = 'search' | 'runtime';

/** One part of setup as the card shows it. */
export type PartState =
  /** Its status has not been read yet, or could not be read: it never holds a question. */
  | 'unknown'
  | 'ready'
  /** Not on this computer and not downloading. */
  | 'needed'
  | 'installing'
  /** The last download failed (`offline`: it could not connect). */
  | 'failed';

export interface PartView {
  id: PartId;
  /** "Search models", "Answer engine". */
  label: string;
  state: PartState;
  /** 0–1 while installing, once the size is known. */
  fraction: number | null;
  /** One plain line: what is happening to this part. */
  detail: string;
  /** Bytes it still downloads (0 when ready). */
  remainingBytes: number;
  /** Why it failed, in plain words (only for `failed`). */
  error: string | null;
  offline: boolean;
}

/** What the search side of setup knows (the search model provider's state). */
export interface SearchInput {
  status: SearchModelsStatus | null;
  statusError: string | null;
  installing: boolean;
  progress: InstallProgress | null;
  installError: string | null;
  installOffline: boolean;
}

/** `agent_runtime_status`, as setup reads it. */
export interface RuntimeStatusInput {
  installed: boolean;
  /** Size of the pinned download on this platform, when it is pinned. */
  downloadBytes?: number | null;
  source?: string;
}

export interface RuntimeProgressInput {
  downloaded: number;
  total: number | null;
}

/** What the answer engine side of setup knows (the runtime provider's state). */
export interface RuntimeInput {
  status: RuntimeStatusInput | null;
  statusError: string | null;
  installing: boolean;
  progress: RuntimeProgressInput | null;
  installError: string | null;
  installOffline: boolean;
}

/** The whole of setup. */
export type ReadinessPhase =
  /** A status is still being read and nothing is known to be missing. */
  | 'checking'
  /** Something is missing and nothing downloads (setup was never started, or was left). */
  | 'needed'
  | 'installing'
  /** A download failed and nothing else downloads: Try again. */
  | 'failed'
  | 'ready';

export interface Readiness {
  phase: ReadinessPhase;
  parts: PartView[];
  /** Every failed part could not connect. */
  offline: boolean;
  /** Bytes still to download over every part. */
  remainingBytes: number;
}

function searchPart(input: SearchInput): PartView {
  const base = { id: 'search' as const, label: 'Search models', fraction: null, error: null, offline: false };
  const { status } = input;
  if (input.installing) {
    const fraction = installFraction(input.progress);
    const detail = input.progress && input.progress.overallTotal > 0
      ? `Downloading · ${formatMegabytes(input.progress.overallBytes)} of ${formatMegabytes(input.progress.overallTotal)}`
      : 'Starting the download…';
    const remaining = input.progress
      ? Math.max(0, input.progress.overallTotal - input.progress.overallBytes)
      : status ? summarizeSetup(status).remainingBytes : 0;
    return { ...base, state: 'installing', fraction, detail, remainingBytes: remaining };
  }
  if (!status) return { ...base, state: 'unknown', detail: input.statusError ? 'Could not be checked' : 'Checking…', remainingBytes: 0 };
  if (status.ready) {
    // "Verified" only when every file's checksum was checked; a quick status check reads sizes.
    const hashed = status.artifacts.length > 0 && status.artifacts.every(a => a.state === 'verified');
    return { ...base, state: 'ready', detail: hashed ? 'Downloaded and verified' : 'Installed', remainingBytes: 0 };
  }
  const summary = summarizeSetup(status);
  if (input.installError) {
    return {
      ...base,
      state: 'failed',
      detail: input.installOffline ? 'Could not connect' : 'Download stopped',
      error: input.installError,
      offline: input.installOffline,
      remainingBytes: summary.remainingBytes,
    };
  }
  const detail = summary.resumable
    ? `Paused · ${formatMegabytes(summary.remainingBytes)} left`
    : `${approxMegabytes(summary.remainingBytes)} to download`;
  return { ...base, state: 'needed', detail, remainingBytes: summary.remainingBytes };
}

function runtimePart(input: RuntimeInput): PartView {
  const base = { id: 'runtime' as const, label: 'Answer engine', fraction: null, error: null, offline: false };
  const size = input.status?.downloadBytes ?? null;
  if (input.installing) {
    const total = input.progress?.total ?? size;
    const downloaded = input.progress?.downloaded ?? 0;
    const fraction = input.progress && total && total > 0 ? Math.min(1, Math.max(0, downloaded / total)) : null;
    const detail = !input.progress
      ? 'Starting the download…'
      : total
        ? `Downloading · ${formatMegabytes(downloaded)} of ${formatMegabytes(total)}`
        : `Downloading · ${formatMegabytes(downloaded)}`;
    return { ...base, state: 'installing', fraction, detail, remainingBytes: total ? Math.max(0, total - downloaded) : 0 };
  }
  if (!input.status) {
    return { ...base, state: 'unknown', detail: input.statusError ? 'Could not be checked' : 'Checking…', remainingBytes: 0 };
  }
  // The status only sees the file; its checksum is checked before every launch.
  if (input.status.installed) return { ...base, state: 'ready', detail: 'Installed (checked before each launch)', remainingBytes: 0 };
  if (input.installError) {
    return {
      ...base,
      state: 'failed',
      detail: input.installOffline ? 'Could not connect' : 'Download stopped',
      error: input.installError,
      offline: input.installOffline,
      remainingBytes: size ?? 0,
    };
  }
  return { ...base, state: 'needed', detail: size ? `${approxMegabytes(size)} to download` : 'To download', remainingBytes: size ?? 0 };
}

/** "≈600 MB" (decimal megabytes, rounded to the nearest 5 above 50). */
export function approxMegabytes(bytes: number): string {
  const mb = bytes / 1_000_000;
  if (mb < 1) return '<1 MB';
  if (mb < 50) return `≈${Math.round(mb)} MB`;
  return `≈${Math.round(mb / 5) * 5} MB`;
}

export function readiness(search: SearchInput, runtime: RuntimeInput): Readiness {
  const parts = [searchPart(search), runtimePart(runtime)];
  const states = parts.map(p => p.state);
  const failed = parts.filter(p => p.state === 'failed');
  let phase: ReadinessPhase;
  if (states.every(s => s === 'ready')) phase = 'ready';
  else if (states.includes('installing')) phase = 'installing';
  else if (failed.length > 0) phase = 'failed';
  else if (states.includes('needed')) phase = 'needed';
  else phase = 'checking';
  return {
    phase,
    parts,
    offline: failed.length > 0 && failed.every(p => p.offline),
    remainingBytes: parts.reduce((n, p) => n + p.remainingBytes, 0),
  };
}

/** What happens to a question sent now: it goes, or it waits for setup. */
export type QuestionGate = 'send' | 'wait';

/**
 * A question waits only while the answer engine, which writes every answer, is
 * missing, downloading or failed. The search models do not hold it: a Code-mode
 * question, a library with nothing indexed, or an offline download must not block
 * asking, and a search before they are in place tells the model to say plainly that
 * search is still being set up. A status not known yet (or unreadable) never holds a
 * question: the send then reports any real problem itself.
 */
export function questionGate(state: Readiness): QuestionGate {
  const engine = state.parts.find(p => p.id === 'runtime');
  return engine && (engine.state === 'needed' || engine.state === 'installing' || engine.state === 'failed') ? 'wait' : 'send';
}

/** The parts setup still has to download (missing or failed), for one Set up / Try again. */
export function partsToInstall(state: Readiness): PartId[] {
  return state.parts.filter(p => p.state === 'needed' || p.state === 'failed').map(p => p.id);
}

/**
 * What setup downloads, from where and how much, in plain words, before it
 * starts. No checksums: what verification means is said instead.
 */
export function setupDisclosure(state: Readiness, runtimeSource = DEFAULT_RUNTIME_SOURCE): string {
  const pieces: string[] = [];
  for (const part of state.parts) {
    if (part.state === 'ready' || part.state === 'unknown') continue;
    const size = part.remainingBytes > 0 ? ` (${approxMegabytes(part.remainingBytes)})` : '';
    pieces.push(part.id === 'search'
      ? `the search models${size} from ${SEARCH_MODELS_SOURCE}, which find passages in your files`
      : `the answer engine${size} from ${runtimeSource}, which writes the answers`);
  }
  if (pieces.length === 0) return 'Everything Shodh needs is on this computer.';
  const what = pieces.length === 1 ? pieces[0] : `${pieces[0]}, and ${pieces[1]}`;
  const total = state.remainingBytes > 0 && pieces.length > 1 ? ` About ${approxMegabytes(state.remainingBytes).replace('≈', '')} in all.` : '';
  const runs = pieces.length === 1 ? 'it runs' : 'both run';
  return `Shodh downloads ${what}. This happens once; ${runs} on this computer, and each file is checked before it is used.${total}`;
}

/** The heading of the setup card. */
export function setupHeading(state: Readiness): string {
  switch (state.phase) {
    case 'ready':
      return 'Shodh is ready';
    case 'installing':
      return 'Getting Shodh ready';
    case 'failed':
      return state.offline ? "Couldn't download what Shodh needs" : 'Setup stopped';
    case 'needed':
    case 'checking':
      return 'Get Shodh ready (one time)';
  }
}

/** One line for a question that waits: "Preparing search (downloading models, 1 of 2)…". */
export function waitingLine(state: Readiness): string {
  const pending = state.parts.filter(p => p.state !== 'ready' && p.state !== 'unknown');
  const total = state.parts.length;
  const done = total - pending.length;
  switch (state.phase) {
    case 'installing': {
      const active = pending.find(p => p.state === 'installing') ?? pending[0];
      const percent = active?.fraction != null ? ` ${Math.round(active.fraction * 100)}%` : '';
      const what = active ? `${active.label.toLowerCase()}${percent}` : 'downloading';
      return `Getting Shodh ready: ${what} (${done} of ${total} ready). Your question is sent when it finishes.`;
    }
    case 'failed':
      return 'Your question is sent once setup finishes. Try again to resume the download.';
    case 'needed':
      return 'Your question is sent once Shodh has downloaded what it needs. Set it up to start.';
    default:
      return 'Your question is sent when Shodh is ready.';
  }
}

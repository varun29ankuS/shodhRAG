/**
 * Search model setup: wire types for `search_models_status` /
 * `install_search_models` and pure helpers for the setup card. No React and
 * type-only imports, so it is unit-tested directly with Node
 * (`app/tests/searchModels.test.ts`).
 */

export const SEARCH_MODELS_PROGRESS_EVENT = 'search-models-progress';
export const SEARCH_MODELS_READY_EVENT = 'search-models-ready';

/** Error code at the start of a failure caused by missing models. */
export const SEARCH_MODELS_MISSING_CODE = 'search_models_missing';
/** Error code when another install is already running. */
export const INSTALL_IN_PROGRESS_CODE = 'install_in_progress';

export type ArtifactState = 'missing' | 'partial' | 'unverified' | 'verified' | 'corrupt';

export interface ArtifactStatus {
  name: string;
  relativePath: string;
  size: number;
  sha256: string;
  state: ArtifactState;
  presentBytes: number;
}

export interface SearchModelsStatus {
  ready: boolean;
  installing: boolean;
  modelDir: string;
  totalBytes: number;
  artifacts: ArtifactStatus[];
}

export type InstallPhase = 'checking' | 'downloading' | 'verifying' | 'verified';

export interface InstallProgress {
  artifact: string;
  phase: InstallPhase;
  artifactBytes: number;
  artifactTotal: number;
  overallBytes: number;
  overallTotal: number;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

export function isInstallProgress(value: unknown): value is InstallProgress {
  return (
    isRecord(value) &&
    typeof value.artifact === 'string' &&
    typeof value.phase === 'string' &&
    typeof value.overallBytes === 'number' &&
    typeof value.overallTotal === 'number'
  );
}

export function isSearchModelsStatus(value: unknown): value is SearchModelsStatus {
  return isRecord(value) && typeof value.ready === 'boolean' && Array.isArray(value.artifacts);
}

/** What the setup card should offer for a status. */
export interface SetupSummary {
  /** Search cannot run until the models are installed. */
  needed: boolean;
  /** A previous download was interrupted and will resume. */
  resumable: boolean;
  /** Some file on disk failed verification and will be replaced. */
  hasCorrupt: boolean;
  /** Bytes still to download (approximate for partial files). */
  remainingBytes: number;
  totalBytes: number;
}

export function summarizeSetup(status: SearchModelsStatus): SetupSummary {
  let remaining = 0;
  let resumable = false;
  let hasCorrupt = false;
  for (const a of status.artifacts) {
    if (a.state === 'verified') continue;
    if (a.state === 'partial') {
      resumable = true;
      remaining += Math.max(0, a.size - a.presentBytes);
    } else {
      if (a.state === 'corrupt') hasCorrupt = true;
      remaining += a.size;
    }
  }
  return {
    needed: !status.ready,
    resumable,
    hasCorrupt,
    remainingBytes: remaining,
    totalBytes: status.totalBytes,
  };
}

/** Fraction (0–1) of the whole install, or null before the first event. */
export function installFraction(progress: InstallProgress | null): number | null {
  if (!progress || progress.overallTotal <= 0) return null;
  return Math.min(1, Math.max(0, progress.overallBytes / progress.overallTotal));
}

/** "555.0 MB" style size. */
export function formatMegabytes(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Status line for a running install. */
export function describeProgress(progress: InstallProgress | null): string {
  if (!progress) return 'Connecting…';
  switch (progress.phase) {
    case 'checking':
      return `Checking ${progress.artifact}…`;
    case 'downloading':
      return `Downloading ${progress.artifact} · ${formatMegabytes(progress.overallBytes)} of ${formatMegabytes(progress.overallTotal)}`;
    case 'verifying':
      return `Verifying ${progress.artifact} (SHA-256)…`;
    case 'verified':
      return `${progress.artifact} verified · ${formatMegabytes(progress.overallBytes)} of ${formatMegabytes(progress.overallTotal)}`;
  }
}

/** The message of a command error, without its machine-readable code prefix. */
export function errorMessage(error: unknown): string {
  const raw = error instanceof Error ? error.message : String(error);
  for (const code of [SEARCH_MODELS_MISSING_CODE, INSTALL_IN_PROGRESS_CODE]) {
    if (raw.startsWith(`${code}: `)) {
      const rest = raw.slice(code.length + 2);
      return rest.charAt(0).toUpperCase() + rest.slice(1);
    }
  }
  return raw;
}

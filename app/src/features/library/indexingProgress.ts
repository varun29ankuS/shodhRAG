/**
 * `indexing-progress` events on the Library's cards. Each event names the source
 * it is for (`IndexingProgress::space_id`, the Library source id), so only that
 * source's card moves, and only while it is indexing: a source that already
 * failed or finished is never painted as indexing or ready again by a late event.
 * No imports: unit-tested under Node.
 */

/** `shodh_rag::indexing::IndexingProgress` (snake_case). */
export interface IndexingProgressPayload {
  space_id?: string;
  current_file: string;
  processed_files: number;
  total_files: number;
  percentage: number;
  /** OCR position in a scanned PDF (`features/library/sources::scannedPagesStep`). */
  scanned_page?: number | null;
  scanned_pages?: number | null;
}

/** The progress fields of a Library source. */
export interface ProgressFields {
  id: string;
  status: string;
  fileCount: number;
  progress?: number;
  currentFile?: string;
  currentStep?: string;
  processedCount?: number;
}

/** The marker `current_file` of a run's last event. */
export const COMPLETED_FILE = 'Completed';

/** Whether `payload` is about `source`: it is indexing and the event names it (or names none). */
export function progressApplies(source: Pick<ProgressFields, 'id' | 'status'>, payload: IndexingProgressPayload): boolean {
  if (source.status !== 'indexing') return false;
  const space = payload.space_id?.trim();
  return !space || space === source.id;
}

/** `source` with `payload` applied (unchanged when it is not about it). */
export function withIndexingProgress<S extends ProgressFields>(source: S, payload: IndexingProgressPayload, currentStep: string | undefined): S {
  if (!progressApplies(source, payload)) return source;
  return {
    ...source,
    status: payload.percentage >= 100 ? 'ready' : 'indexing',
    progress: Math.round(payload.percentage),
    currentFile: payload.current_file === COMPLETED_FILE ? undefined : payload.current_file,
    currentStep,
    fileCount: payload.total_files || source.fileCount,
    processedCount: payload.processed_files,
  };
}

/** Whether `payload` ends the run of `indexingId` (the source the Library is indexing). */
export function endsRun(payload: IndexingProgressPayload, indexingId: string | null): boolean {
  if (payload.percentage < 100 || payload.current_file !== COMPLETED_FILE) return false;
  const space = payload.space_id?.trim();
  return !space || space === indexingId;
}

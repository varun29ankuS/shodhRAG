/**
 * Who wrote a question: the user (`typed`), the app (`composed`: a side-thread summary
 * request), or the code review pane (`review`: a fix or a check it sent). Only `typed`
 * text is learned from or taken for the user's own words; a `review` question also
 * makes every edit and write of its answer ask in every mode (`agent_send`'s
 * `reviewAction`). A retry keeps the origin of the question it repeats, so a review's
 * message never becomes typed text. Kept in the message's metadata under `composedBy`.
 *
 * Pure (no imports) so it is unit-tested directly with Node (`app/tests/messageOrigin.test.ts`).
 */

export type MessageOrigin = 'typed' | 'composed' | 'review';

export const ORIGIN_METADATA_KEY = 'composedBy';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Whether stored metadata marks the question as sent from the code review pane. */
export function readFromReview(metadata: unknown): boolean {
  return isRecord(metadata) && metadata[ORIGIN_METADATA_KEY] === 'review';
}

/** Metadata with the review mark set or removed, keeping every other key. */
export function metadataWithFromReview(metadata: unknown, fromReview: boolean): Record<string, unknown> | undefined {
  const base: Record<string, unknown> = isRecord(metadata) ? { ...metadata } : {};
  delete base[ORIGIN_METADATA_KEY];
  if (fromReview) base[ORIGIN_METADATA_KEY] = 'review';
  return Object.keys(base).length > 0 ? base : undefined;
}

/** The origin of a question being sent. */
export function originOf(question: { sideSummary?: unknown; fromReview?: boolean; composed?: boolean }): MessageOrigin {
  if (question.fromReview) return 'review';
  if (question.sideSummary || question.composed) return 'composed';
  return 'typed';
}

/** What `agent_send` is told about a question of `origin`. */
export function wireOrigin(origin: MessageOrigin): { textOrigin: 'typed' | 'composed'; reviewAction: boolean } {
  return { textOrigin: origin === 'typed' ? 'typed' : 'composed', reviewAction: origin === 'review' };
}

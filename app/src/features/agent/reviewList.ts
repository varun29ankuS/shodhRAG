/**
 * The files a Code command prompt lists as changed without review
 * (`code_mode.rs` `UNREVIEWED_KEY`, `REVIEW_TOKEN_KEY`). The prompt shows
 * every file; only its Approve button sends the token back, which is what
 * lets the backend mark exactly those files reviewed. Approvals without it
 * (the Inbox, a keyboard shortcut) run the command and mark nothing.
 */

/** Preview key of every file changed without review. */
export const UNREVIEWED_KEY = 'changed_without_review';

/** Preview key of the token the prompt sends back with its approval. */
export const REVIEW_TOKEN_KEY = 'review_token';

/** Preview keys shown as their own block or never shown, not as rows. */
export const REVIEW_BLOCK_KEYS: readonly string[] = [UNREVIEWED_KEY, REVIEW_TOKEN_KEY];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Every file the preview lists as changed without review (none: an empty list). */
export function unreviewedFiles(preview: unknown): string[] {
  if (!isRecord(preview) || !Array.isArray(preview[UNREVIEWED_KEY])) return [];
  return (preview[UNREVIEWED_KEY] as unknown[]).filter((file): file is string => typeof file === 'string');
}

/**
 * The token to send with an approval, or `undefined`. Only a prompt that
 * renders the whole list may send it, so callers pass whether they did.
 */
export function reviewTokenFor(preview: unknown, listShown: boolean): string | undefined {
  if (!listShown || !isRecord(preview)) return undefined;
  const token = preview[REVIEW_TOKEN_KEY];
  return typeof token === 'string' && token.trim() !== '' ? token : undefined;
}

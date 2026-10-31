/**
 * What a file's verification means, in plain words. Shodh checks every file it
 * downloads (search models, the answer engine, the answer-check and scan-reader
 * models) against the checksum of the official copy; people who are not
 * technical are told what that check means, and the checksums themselves stay
 * behind a "Technical details" disclosure for diagnostics.
 *
 * Pure (no imports) so it is unit-tested directly with Node
 * (`app/tests/verification.test.ts`).
 */

/** "Verified: this file matches the official copy from GitHub, …". */
export function verifiedSentence(source: string, plural = false): string {
  return plural
    ? `Verified: these files match the official copies from ${source}, so they haven't been changed or tampered with.`
    : `Verified: this file matches the official copy from ${source}, so it hasn't been changed or tampered with.`;
}

/**
 * The answer engine after its download was checked. With a pinned checksum (Windows) it
 * matches the copy Shodh was built against; elsewhere it is checked against the checksum
 * published with the same release, which proves it arrived complete and unchanged, not
 * that the release itself is genuine, so it says only that.
 */
export function runtimeVerifiedSentence(source: string, pinned: boolean): string {
  return pinned
    ? verifiedSentence(source)
    : `Checked: this file matches what ${source} published with the release, so it arrived complete and unchanged.`;
}

/** Before the answer engine's download: what its check will mean. */
export function runtimeCheckPromise(source: string, pinned: boolean): string {
  return pinned
    ? verificationPromise(source)
    : `Shodh downloads it from ${source} and checks that it matches what was published with the release before using it, so a damaged download is never run.`;
}

/** The sentence for a file that failed its check. */
export function unverifiedSentence(plural = false): string {
  return plural
    ? "Couldn't verify these files: they don't match the official copies, so Shodh won't use them. Try again to download them again."
    : "Couldn't verify this file: it doesn't match the official copy, so Shodh won't use it. Try again to download it again.";
}

/** Before a download: what the check will mean. */
export function verificationPromise(source: string): string {
  return `Shodh downloads it from ${source} and checks that it matches the official copy before using it, so a damaged or changed file is never run.`;
}

/** One row of a "Technical details" disclosure. */
export interface TechnicalRow {
  label: string;
  value: string;
}

/** A SHA-256 checksum as the disclosure shows it (the whole value: it is for comparing). */
export function checksumRow(label: string, sha256: string): TechnicalRow {
  return { label, value: `SHA-256 ${sha256.toLowerCase()}` };
}

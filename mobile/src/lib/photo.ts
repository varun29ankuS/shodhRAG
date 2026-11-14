/**
 * "Point the camera and ask": the photo goes to the computer, which reads it
 * with its own OCR and keeps it apart from the library; the question then
 * carries the photo's text as its first passage, "Your photo", so the answer
 * cites it as [1] next to the library's passages. "Keep in library" adds the
 * photo to "From my phone"; until then it is never indexed.
 */
import type { Attached, CarriedPassage } from './ask';
import type { PhotoRead } from './device';
import type { AskContext } from './nav';

export const PHOTO_SOURCE = 'Your photo';
export const PHOTO_SCHEME = 'photo://';

export function isPhotoPath(path: string): boolean {
  return path.startsWith(PHOTO_SCHEME);
}

export function photoPassage(read: PhotoRead): CarriedPassage {
  return { n: 1, file: PHOTO_SOURCE, path: `${PHOTO_SCHEME}${read.photoId}`, text: read.text };
}

/** What a question about the photo carries. */
export function photoContext(read: PhotoRead): AskContext {
  const attached: Attached[] = [{ origin: 'photo', text: read.text }];
  return { attached, passages: [photoPassage(read)], photoId: read.photoId };
}

/** The photo's id in a "Your photo" citation's path. */
export function photoIdOf(path: string): string | null {
  return isPhotoPath(path) ? path.slice(PHOTO_SCHEME.length) : null;
}

/**
 * At most one simulation runs at a time in the whole app: starting one
 * pauses whichever was running (an answer's copy, the enlarged copy, a
 * side answer). Pure module, unit-tested with Node.
 */

interface Player {
  id: string;
  pause: () => void;
}

let current: Player | null = null;

/** Make `id` the running simulation, pausing the previous one. */
export function claimPlayback(id: string, pause: () => void): void {
  if (current && current.id !== id) {
    const previous = current;
    current = null;
    previous.pause();
  }
  current = { id, pause };
}

/** `id` stopped running (paused, finished, unmounted). */
export function releasePlayback(id: string): void {
  if (current?.id === id) current = null;
}

/** Whether a simulation other than `id` is running. */
export function otherIsPlaying(id: string): boolean {
  return current !== null && current.id !== id;
}

/** Id of the running simulation, if any. */
export function playingId(): string | null {
  return current?.id ?? null;
}

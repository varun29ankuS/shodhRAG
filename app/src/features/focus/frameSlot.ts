/**
 * At most one pending animation frame. `schedule` is a no-op while a frame is
 * pending; `cancel` drops the pending frame and frees the slot, so a frame
 * cancelled before it ran never blocks later scheduling.
 */
export interface FrameSlot {
  schedule(run: () => void): void;
  cancel(): void;
  readonly pending: boolean;
}

export function createFrameSlot(
  request: (cb: () => void) => number = cb => requestAnimationFrame(cb),
  release: (id: number) => void = id => cancelAnimationFrame(id),
): FrameSlot {
  let id: number | null = null;
  return {
    schedule(run) {
      if (id !== null) return;
      id = request(() => {
        id = null;
        run();
      });
    },
    cancel() {
      if (id === null) return;
      release(id);
      id = null;
    },
    get pending() {
      return id !== null;
    },
  };
}

/**
 * The shell's one polite live region: what the assistant opened, highlighted
 * or could not find is said once, in plain words ("Shodh opened Tasks:
 * Renew lease", "Shodh highlighted Passages per search in Settings"). One
 * region for all of it, so two live regions never talk over each other.
 *
 * `ShellAnnouncer` (components/shell) renders the region; anything may call
 * `announce`. No imports: unit-tested under Node.
 */

type Listener = (message: string) => void;

const listeners = new Set<Listener>();

/** Say `message` through the shell's live region (politely: after what is being read). */
export function announce(message: string): void {
  const text = message.trim();
  if (!text) return;
  for (const listener of listeners) listener(text);
}

/** Listen for announcements; returns the unsubscribe function. */
export function subscribeAnnouncements(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

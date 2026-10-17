/**
 * "Accept N non-sensitive" in Settings → Memory. A suggestion the user rejects is hidden at
 * once and rejected only when its undo window ends (`lib/undoQueue.ts`); until then it is
 * still pending in the backend. The list therefore keeps the ids being rejected, and they
 * are left out of the batch and of the counts, so a declined memory is never saved by the
 * batch accept. Pure and import-free, so Node's strip-types runner tests it
 * (`app/tests/batchAccept.test.ts`).
 */

/** What the batch accept needs of a suggestion. */
export interface BatchCandidate {
  id: string;
  status: string;
  sensitive: readonly unknown[];
}

/** Pending suggestions that can be accepted together: not sensitive, and not being rejected. */
export function batchAcceptable<T extends BatchCandidate>(
  suggestions: readonly T[],
  rejecting: ReadonlySet<string> = new Set(),
): T[] {
  return suggestions.filter(s => s.status === 'pending' && s.sensitive.length === 0 && !rejecting.has(s.id));
}

/** The suggestions still shown: those not being rejected. */
export function withoutRejecting<T extends { id: string }>(list: readonly T[], rejecting: ReadonlySet<string>): T[] {
  return rejecting.size === 0 ? [...list] : list.filter(s => !rejecting.has(s.id));
}

/** `rejecting` with `id` added (`on`) or removed; the same set when nothing changes. */
export function markRejecting(rejecting: ReadonlySet<string>, id: string, on: boolean): ReadonlySet<string> {
  if (rejecting.has(id) === on) return rejecting;
  const next = new Set(rejecting);
  if (on) next.add(id);
  else next.delete(id);
  return next;
}

/**
 * `rejecting` without the ids no longer waiting (their rejection was carried out and the
 * list reloaded); the same set when nothing changes. Ids still waiting stay excluded until
 * then, even after the rejection was sent.
 */
export function pruneRejecting(rejecting: ReadonlySet<string>, waiting: readonly { id: string }[]): ReadonlySet<string> {
  if (rejecting.size === 0) return rejecting;
  const ids = new Set(waiting.map(s => s.id));
  const kept = [...rejecting].filter(id => ids.has(id));
  return kept.length === rejecting.size ? rejecting : new Set(kept);
}

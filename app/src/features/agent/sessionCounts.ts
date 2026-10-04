/** Live agent sessions by kind (`agent_session_commands.rs::SessionCounts`). */
export interface SessionCounts {
  main: number;
  side: number;
}

/** Emitted with `SessionCounts` whenever sessions start or stop. */
export const AGENT_SESSIONS_EVENT = 'agent_sessions_changed';

export function isSessionCounts(value: unknown): value is SessionCounts {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as Record<string, unknown>).main === 'number' &&
    typeof (value as Record<string, unknown>).side === 'number'
  );
}

/** "2 assistant sessions open (1 side discussion)" for the activity tray. */
export function sessionCountsLabel(counts: SessionCounts): string {
  const total = counts.main + counts.side;
  if (total === 0) return 'No assistant sessions open';
  const sessions = `${total} assistant session${total === 1 ? '' : 's'} open`;
  if (counts.side === 0) return sessions;
  return `${sessions} (${counts.side} side discussion${counts.side === 1 ? '' : 's'})`;
}

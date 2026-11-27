/**
 * A launch-tour run scene's times (`LaunchProbeHost`): the fixture's run is recorded at a
 * fixed time, so a live state's timer would read the months since. Its clock is moved so
 * that its last event happened just now. Pure, for the Node tests.
 */

/** How long before now the run's last event happened. */
export const LAST_EVENT_AGO_MS = 2000;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** `seed` and `actions` with every time (`startedAtMs`, `atMs`) moved so the last is `now - LAST_EVENT_AGO_MS`. */
export function rebaseRun(seed: unknown, actions: readonly unknown[], now: number): { seed: unknown; actions: unknown[] } {
  const times: number[] = [];
  if (isRecord(seed) && typeof seed.startedAtMs === 'number') times.push(seed.startedAtMs);
  for (const action of actions) if (isRecord(action) && typeof action.atMs === 'number') times.push(action.atMs);
  if (times.length === 0) return { seed, actions: [...actions] };
  const shift = now - LAST_EVENT_AGO_MS - Math.max(...times);
  const moved = (value: unknown, key: 'startedAtMs' | 'atMs') =>
    isRecord(value) && typeof value[key] === 'number' ? { ...value, [key]: (value[key] as number) + shift } : value;
  return { seed: moved(seed, 'startedAtMs'), actions: actions.map(action => moved(action, 'atMs')) };
}

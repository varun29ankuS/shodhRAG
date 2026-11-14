/**
 * The phone around the app in tests: its light or dark setting, and a
 * scripted computer behind the link (`fakeSource`).
 */
import type { LinkSource, LinkStatus } from '../src/lib/link';
import { FakeLink } from './fakeLink';

// ── The phone's theme setting (matchMedia) ─────────────────────────

let dark = true;
const listeners = new Set<(event: MediaQueryListEvent) => void>();

function mediaList(query: string): MediaQueryList {
  const isDark = query.includes('prefers-color-scheme: dark');
  const list = {
    media: query,
    get matches() {
      return isDark ? dark : false;
    },
    onchange: null,
    addEventListener: (_: string, listener: (event: MediaQueryListEvent) => void) => {
      if (isDark) listeners.add(listener);
    },
    removeEventListener: (_: string, listener: (event: MediaQueryListEvent) => void) => {
      listeners.delete(listener);
    },
    addListener: () => undefined,
    removeListener: () => undefined,
    dispatchEvent: () => true,
  };
  return list as unknown as MediaQueryList;
}

export function resetPhoneTheme() {
  dark = true;
  listeners.clear();
  window.matchMedia = mediaList;
}

/** The phone switches its own setting. */
export function setPhoneTheme(theme: 'light' | 'dark') {
  dark = theme === 'dark';
  for (const listener of [...listeners]) listener({ matches: dark } as MediaQueryListEvent);
}

// ── A scripted computer ────────────────────────────────────────────

export interface FakeSource extends LinkSource {
  /** The link's state changes. */
  setStatus(status: LinkStatus): void;
  /** Every ready line: [screen, theme]. */
  readies: Array<[string, string]>;
  reconnects: number;
}

export function fakeSource(link: FakeLink, initial: LinkStatus): FakeSource {
  let status = initial;
  const handlers = new Set<(status: LinkStatus) => void>();
  const source: FakeSource = {
    transport: link,
    status: async () => status,
    onStatus: async handler => {
      handlers.add(handler);
      return () => handlers.delete(handler);
    },
    reconnect: async () => {
      source.reconnects += 1;
    },
    pair: async () => undefined,
    reset: async () => undefined,
    ready: async (screen, theme) => {
      source.readies.push([screen, theme]);
    },
    startScreen: async () => null,
    setStatus(next) {
      status = next;
      for (const handler of [...handlers]) handler(next);
    },
    readies: [],
    reconnects: 0,
  };
  return source;
}

/** A computer that answers `answers[command]` (a value, or a function of the arguments). */
export function computer(answers: Record<string, unknown>): FakeLink {
  return new FakeLink(async (command, args) => {
    if (!(command in answers)) throw { code: 'runtime_error', message: `no script for ${command}` };
    const answer = answers[command];
    return typeof answer === 'function' ? (answer as (args: Record<string, unknown>) => unknown)(args) : answer;
  });
}

/** A computer that never answers: everything stays loading. */
export function silentComputer(): FakeLink {
  return new FakeLink(() => new Promise(() => undefined));
}

export const CONNECTED: LinkStatus = { state: 'connected' };
export const ASLEEP: LinkStatus = { state: 'offline', message: 'Your computer did not answer.' };

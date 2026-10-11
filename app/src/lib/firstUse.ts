/**
 * One-time explanations ("first use" hints): whether each was dismissed,
 * kept in local storage, with listeners so every place showing the same hint
 * hides it together.
 *
 * Pure module (storage injected), unit-tested with Node (`app/tests/firstUse.test.ts`).
 */

export type FirstUseHint =
  /** The grounding chip under an answer: what it measures. */
  | 'grounding-chip'
  /** Flagged statements in an answer: what to do about them. */
  | 'citation-flags';

export const FIRST_USE_STORAGE_KEY = 'shodh.firstUse.v1';

const HINTS: readonly FirstUseHint[] = ['grounding-chip', 'citation-flags'];

export interface HintStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export interface FirstUseStore {
  seen(hint: FirstUseHint): boolean;
  dismiss(hint: FirstUseHint): void;
  subscribe(listener: () => void): () => void;
}

/**
 * The hint store. `storage` null (or failing) keeps dismissals for this
 * session only; an unreadable record counts as nothing dismissed.
 */
export function createFirstUseStore(storage: HintStorage | null): FirstUseStore {
  const dismissed = new Set<FirstUseHint>();
  try {
    const parsed: unknown = JSON.parse(storage?.getItem(FIRST_USE_STORAGE_KEY) ?? '[]');
    if (Array.isArray(parsed)) {
      for (const item of parsed) {
        if ((HINTS as readonly unknown[]).includes(item)) dismissed.add(item as FirstUseHint);
      }
    }
  } catch {
    // Unreadable: show the hints again.
  }
  const listeners = new Set<() => void>();
  return {
    seen: hint => dismissed.has(hint),
    dismiss: hint => {
      if (dismissed.has(hint)) return;
      dismissed.add(hint);
      try {
        storage?.setItem(FIRST_USE_STORAGE_KEY, JSON.stringify([...dismissed]));
      } catch {
        // Not persisted; hidden for this session.
      }
      for (const listener of [...listeners]) listener();
    },
    subscribe: listener => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

function browserStorage(): HintStorage | null {
  try {
    return typeof window !== 'undefined' ? window.localStorage : null;
  } catch {
    return null;
  }
}

let shared: FirstUseStore | null = null;

/** The app's hint store (local storage). */
export function firstUseStore(): FirstUseStore {
  shared ??= createFirstUseStore(browserStorage());
  return shared;
}

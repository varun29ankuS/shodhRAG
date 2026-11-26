/**
 * The lock in front of the app: once this phone is paired with a computer,
 * Shodh opens only after the PIN (or the fingerprint), and asks again when it
 * comes back after a minute away. While it is locked nothing behind it runs,
 * so nothing asks the computer (the app's Rust side refuses anyway).
 *
 * Before pairing there is nothing to protect, so the pairing screen shows.
 * A debug build opened at a screen for CI's screenshots (`skip`) is not locked.
 */
import { useEffect, useState, type ReactNode } from 'react';
import { useLinkStatus } from '../lib/context';
import type { LockStatus } from '../lib/device';
import type { LinkSource } from '../lib/link';
import { useDevice } from '../lib/queue';
import { LockScreen } from './LockScreen';
import { IOS } from '../lib/platform';

export function LockGate({ source, skip, children }: { source: LinkSource; skip: boolean; children: ReactNode }) {
  const { lock } = useDevice();
  const status = useLinkStatus(source);
  const [state, setState] = useState<LockStatus | null>(null);

  useEffect(() => {
    let alive = true;
    lock
      .status()
      .then(next => {
        if (alive) setState(next);
      })
      .catch(() => {
        // The lock could not be read: it stays shut.
        if (alive) setState({ required: true, hasPin: true, biometric: false, biometry: IOS ? 'touch' : 'fingerprint', locked: true });
      });
    return () => {
      alive = false;
    };
  }, [lock]);

  // Away and back: locked again after a minute.
  useEffect(() => {
    const changed = () => {
      if (document.visibilityState === 'hidden') {
        lock.hidden().catch(() => undefined);
        return;
      }
      lock
        .shown()
        .then(locked => {
          if (locked) setState(s => (s ? { ...s, locked: true } : s));
        })
        .catch(() => undefined);
    };
    document.addEventListener('visibilitychange', changed);
    return () => document.removeEventListener('visibilitychange', changed);
  }, [lock]);

  const paired = !['unpaired', 'refused', 'starting', 'failed'].includes(status.state);
  const shut = !skip && paired && (state === null || (state.required && (state.locked || !state.hasPin)));
  if (shut && state !== null) {
    return <LockScreen status={state} onOpen={() => setState({ ...state, hasPin: true, locked: false })} />;
  }
  if (shut) return null;
  return <>{children}</>;
}

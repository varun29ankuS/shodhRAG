/**
 * The lock, on by default: Shodh opens with the PIN (or the phone's
 * fingerprint, when the user turned it on), so a lost or borrowed phone
 * cannot ask the user's computer anything. The first time, the user sets the
 * PIN. Five wrong PINs and it waits 30 seconds (the app's Rust side counts,
 * and refuses every call to the computer while it is locked).
 */
import { useState, type FormEvent } from 'react';
import { Fingerprint, LockKeyhole } from 'lucide-react';
import type { LockStatus } from '../lib/device';
import { errorText } from '../lib/link';
import { useDevice } from '../lib/queue';

export function LockScreen({ status, onOpen }: { status: LockStatus; onOpen: () => void }) {
  const { lock } = useDevice();
  const [pin, setPin] = useState('');
  const [again, setAgain] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const setting = !status.hasPin;

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (setting && pin !== again) {
      setError('The two PINs are not the same.');
      return;
    }
    setBusy(true);
    setError(null);
    (setting ? lock.setPin(pin) : lock.unlock(pin))
      .then(onOpen)
      .catch(e => {
        setError(errorText(e));
        setPin('');
        setAgain('');
      })
      .finally(() => setBusy(false));
  };

  const fingerprint = () => {
    setError(null);
    lock
      .unlockBiometric()
      .then(onOpen)
      .catch(e => setError(errorText(e)));
  };

  return (
    <div className="lock" role="dialog" aria-modal="true" aria-label={setting ? 'Set a PIN' : 'Shodh is locked'}>
      <span className="panel-icon">
        <LockKeyhole aria-hidden="true" className="icon" />
      </span>
      <h1 className="title">{setting ? 'Keep Shodh private' : 'Shodh is locked'}</h1>
      <p className="panel-text">
        {setting
          ? 'Set a PIN of 4 to 12 digits. Shodh asks for it when it opens, so nobody else can ask your computer anything from this phone. You can change this in Settings.'
          : 'Enter your PIN to open Shodh.'}
      </p>
      <form className="lock-form" onSubmit={submit}>
        <label htmlFor="lock-pin" className="visually-hidden">
          {setting ? 'Your new PIN' : 'Your PIN'}
        </label>
        <input id="lock-pin" type="password" inputMode="numeric" autoComplete="off" value={pin} onChange={e => setPin(e.target.value.replace(/\D/g, ''))} placeholder="PIN" />
        {setting && (
          <>
            <label htmlFor="lock-again" className="visually-hidden">
              The new PIN again
            </label>
            <input id="lock-again" type="password" inputMode="numeric" autoComplete="off" value={again} onChange={e => setAgain(e.target.value.replace(/\D/g, ''))} placeholder="The PIN again" />
          </>
        )}
        <button type="submit" className="button button-primary button-wide" disabled={busy || pin.length < 4}>
          <LockKeyhole aria-hidden="true" className="icon-small" />
          {setting ? 'Set the PIN' : 'Unlock'}
        </button>
      </form>
      {!setting && status.biometric && (
        <button type="button" className="button button-wide" onClick={fingerprint}>
          <Fingerprint aria-hidden="true" className="icon-small" />
          Use your fingerprint
        </button>
      )}
      {error && (
        <p className="panel-text" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

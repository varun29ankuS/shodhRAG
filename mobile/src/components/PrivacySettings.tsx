/**
 * Settings: Notifications (off by default) and the lock (on by default).
 *
 * Notifications never carry content: only "Shodh needs your approval" or
 * "Your answer is ready"; the details come over the encrypted link when the
 * app opens. Off: only while the app is open. Stay connected: a lasting
 * notification keeps Shodh connected. Push: the user's UnifiedPush app (such
 * as ntfy) wakes Shodh; the computer sends it only that one word.
 *
 * iPhone has neither: iOS suspends an app about half a minute after it
 * leaves the screen, and has no UnifiedPush. There the choice is Off or
 * Briefly (the same `stay` mode, for the time iOS allows), and it says so.
 */
import { useEffect, useState } from 'react';
import { Bell, BellOff, BellRing, Fingerprint, LockKeyhole, ScanFace } from 'lucide-react';
import { useNote } from '../lib/context';
import type { LockStatus, NotifyMode } from '../lib/device';
import { errorText } from '../lib/link';
import { BIOMETRY, IOS } from '../lib/platform';
import { useDevice } from '../lib/queue';

type ModeChoice = { value: NotifyMode; label: string; words: string };

const ANDROID_MODES: readonly ModeChoice[] = [
  { value: 'off', label: 'Off', words: 'Shodh tells you only while it is open.' },
  { value: 'stay', label: 'Stay connected', words: 'A lasting notification keeps Shodh connected to your computer, so it can tell you when an approval or an answer waits. It uses a little more battery.' },
  { value: 'push', label: 'Push', words: 'Your UnifiedPush app (such as ntfy) wakes Shodh. Your computer sends it one word, never what it is about.' },
];

const IOS_MODES: readonly ModeChoice[] = [
  { value: 'off', label: 'Off', words: 'Shodh tells you only while it is open.' },
  { value: 'stay', label: 'Briefly', words: 'Shodh stays connected for about 30 seconds after you leave it, and tells you if an approval or an answer arrives in that time. iOS then pauses it.' },
];

const MODES = IOS ? IOS_MODES : ANDROID_MODES;

const ICONS = { off: BellOff, stay: BellRing, push: Bell } as const;

export function NotificationSettings() {
  const { notifications } = useDevice();
  const { show } = useNote();
  const [mode, setMode] = useState<NotifyMode | null>(null);
  const [apps, setApps] = useState<string[] | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    notifications
      .mode()
      .then(setMode)
      .catch(() => setMode('off'));
  }, [notifications]);

  const choose = async (next: NotifyMode) => {
    if (next === 'push') {
      const found = await notifications.distributors().catch(() => []);
      setApps(found);
      if (found.length !== 1) return;
      return save(next, found[0]);
    }
    setApps(null);
    return save(next);
  };

  const save = (next: NotifyMode, app?: string) => {
    setSaving(true);
    return notifications
      .set(next, app)
      .then(setMode)
      .catch(e => show(errorText(e), 'problem'))
      .finally(() => setSaving(false));
  };

  const current = MODES.find(m => m.value === mode);
  return (
    <div className="card">
      <div className="setting">
        <span className="row-title">When Shodh is closed</span>
        <div className="segmented" role="radiogroup" aria-label="Notifications">
          {MODES.map(option => {
            const Icon = ICONS[option.value];
            return (
              <button
                key={option.value}
                type="button"
                role="radio"
                className="segment"
                aria-checked={mode === option.value}
                disabled={saving || mode === null}
                onClick={() => void choose(option.value)}
              >
                <Icon aria-hidden="true" className="icon-small" /> {option.label}
              </button>
            );
          })}
        </div>
        {current && <span className="row-sub">{current.words}</span>}
        <span className="row-sub">Every notification says only what waits, never what it is about.</span>
        {apps !== null && apps.length === 0 && <span className="row-sub">No UnifiedPush app is on this phone. Install one, such as ntfy, then choose Push again.</span>}
        {apps !== null && apps.length > 1 && (
          <div className="choices" role="radiogroup" aria-label="Push app">
            {apps.map(app => (
              <button key={app} type="button" role="radio" className="choice" aria-checked={false} onClick={() => void save('push', app)}>
                <Bell aria-hidden="true" className="icon-small" />
                <span className="row-title">{app}</span>
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

export function LockSettings() {
  const { lock } = useDevice();
  const { show } = useNote();
  const [state, setState] = useState<LockStatus | null>(null);
  const [pin, setPin] = useState('');
  const [newPin, setNewPin] = useState('');

  const reload = () => {
    lock
      .status()
      .then(setState)
      .catch(e => show(errorText(e), 'problem'));
  };
  useEffect(reload, [lock]);

  const change = (what: { enabled?: boolean; biometric?: boolean }) =>
    lock
      .configure(pin, what)
      .then(() => {
        setPin('');
        reload();
      })
      .catch(e => show(errorText(e), 'problem'));

  if (state === null) return null;
  return (
    <div className="card">
      <div className="setting">
        <span className="row-title">
          <LockKeyhole aria-hidden="true" className="icon-small" /> {state.required ? 'Shodh asks for your PIN when it opens' : 'Shodh opens without a PIN'}
        </span>
        <span className="row-sub">
          {state.required
            ? 'Nobody else can ask your computer anything from this phone. After a minute away, it asks again.'
            : 'Anyone with this phone can ask your computer. Turn the lock on to keep it private.'}
        </span>
        <label htmlFor="lock-current" className="visually-hidden">
          Your PIN, to change the lock
        </label>
        <input
          id="lock-current"
          className="pin-field"
          type="password"
          inputMode="numeric"
          autoComplete="off"
          placeholder="Your PIN, to change this"
          value={pin}
          onChange={e => setPin(e.target.value.replace(/\D/g, ''))}
        />
        <div className="sheet-actions">
          <button type="button" className="button" disabled={pin.length < 4} onClick={() => void change({ enabled: !state.required })}>
            <LockKeyhole aria-hidden="true" className="icon-small" />
            {state.required ? 'Turn off' : 'Turn on'}
          </button>
          <button type="button" className="button" disabled={pin.length < 4 || !state.required} onClick={() => void change({ biometric: !state.biometric })}>
            {state.biometry === 'face' ? <ScanFace aria-hidden="true" className="icon-small" /> : <Fingerprint aria-hidden="true" className="icon-small" />}
            {state.biometric ? BIOMETRY[state.biometry].off : BIOMETRY[state.biometry].on}
          </button>
        </div>
        <label htmlFor="lock-new" className="visually-hidden">
          A new PIN
        </label>
        <input id="lock-new" className="pin-field" type="password" inputMode="numeric" autoComplete="off" placeholder="A new PIN" value={newPin} onChange={e => setNewPin(e.target.value.replace(/\D/g, ''))} />
        <button
          type="button"
          className="button"
          disabled={newPin.length < 4}
          onClick={() =>
            void lock
              .setPin(newPin)
              .then(() => {
                setNewPin('');
                show('Your PIN is changed.');
                reload();
              })
              .catch(e => show(errorText(e), 'problem'))
          }
        >
          <LockKeyhole aria-hidden="true" className="icon-small" />
          Change the PIN
        </button>
      </div>
    </div>
  );
}

/**
 * The connection to the computer, in plain words: a dot in the header and a
 * line under the greeting, and a panel that takes the place of a screen's
 * content while the computer cannot be asked.
 */
import { useState } from 'react';
import { LoaderCircle, Monitor, RefreshCw, Smartphone, TriangleAlert } from 'lucide-react';
import { useLink } from '../lib/context';
import { errorText, type LinkStatus } from '../lib/link';

export interface ConnectionWords {
  dot: 'connected' | 'waiting' | 'off';
  /** Short: the menu and the header's label. */
  short: string;
  /** A sentence: under the greeting. */
  line: string;
}

export function connectionWords(status: LinkStatus): ConnectionWords {
  switch (status.state) {
    case 'connected':
      return { dot: 'connected', short: 'Connected', line: 'Your computer is connected.' };
    case 'starting':
    case 'connecting':
      return { dot: 'waiting', short: 'Connecting', line: 'Connecting to your computer…' };
    case 'offline':
      return { dot: 'waiting', short: 'Computer asleep', line: 'Your computer is asleep or offline. Ask when it is back.' };
    case 'unpaired':
    case 'refused':
      return { dot: 'off', short: 'Not paired', line: 'This phone is not paired with a computer yet.' };
    case 'update':
      return { dot: 'off', short: 'Update needed', line: 'Update Shodh on your phone to match your computer.' };
    case 'failed':
      return { dot: 'off', short: 'Not working', line: 'Shodh could not start its link to your computer.' };
  }
}

export function Dot({ status, small = false }: { status: LinkStatus; small?: boolean }) {
  const { dot } = connectionWords(status);
  return <span className={`dot dot-${dot}${small ? ' dot-small' : ''}`} aria-hidden="true" />;
}

/**
 * What a screen shows instead of its content while the computer cannot be
 * asked. `what` names the content ("your conversations").
 */
export function NotConnected({ what }: { what: string }) {
  const { status, reconnect, openPairing } = useLink();
  const [trying, setTrying] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const tryAgain = () => {
    setTrying(true);
    setError(null);
    reconnect()
      .catch(e => setError(errorText(e)))
      .finally(() => setTrying(false));
  };

  if (status.state === 'connecting' || status.state === 'starting') {
    return (
      <section className="panel" aria-busy="true">
        <span className="panel-icon">
          <LoaderCircle aria-hidden="true" className="icon spin" />
        </span>
        <p className="panel-title">Connecting to your computer</p>
        <p className="panel-text">{`Shodh shows ${what} as soon as your computer answers.`}</p>
      </section>
    );
  }
  if (status.state === 'unpaired' || status.state === 'refused') {
    return (
      <section className="panel">
        <span className="panel-icon">
          <Smartphone aria-hidden="true" className="icon" />
        </span>
        <p className="panel-title">Pair this phone with your computer</p>
        <p className="panel-text">
          {`Your phone shows ${what} from Shodh on your computer. On your computer, open Settings, then Phone, and choose Pair a phone.`}
        </p>
        <button type="button" className="button button-primary" onClick={openPairing}>
          <Smartphone aria-hidden="true" className="icon-small" />
          Pair this phone
        </button>
      </section>
    );
  }
  if (status.state === 'offline') {
    return (
      <section className="panel">
        <span className="panel-icon">
          <Monitor aria-hidden="true" className="icon" />
        </span>
        <p className="panel-title">Your computer is asleep or offline</p>
        <p className="panel-text">{`Shodh shows ${what} when your computer is awake and Shodh is open on it.`}</p>
        <button type="button" className="button" disabled={trying} onClick={tryAgain}>
          {trying ? <LoaderCircle aria-hidden="true" className="icon-small spin" /> : <RefreshCw aria-hidden="true" className="icon-small" />}
          Try again
        </button>
        {error && <p className="panel-text">{error}</p>}
      </section>
    );
  }
  // An update is needed, or the link could not start.
  const message = 'message' in status ? status.message : '';
  return (
    <section className="panel">
      <span className="panel-icon">
        <TriangleAlert aria-hidden="true" className="icon" />
      </span>
      <p className="panel-title">{connectionWords(status).line}</p>
      {message && <p className="panel-text">{message}</p>}
    </section>
  );
}

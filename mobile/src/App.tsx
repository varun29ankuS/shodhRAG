import { useEffect, useRef, useState } from 'react';
import { RotateCcw, TriangleAlert } from 'lucide-react';
import { errorText, linkStatus, onLinkStatus, resetLink, screenShown, type LinkStatus } from './lib/link';
import { Connected } from './screens/Connected';
import { Pair } from './screens/Pair';
import logo from '../src-tauri/icons/icon.png';

type Screen = 'pair' | 'connected' | 'offline' | 'failed';

function screenOf(status: LinkStatus): Screen | null {
  switch (status.state) {
    case 'starting':
      return null;
    case 'unpaired':
    case 'refused':
      return 'pair';
    case 'connected':
    case 'connecting':
      return 'connected';
    case 'offline':
    case 'update':
      return 'offline';
    case 'failed':
      return 'failed';
  }
}

export function App() {
  const [status, setStatus] = useState<LinkStatus>({ state: 'starting' });

  useEffect(() => {
    let alive = true;
    let stop: (() => void) | null = null;
    // Listen first, then read: no change is missed in between.
    onLinkStatus(next => {
      if (alive) setStatus(next);
    })
      .then(unlisten => {
        if (!alive) unlisten();
        else stop = unlisten;
        return linkStatus();
      })
      .then(current => {
        if (alive) setStatus(current);
      })
      .catch(error => {
        if (alive) setStatus({ state: 'failed', message: errorText(error) });
      });
    return () => {
      alive = false;
      stop?.();
    };
  }, []);

  const screen = screenOf(status);
  const told = useRef(false);
  useEffect(() => {
    if (screen === null || told.current) return;
    told.current = true;
    screenShown(screen).catch(() => {
      // The app logged why it is not ready, and the screen says it too.
    });
  }, [screen]);

  return (
    <div className="app">
      <header className="app-header">
        <img src={logo} alt="" className="app-logo" />
        <span className="app-name">Shodh</span>
      </header>
      <main className="app-main">
        {screen === null && <p className="muted">Starting…</p>}
        {screen === 'pair' && <Pair refusal={status.state === 'refused' ? status.message : null} />}
        {(screen === 'connected' || screen === 'offline') && <Connected status={status} />}
        {status.state === 'failed' && <Failed message={status.message} />}
      </main>
    </div>
  );
}

function Failed({ message }: { message: string }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const startOver = () => {
    setBusy(true);
    setError(null);
    resetLink()
      .catch(e => setError(errorText(e)))
      .finally(() => setBusy(false));
  };
  return (
    <section className="screen">
      <h1>Shodh could not start</h1>
      <div className="callout callout-error" role="alert">
        <TriangleAlert aria-hidden="true" className="icon" />
        <p>{message}</p>
      </div>
      <p className="muted">
        If this phone's data came from another phone, start over: this phone gets a new key, and you pair it with your
        computer again.
      </p>
      <button type="button" className="button" disabled={busy} onClick={startOver}>
        <RotateCcw aria-hidden="true" className="icon" />
        Start over
      </button>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

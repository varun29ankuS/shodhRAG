/** The link could not start (the phone's secure storage could not be read): start over. */
import { useState } from 'react';
import { RotateCcw, TriangleAlert } from 'lucide-react';
import { useLink } from '../lib/context';
import { errorText } from '../lib/link';

export function Failed({ message }: { message: string }) {
  const { source } = useLink();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const startOver = () => {
    setBusy(true);
    setError(null);
    source
      .reset()
      .catch(e => setError(errorText(e)))
      .finally(() => setBusy(false));
  };
  return (
    <div className="view">
      <div className="scroll">
        <div className="pair" style={{ paddingTop: 24 }}>
          <h1 className="title">Shodh could not start</h1>
          <div className="callout callout-error" role="alert">
            <TriangleAlert aria-hidden="true" className="icon" />
            <p>{message}</p>
          </div>
          <p className="panel-text">
            Data from another phone? Start over and pair again.
          </p>
          <button type="button" className="button button-primary" disabled={busy} onClick={startOver}>
            <RotateCcw aria-hidden="true" className="icon-small" />
            Start over
          </button>
          {error && (
            <p className="panel-text" role="alert">
              {error}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

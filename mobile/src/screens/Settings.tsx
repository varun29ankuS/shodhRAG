/**
 * The phone's own settings: its computer and the connection, and how it
 * looks. Everything else is set on the computer.
 */
import { useState } from 'react';
import { motion } from 'framer-motion';
import { LoaderCircle, RefreshCw, Smartphone } from 'lucide-react';
import { TopBar } from '../components/Bars';
import { connectionWords, Dot } from '../components/Connection';
import { useLink, useThemeChoice } from '../lib/context';
import { errorText } from '../lib/link';
import { viewIn } from '../lib/motion';
import { THEME_CHOICES } from '../lib/theme';
import { version } from '../../package.json';

export function Settings() {
  const { status, reconnect, openPairing } = useLink();
  const { choice, theme, choose } = useThemeChoice();
  const [trying, setTrying] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const words = connectionWords(status);
  const paired = status.state !== 'unpaired' && status.state !== 'refused';

  const tryAgain = () => {
    setTrying(true);
    setError(null);
    reconnect()
      .catch(e => setError(errorText(e)))
      .finally(() => setTrying(false));
  };

  return (
    <motion.div className="view" variants={viewIn} initial="hidden" animate="shown" exit="gone">
      <div className="scroll">
        <TopBar brand={false} />
        <h1 className="title serif pad">Settings</h1>

        <div className="label">This phone</div>
        <div className="card">
          <div className="setting">
            <span className="row-title">Your computer</span>
            <span className="connection" style={{ fontSize: 14 }}>
              <Dot status={status} small />
              {words.line}
            </span>
            {'message' in status && status.message && <span className="row-sub">{status.message}</span>}
            {paired && status.state !== 'connected' && (
              <button type="button" className="button" disabled={trying} onClick={tryAgain}>
                {trying ? <LoaderCircle aria-hidden="true" className="icon-small spin" /> : <RefreshCw aria-hidden="true" className="icon-small" />}
                Try again
              </button>
            )}
            {!paired && (
              <button type="button" className="button button-primary" onClick={openPairing}>
                <Smartphone aria-hidden="true" className="icon-small" />
                Pair this phone
              </button>
            )}
            {error && <span className="row-sub">{error}</span>}
          </div>
        </div>

        <div className="label">Appearance</div>
        <div className="card">
          <div className="setting">
            <span className="row-title">Theme</span>
            <span className="row-sub">{choice === 'phone' ? `Follows your phone (now ${theme})` : `Always ${theme}`}</span>
            <div className="segmented" role="radiogroup" aria-label="Theme">
              {THEME_CHOICES.map(option => (
                <button
                  key={option.value}
                  type="button"
                  role="radio"
                  className="segment"
                  aria-checked={choice === option.value}
                  onClick={() => choose(option.value)}
                >
                  {option.label}
                </button>
              ))}
            </div>
          </div>
        </div>

        <div className="label">About</div>
        <div className="card">
          <div className="setting">
            <span className="row-title">{`Shodh ${version}`}</span>
            <span className="row-sub">Your phone and your computer run the same version of Shodh.</span>
          </div>
        </div>
      </div>
    </motion.div>
  );
}

/**
 * Pair with the computer: scan its code, or paste it.
 *
 * Android's scanner is a screen of its own, with its own way back. iOS's
 * draws the camera behind the page, so on iPhone the page steps aside while
 * it scans (`html.scanning`, theme.css) and shows only the frame to aim with
 * and Cancel.
 */
import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { motion } from 'framer-motion';
import { ChevronLeft, ClipboardPaste, LoaderCircle, ScanQrCode, Settings, TriangleAlert, X } from 'lucide-react';
import { cancel, checkPermissions, Format, openAppSettings, requestPermissions, scan } from '@tauri-apps/plugin-barcode-scanner';
import { useLink } from '../lib/context';
import { errorText } from '../lib/link';
import { viewIn } from '../lib/motion';
import { IOS } from '../lib/platform';
import logo from '../../src-tauri/icons/icon.png';

/** What iOS's scanner answers when the user chose Cancel ("cancelled"). */
const CANCELLED = /cancell?ed/i;

/** iPhone only: the frame to aim with and Cancel, over the camera. */
function Scanning({ onCancel }: { onCancel: () => void }) {
  useEffect(() => {
    document.documentElement.classList.add('scanning');
    return () => document.documentElement.classList.remove('scanning');
  }, []);
  return createPortal(
    <div className="scan" role="dialog" aria-modal="true" aria-label="Scan the code">
      <div className="scan-frame" aria-hidden="true" />
      <div className="scan-bar">
        <p className="scan-hint">Point at the code</p>
        <button type="button" className="button button-wide" onClick={onCancel}>
          <X aria-hidden="true" className="icon-small" />
          Cancel
        </button>
      </div>
    </div>,
    document.body,
  );
}

export function Pair({ refusal, onClose }: { refusal: string | null; onClose: (() => void) | null }) {
  const { source } = useLink();
  const [code, setCode] = useState('');
  const [busy, setBusy] = useState<'scan' | 'pair' | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cameraDenied, setCameraDenied] = useState(false);
  const [aiming, setAiming] = useState(false);

  const pairWith = async (text: string) => {
    setBusy('pair');
    setError(null);
    try {
      await source.pair(text.trim());
      onClose?.();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const scanCode = async () => {
    setBusy('scan');
    setError(null);
    try {
      let permission = await checkPermissions();
      if (permission !== 'granted') permission = await requestPermissions();
      if (permission !== 'granted') {
        setCameraDenied(true);
        setBusy(null);
        return;
      }
      setCameraDenied(false);
      setAiming(IOS);
      const scanned = await scan({ formats: [Format.QRCode], windowed: IOS }).finally(() => setAiming(false));
      await pairWith(scanned.content);
    } catch (e) {
      // Cancel is the user's own choice, not a problem to report.
      if (!CANCELLED.test(errorText(e))) setError(errorText(e));
      setBusy(null);
    }
  };

  return (
    <motion.div className="view" variants={viewIn} initial="hidden" animate="shown" exit="gone">
      <div className="scroll">
        <header className="topbar">
          {onClose ? (
            <button type="button" className="back" onClick={onClose}>
              <ChevronLeft aria-hidden="true" className="icon-back" />
              <span>Back</span>
            </button>
          ) : (
            <span className="brand" style={{ paddingLeft: 10 }}>
              <img src={logo} alt="" className="brand-logo" width={34} height={34} />
              <span className="brand-name">Shodh</span>
            </span>
          )}
        </header>
        <div className="pair">
          <h1 className="title">Pair with your computer</h1>
          <p className="panel-text">
            On your computer: Settings › Phone › Pair a phone. End-to-end encrypted.
          </p>
          {refusal && (
            <div className="callout callout-error" role="alert">
              <TriangleAlert aria-hidden="true" className="icon" />
              <p>{`${refusal} Pair again to use this phone.`}</p>
            </div>
          )}
          <button type="button" className="button button-primary button-wide" disabled={busy !== null} onClick={scanCode}>
            {busy === 'scan' ? <LoaderCircle aria-hidden="true" className="icon spin" /> : <ScanQrCode aria-hidden="true" className="icon" />}
            Scan the code
          </button>
          {cameraDenied && (
            <div className="callout">
              <p>Shodh needs the camera to scan the code. Allow it in the app's settings, or paste the code below.</p>
              <button type="button" className="button" onClick={() => void openAppSettings()}>
                <Settings aria-hidden="true" className="icon-small" />
                Open app settings
              </button>
            </div>
          )}
          <form
            className="pair"
            style={{ padding: 0, gap: 8 }}
            onSubmit={event => {
              event.preventDefault();
              if (code.trim()) void pairWith(code);
            }}
          >
            <label htmlFor="pair-code" className="field-label">
              Or paste the code
            </label>
            <textarea
              id="pair-code"
              rows={3}
              value={code}
              onChange={event => setCode(event.target.value)}
              placeholder="shodh-pair:1:…"
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
            />
            <button type="submit" className="button button-wide" disabled={busy !== null || code.trim() === ''}>
              {busy === 'pair' ? <LoaderCircle aria-hidden="true" className="icon-small spin" /> : <ClipboardPaste aria-hidden="true" className="icon-small" />}
              {busy === 'pair' ? 'Pairing…' : 'Pair'}
            </button>
          </form>
          {error && (
            <div className="callout callout-error" role="alert">
              <TriangleAlert aria-hidden="true" className="icon" />
              <p>{error}</p>
            </div>
          )}
        </div>
      </div>
      {aiming && <Scanning onCancel={() => void cancel().catch(() => undefined)} />}
    </motion.div>
  );
}
